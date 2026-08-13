use std::ffi::{OsStr, OsString, c_void};
use std::fs::File;
use std::mem::{size_of, zeroed};
use std::os::windows::ffi::OsStrExt;
use std::ptr::{null, null_mut};
use std::sync::{Arc, Mutex};

use windows_sys::Win32::Foundation::{
    CloseHandle, ERROR_ALREADY_EXISTS, ERROR_FILE_NOT_FOUND, GENERIC_READ, GENERIC_WRITE,
    GetLastError, HANDLE, INVALID_HANDLE_VALUE, SetLastError,
};
use windows_sys::Win32::Security::SECURITY_ATTRIBUTES;
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
};
use windows_sys::Win32::System::JobObjects::{
    CreateJobObjectW, IsProcessInJob, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    JOBOBJECT_BASIC_ACCOUNTING_INFORMATION, JOBOBJECT_BASIC_PROCESS_ID_LIST,
    JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectBasicAccountingInformation,
    JobObjectBasicProcessIdList, JobObjectExtendedLimitInformation, OpenJobObjectW,
    QueryInformationJobObject, SetInformationJobObject, TerminateJobObject,
};
use windows_sys::Win32::System::Threading::{
    CREATE_NEW_PROCESS_GROUP, CREATE_NO_WINDOW, CREATE_SUSPENDED, CreateProcessW,
    DeleteProcThreadAttributeList, EXTENDED_STARTUPINFO_PRESENT, InitializeProcThreadAttributeList,
    PROC_THREAD_ATTRIBUTE_HANDLE_LIST, PROC_THREAD_ATTRIBUTE_JOB_LIST, PROCESS_INFORMATION,
    ResumeThread, STARTF_USESTDHANDLES, STARTUPINFOEXW, UpdateProcThreadAttribute,
    WaitForSingleObject,
};

use super::{ProviderError, argument_digest, process_matches, process_start_token_from_handle};

const JOB_OBJECT_QUERY: u32 = 0x0004;
const JOB_OBJECT_TERMINATE: u32 = 0x0008;
const INFINITE: u32 = u32::MAX;
const THREAD_RESUME_FAILED: u32 = u32::MAX;
#[derive(Debug)]
struct OwnedHandle(HANDLE);

impl OwnedHandle {
    fn new(handle: HANDLE) -> Option<Self> {
        if handle.is_null() || handle == INVALID_HANDLE_VALUE {
            None
        } else {
            Some(Self(handle))
        }
    }

    fn raw(&self) -> HANDLE {
        self.0
    }
}

impl Drop for OwnedHandle {
    fn drop(&mut self) {
        unsafe {
            CloseHandle(self.0);
        }
    }
}

unsafe impl Send for OwnedHandle {}

#[derive(Debug)]
struct AttributeList {
    storage: Vec<usize>,
}

impl AttributeList {
    fn new(attribute_count: u32) -> Result<Self, ProviderError> {
        let mut byte_count = 0_usize;
        unsafe {
            InitializeProcThreadAttributeList(null_mut(), attribute_count, 0, &mut byte_count);
        }
        if byte_count == 0 {
            return Err(last_command_error(
                "Windows process attribute-list sizing failed",
            ));
        }
        let words = byte_count.div_ceil(size_of::<usize>());
        let mut storage = vec![0_usize; words];
        let result = unsafe {
            InitializeProcThreadAttributeList(
                storage.as_mut_ptr().cast(),
                attribute_count,
                0,
                &mut byte_count,
            )
        };
        if result == 0 {
            return Err(last_command_error(
                "Windows process attribute-list initialization failed",
            ));
        }
        Ok(Self { storage })
    }

    fn raw(&mut self) -> *mut c_void {
        self.storage.as_mut_ptr().cast()
    }

    fn update_slice<T>(&mut self, attribute: usize, value: &[T]) -> Result<(), ProviderError> {
        let result = unsafe {
            UpdateProcThreadAttribute(
                self.raw(),
                0,
                attribute,
                value.as_ptr().cast(),
                size_of_val(value),
                null_mut(),
                null(),
            )
        };
        if result == 0 {
            return Err(last_command_error(
                "Windows process attribute-list update failed",
            ));
        }
        Ok(())
    }
}

impl Drop for AttributeList {
    fn drop(&mut self) {
        unsafe {
            DeleteProcThreadAttributeList(self.raw());
        }
    }
}

#[derive(Debug)]
pub(super) struct PreparedQemuProcess {
    process_id: u32,
    process_start_token: u64,
    command_sha256: String,
    process: Option<OwnedHandle>,
    primary_thread: OwnedHandle,
    job: OwnedHandle,
    launch_guard: OwnedHandle,
    terminate_on_drop: bool,
}

impl PreparedQemuProcess {
    pub(super) fn process_id(&self) -> u32 {
        self.process_id
    }

    pub(super) fn process_start_token(&self) -> u64 {
        self.process_start_token
    }

    pub(super) fn command_sha256(&self) -> &str {
        &self.command_sha256
    }

    pub(super) fn activate(mut self) -> Result<(), ProviderError> {
        let process = self.process.take().ok_or_else(|| {
            ProviderError::OwnershipChanged(
                "prepared Windows QEMU process handle was unavailable".to_owned(),
            )
        })?;
        let shared = Arc::new(Mutex::new(Some(process)));
        let waiter = Arc::clone(&shared);
        let reaper = std::thread::Builder::new()
            .name("vmcell-qemu-reaper".to_owned())
            .spawn(move || {
                if let Some(process) = waiter.lock().ok().and_then(|mut value| value.take()) {
                    unsafe {
                        WaitForSingleObject(process.raw(), INFINITE);
                    }
                }
            });
        if reaper.is_err() {
            self.process = shared.lock().ok().and_then(|mut value| value.take());
            return Err(ProviderError::Command(
                "QEMU process reaper could not be started".to_owned(),
            ));
        }

        if unsafe { ResumeThread(self.primary_thread.raw()) } == THREAD_RESUME_FAILED {
            return Err(last_command_error(
                "Windows QEMU primary thread could not be resumed",
            ));
        }
        set_kill_on_job_close(self.launch_guard.raw(), false)?;
        set_kill_on_job_close(self.job.raw(), false)?;
        self.terminate_on_drop = false;
        Ok(())
    }
}

impl Drop for PreparedQemuProcess {
    fn drop(&mut self) {
        if self.terminate_on_drop {
            unsafe {
                TerminateJobObject(self.job.raw(), 1);
                TerminateJobObject(self.launch_guard.raw(), 1);
            }
        }
    }
}

pub(super) fn prepare_qemu_process(
    program: &OsStr,
    args: &[OsString],
    job_name: &str,
    executable_sha256: &str,
    _pinned_executable: &File,
    command_sha256: &str,
) -> Result<PreparedQemuProcess, ProviderError> {
    if argument_digest(args) != command_sha256 {
        return Err(ProviderError::OwnershipChanged(
            "Windows QEMU command buffer drifted from its durable launch digest".to_owned(),
        ));
    }
    let stdio_security = SECURITY_ATTRIBUTES {
        nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: null_mut(),
        bInheritHandle: 1,
    };
    let mut job_name_wide = wide_nul(OsStr::new(job_name))?;
    unsafe {
        SetLastError(0);
    }
    let job = OwnedHandle::new(unsafe { CreateJobObjectW(null(), job_name_wide.as_mut_ptr()) })
        .ok_or_else(|| last_command_error("Windows QEMU Job Object creation failed"))?;
    if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
        return Err(ProviderError::Collision(
            "Windows QEMU Job Object identity already exists".to_owned(),
        ));
    }
    set_kill_on_job_close(job.raw(), true)?;
    let inherited_job =
        OwnedHandle::new(unsafe { OpenJobObjectW(JOB_OBJECT_QUERY, 1, job_name_wide.as_ptr()) })
            .ok_or_else(|| {
                last_command_error("Windows QEMU inherited Job receipt creation failed")
            })?;
    let launch_guard = OwnedHandle::new(unsafe { CreateJobObjectW(null(), null()) })
        .ok_or_else(|| last_command_error("Windows QEMU launch-guard creation failed"))?;
    set_kill_on_job_close(launch_guard.raw(), true)?;

    let nul_name = wide_nul(OsStr::new("NUL"))?;
    let nul = OwnedHandle::new(unsafe {
        CreateFileW(
            nul_name.as_ptr(),
            GENERIC_READ | GENERIC_WRITE,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            &stdio_security,
            OPEN_EXISTING,
            FILE_ATTRIBUTE_NORMAL,
            null_mut(),
        )
    })
    .ok_or_else(|| last_command_error("Windows null-device handle creation failed"))?;

    let mut attributes = AttributeList::new(2)?;
    let job_chain = [launch_guard.raw(), job.raw()];
    attributes.update_slice(PROC_THREAD_ATTRIBUTE_JOB_LIST as usize, &job_chain)?;
    let inherited_handles = [nul.raw(), inherited_job.raw()];
    attributes.update_slice(
        PROC_THREAD_ATTRIBUTE_HANDLE_LIST as usize,
        &inherited_handles,
    )?;

    let mut startup = STARTUPINFOEXW::default();
    startup.StartupInfo.cb = size_of::<STARTUPINFOEXW>() as u32;
    startup.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
    startup.StartupInfo.hStdInput = nul.raw();
    startup.StartupInfo.hStdOutput = nul.raw();
    startup.StartupInfo.hStdError = nul.raw();
    startup.lpAttributeList = attributes.raw();

    let application = wide_nul(program)?;
    let mut command_line = windows_command_line(program, args)?;
    let mut process_information: PROCESS_INFORMATION = unsafe { zeroed() };
    let created = unsafe {
        CreateProcessW(
            application.as_ptr(),
            command_line.as_mut_ptr(),
            null(),
            null(),
            1,
            EXTENDED_STARTUPINFO_PRESENT
                | CREATE_SUSPENDED
                | CREATE_NEW_PROCESS_GROUP
                | CREATE_NO_WINDOW,
            null(),
            null(),
            (&startup.StartupInfo) as *const _,
            &mut process_information,
        )
    };
    if created == 0 {
        return Err(last_command_error(
            "atomic Windows QEMU Job Object launch failed",
        ));
    }

    let process = OwnedHandle::new(process_information.hProcess).ok_or_else(|| {
        ProviderError::Command("Windows QEMU process handle was unavailable".to_owned())
    })?;
    let primary_thread = OwnedHandle::new(process_information.hThread).ok_or_else(|| {
        ProviderError::Command("Windows QEMU primary-thread handle was unavailable".to_owned())
    })?;
    let process_start_token =
        process_start_token_from_handle(process.raw()).filter(|value| *value != 0);
    let Some(process_start_token) = process_start_token else {
        unsafe {
            TerminateJobObject(job.raw(), 1);
        }
        return Err(ProviderError::Command(
            "QEMU process start identity was unavailable".to_owned(),
        ));
    };
    if !process_matches(
        process_information.dwProcessId,
        process_start_token,
        program,
        &argument_digest(args),
        executable_sha256,
        Some(job_name),
    ) {
        unsafe {
            TerminateJobObject(job.raw(), 1);
        }
        return Err(ProviderError::OwnershipChanged(
            "atomically contained QEMU process identity did not match its launch receipt"
                .to_owned(),
        ));
    }

    Ok(PreparedQemuProcess {
        process_id: process_information.dwProcessId,
        process_start_token,
        command_sha256: command_sha256.to_owned(),
        process: Some(process),
        primary_thread,
        job,
        launch_guard,
        terminate_on_drop: true,
    })
}

pub(super) fn process_is_in_job(process: HANDLE, job_name: &str) -> bool {
    let Ok(name) = wide_nul(OsStr::new(job_name)) else {
        return false;
    };
    let Some(job) = OwnedHandle::new(unsafe { OpenJobObjectW(JOB_OBJECT_QUERY, 0, name.as_ptr()) })
    else {
        return false;
    };
    let mut contained = 0;
    unsafe { IsProcessInJob(process, job.raw(), &mut contained) != 0 && contained != 0 }
}

pub(super) fn job_is_empty_or_missing(job_name: &str) -> bool {
    matches!(observe_exact_job(job_name), ExactJobObservation::Empty)
}

pub(super) fn job_is_exact_nonempty(job_name: &str) -> bool {
    matches!(observe_exact_job(job_name), ExactJobObservation::NonEmpty)
}

enum ExactJobObservation {
    Empty,
    NonEmpty,
    Unprovable,
}

fn observe_exact_job(job_name: &str) -> ExactJobObservation {
    let Ok(name) = wide_nul(OsStr::new(job_name)) else {
        return ExactJobObservation::Unprovable;
    };
    let Some(job) = OwnedHandle::new(unsafe { OpenJobObjectW(JOB_OBJECT_QUERY, 0, name.as_ptr()) })
    else {
        return if unsafe { GetLastError() } == ERROR_FILE_NOT_FOUND {
            ExactJobObservation::Empty
        } else {
            ExactJobObservation::Unprovable
        };
    };
    let mut accounting = JOBOBJECT_BASIC_ACCOUNTING_INFORMATION::default();
    let accounting_ok = unsafe {
        QueryInformationJobObject(
            job.raw(),
            JobObjectBasicAccountingInformation,
            (&mut accounting as *mut JOBOBJECT_BASIC_ACCOUNTING_INFORMATION).cast(),
            size_of::<JOBOBJECT_BASIC_ACCOUNTING_INFORMATION>() as u32,
            null_mut(),
        )
    } != 0;
    if !accounting_ok {
        return ExactJobObservation::Unprovable;
    }
    if accounting.ActiveProcesses != 0 {
        return ExactJobObservation::NonEmpty;
    }
    let mut processes = JOBOBJECT_BASIC_PROCESS_ID_LIST::default();
    let list_ok = unsafe {
        QueryInformationJobObject(
            job.raw(),
            JobObjectBasicProcessIdList,
            (&mut processes as *mut JOBOBJECT_BASIC_PROCESS_ID_LIST).cast(),
            size_of::<JOBOBJECT_BASIC_PROCESS_ID_LIST>() as u32,
            null_mut(),
        )
    } != 0;
    if !list_ok {
        ExactJobObservation::Unprovable
    } else if processes.NumberOfAssignedProcesses == 0 && processes.NumberOfProcessIdsInList == 0 {
        ExactJobObservation::Empty
    } else {
        ExactJobObservation::NonEmpty
    }
}

pub(super) fn terminate_exact_job(job_name: &str) -> Result<(), ProviderError> {
    let name = wide_nul(OsStr::new(job_name))?;
    let job = OwnedHandle::new(unsafe {
        OpenJobObjectW(JOB_OBJECT_QUERY | JOB_OBJECT_TERMINATE, 0, name.as_ptr())
    })
    .ok_or_else(|| {
        ProviderError::OwnershipChanged(
            "exact Windows QEMU Job Object could not be opened for cleanup".to_owned(),
        )
    })?;
    if unsafe { TerminateJobObject(job.raw(), 1) } == 0 {
        return Err(last_command_error(
            "exact Windows QEMU Job Object termination failed",
        ));
    }
    Ok(())
}

fn set_kill_on_job_close(job: HANDLE, enabled: bool) -> Result<(), ProviderError> {
    let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
    if enabled {
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
    }
    let result = unsafe {
        SetInformationJobObject(
            job,
            JobObjectExtendedLimitInformation,
            (&limits as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
            size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
        )
    };
    if result == 0 {
        return Err(last_command_error(
            "Windows QEMU Job Object limit update failed",
        ));
    }
    Ok(())
}

fn windows_command_line(program: &OsStr, args: &[OsString]) -> Result<Vec<u16>, ProviderError> {
    let mut command_line = quote_windows_argument(program)?;
    for argument in args {
        command_line.push(b' ' as u16);
        command_line.extend(quote_windows_argument(argument)?);
    }
    command_line.push(0);
    if command_line.len() > 32_767 {
        return Err(ProviderError::Command(
            "Windows QEMU command line exceeded the CreateProcessW limit".to_owned(),
        ));
    }
    Ok(command_line)
}

fn quote_windows_argument(argument: &OsStr) -> Result<Vec<u16>, ProviderError> {
    let units = argument.encode_wide().collect::<Vec<_>>();
    if units.contains(&0) {
        return Err(ProviderError::Command(
            "Windows QEMU command line contained a null character".to_owned(),
        ));
    }
    if !units.is_empty()
        && !units.iter().any(|unit| {
            *unit == u16::from(b' ') || *unit == u16::from(b'\t') || *unit == u16::from(b'"')
        })
    {
        return Ok(units);
    }
    let mut quoted = vec![b'"' as u16];
    let mut backslashes = 0_usize;
    for unit in units {
        if unit == b'\\' as u16 {
            backslashes += 1;
        } else if unit == b'"' as u16 {
            quoted.extend(std::iter::repeat_n(b'\\' as u16, backslashes * 2 + 1));
            quoted.push(unit);
            backslashes = 0;
        } else {
            quoted.extend(std::iter::repeat_n(b'\\' as u16, backslashes));
            backslashes = 0;
            quoted.push(unit);
        }
    }
    quoted.extend(std::iter::repeat_n(b'\\' as u16, backslashes * 2));
    quoted.push(b'"' as u16);
    Ok(quoted)
}

fn wide_nul(value: &OsStr) -> Result<Vec<u16>, ProviderError> {
    let mut wide = value.encode_wide().collect::<Vec<_>>();
    if wide.contains(&0) {
        return Err(ProviderError::Command(
            "Windows QEMU identity contained a null character".to_owned(),
        ));
    }
    wide.push(0);
    Ok(wide)
}

fn last_command_error(context: &str) -> ProviderError {
    ProviderError::Command(format!("{context}: Windows error {}", unsafe {
        GetLastError()
    }))
}

#[cfg(test)]
mod tests {
    use std::os::windows::ffi::OsStringExt;
    use std::time::{Duration, Instant};

    use super::*;
    use crate::providers::qemu::pinned_ordinary_file_sha256;

    fn prepare_test_process(
        program: &OsStr,
        args: &[OsString],
        job_name: &str,
        executable_sha256: &str,
        pinned_executable: &File,
    ) -> Result<PreparedQemuProcess, ProviderError> {
        let command_sha256 = argument_digest(args);
        prepare_qemu_process(
            program,
            args,
            job_name,
            executable_sha256,
            pinned_executable,
            &command_sha256,
        )
    }

    #[test]
    fn argument_quoting_preserves_spaces_quotes_backslashes_and_unicode() {
        let cases = [
            ("plain", "plain"),
            ("", "\"\""),
            ("two words", "\"two words\""),
            ("a\"b", "\"a\\\"b\""),
            (r"C:\path with space\", r#""C:\path with space\\""#),
            ("数据", "数据"),
        ];
        for (input, expected) in cases {
            let actual = OsString::from_wide(&quote_windows_argument(OsStr::new(input)).unwrap());
            assert_eq!(actual, OsString::from(expected));
        }
    }

    #[test]
    fn executable_pin_denies_write_and_replacement_through_process_creation_window() {
        let directory = tempfile::tempdir().unwrap();
        let executable = directory.path().join("pinned-qemu-surrogate.exe");
        std::fs::copy(std::env::current_exe().unwrap(), &executable).unwrap();
        let (pinned, hash) = pinned_ordinary_file_sha256(&executable).unwrap();
        assert_eq!(hash.len(), 64);
        assert!(
            std::fs::OpenOptions::new()
                .write(true)
                .open(&executable)
                .is_err()
        );
        assert!(std::fs::rename(&executable, directory.path().join("replacement.exe")).is_err());
        drop(pinned);
        assert!(
            std::fs::OpenOptions::new()
                .write(true)
                .open(&executable)
                .is_ok()
        );
    }

    #[test]
    fn suspended_launch_rejects_command_digest_drift_before_process_creation() {
        let program = std::env::current_exe().unwrap();
        let (pinned, hash) = pinned_ordinary_file_sha256(&program).unwrap();
        let job_name = format!(r"Local\vmcell-qemu-{}", uuid::Uuid::new_v4());
        let result = prepare_qemu_process(
            program.as_os_str(),
            &[OsString::from("--help")],
            &job_name,
            &hash,
            &pinned,
            &"0".repeat(64),
        );
        assert!(matches!(result, Err(ProviderError::OwnershipChanged(_))));
        assert!(job_is_empty_or_missing(&job_name));
    }

    #[test]
    fn atomic_job_membership_precedes_resume_and_drop_proves_empty() {
        let program = std::env::current_exe().unwrap();
        let (pinned, hash) = pinned_ordinary_file_sha256(&program).unwrap();
        let job_name = format!(r"Local\vmcell-qemu-{}", uuid::Uuid::new_v4());
        let prepared = prepare_test_process(
            program.as_os_str(),
            &[OsString::from("--help")],
            &job_name,
            &hash,
            &pinned,
        )
        .unwrap();
        assert!(!job_is_empty_or_missing(&job_name));
        drop(prepared);
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline && !job_is_empty_or_missing(&job_name) {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(job_is_empty_or_missing(&job_name));
    }

    #[test]
    fn job_name_collision_is_rejected() {
        let program = std::env::current_exe().unwrap();
        let (pinned, hash) = pinned_ordinary_file_sha256(&program).unwrap();
        let job_name = format!(r"Local\vmcell-qemu-{}", uuid::Uuid::new_v4());
        let prepared = prepare_test_process(
            program.as_os_str(),
            &[OsString::from("--help")],
            &job_name,
            &hash,
            &pinned,
        )
        .unwrap();
        let collision = prepare_test_process(
            program.as_os_str(),
            &[OsString::from("--help")],
            &job_name,
            &hash,
            &pinned,
        );
        assert!(matches!(collision, Err(ProviderError::Collision(_))));
        drop(prepared);
    }

    #[test]
    fn activated_job_reaches_exact_empty_state() {
        let program = std::env::current_exe().unwrap();
        let (pinned, hash) = pinned_ordinary_file_sha256(&program).unwrap();
        let job_name = format!(r"Local\vmcell-qemu-{}", uuid::Uuid::new_v4());
        let prepared = prepare_test_process(
            program.as_os_str(),
            &[OsString::from("--help")],
            &job_name,
            &hash,
            &pinned,
        )
        .unwrap();
        prepared.activate().unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline && !job_is_empty_or_missing(&job_name) {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(job_is_empty_or_missing(&job_name));
    }

    #[test]
    fn exited_leader_does_not_hide_a_live_descendant_and_exact_job_cleanup_empties_tree() {
        const FIXTURE_MODE: &str = "VMCELL_WINDOWS_JOB_DESCENDANT_MODE";
        const FIXTURE_MARKER: &str = "VMCELL_WINDOWS_JOB_DESCENDANT_MARKER";
        const FIXTURE_JOB: &str = "VMCELL_WINDOWS_JOB_DESCENDANT_NAME";

        let program = std::env::current_exe().unwrap();
        let (pinned, hash) = pinned_ordinary_file_sha256(&program).unwrap();
        let job_name = format!(r"Local\vmcell-qemu-{}", uuid::Uuid::new_v4());
        let marker_directory = tempfile::tempdir().unwrap();
        let marker_root = marker_directory.path().join("fixture");
        let previous_mode = std::env::var_os(FIXTURE_MODE);
        let previous_marker = std::env::var_os(FIXTURE_MARKER);
        let previous_job = std::env::var_os(FIXTURE_JOB);
        unsafe {
            std::env::set_var(FIXTURE_MODE, "parent");
            std::env::set_var(FIXTURE_MARKER, &marker_root);
            std::env::set_var(FIXTURE_JOB, &job_name);
        }
        let prepared = prepare_test_process(
            program.as_os_str(),
            &[
                OsString::from("--ignored"),
                OsString::from("--exact"),
                OsString::from("providers::qemu::windows_job::tests::descendant_fixture_parent"),
            ],
            &job_name,
            &hash,
            &pinned,
        );
        unsafe {
            if let Some(value) = previous_mode {
                std::env::set_var(FIXTURE_MODE, value);
            } else {
                std::env::remove_var(FIXTURE_MODE);
            }
            if let Some(value) = previous_marker {
                std::env::set_var(FIXTURE_MARKER, value);
            } else {
                std::env::remove_var(FIXTURE_MARKER);
            }
            if let Some(value) = previous_job {
                std::env::set_var(FIXTURE_JOB, value);
            } else {
                std::env::remove_var(FIXTURE_JOB);
            }
        }
        let prepared = prepared.unwrap();
        let process_id = prepared.process_id();
        let process_start_token = prepared.process_start_token();
        prepared.activate().unwrap();

        let leaf_marker = marker_root.with_extension("leaf");
        let parent_marker = marker_root.with_extension("parent");
        let marker_deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < marker_deadline
            && (!leaf_marker.is_file() || !parent_marker.is_file())
        {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(
            leaf_marker.is_file(),
            "descendant fixture did not start its leaf"
        );
        let parent_receipt = std::fs::read_to_string(&parent_marker).unwrap();
        let child_id = parent_receipt
            .strip_prefix("child=")
            .and_then(|value| value.split_once(' '))
            .and_then(|(value, _)| value.parse::<u32>().ok())
            .unwrap();
        let child_handle = OwnedHandle::new(unsafe {
            windows_sys::Win32::System::Threading::OpenProcess(
                windows_sys::Win32::System::Threading::PROCESS_QUERY_LIMITED_INFORMATION,
                0,
                child_id,
            )
        })
        .unwrap();
        assert!(
            process_is_in_job(child_handle.raw(), &job_name),
            "live descendant was not inherited into the exact Job Object: {parent_receipt}"
        );

        let leader_deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < leader_deadline
            && !crate::providers::qemu::process_absence_proven(process_id, process_start_token)
        {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(crate::providers::qemu::process_absence_proven(
            process_id,
            process_start_token,
        ));
        assert!(!job_is_empty_or_missing(&job_name));

        let name = wide_nul(OsStr::new(&job_name)).unwrap();
        let job = OwnedHandle::new(unsafe {
            OpenJobObjectW(JOB_OBJECT_QUERY | JOB_OBJECT_TERMINATE, 0, name.as_ptr())
        })
        .unwrap();
        assert_ne!(unsafe { TerminateJobObject(job.raw(), 1) }, 0);
        drop(job);
        let empty_deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < empty_deadline && !job_is_empty_or_missing(&job_name) {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(job_is_empty_or_missing(&job_name));
    }

    #[test]
    #[ignore = "Windows Job descendant fixture; launched by the bounded parent contract test"]
    fn descendant_fixture_parent() {
        use windows_sys::Win32::System::Threading::GetCurrentProcess;

        const FIXTURE_MODE: &str = "VMCELL_WINDOWS_JOB_DESCENDANT_MODE";
        const FIXTURE_MARKER: &str = "VMCELL_WINDOWS_JOB_DESCENDANT_MARKER";
        const FIXTURE_JOB: &str = "VMCELL_WINDOWS_JOB_DESCENDANT_NAME";
        if std::env::var(FIXTURE_MODE).as_deref() != Ok("parent") {
            return;
        }
        let marker = std::path::PathBuf::from(std::env::var_os(FIXTURE_MARKER).unwrap());
        let job_name = std::env::var(FIXTURE_JOB).unwrap();
        let child = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--ignored",
                "--exact",
                "providers::qemu::windows_job::tests::descendant_fixture_leaf",
            ])
            .env(FIXTURE_MODE, "leaf")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap();
        let child_id = child.id();
        drop(child);
        let receipt = format!(
            "child={} exact_job={}",
            child_id,
            process_is_in_job(unsafe { GetCurrentProcess() }, &job_name),
        );
        std::fs::write(marker.with_extension("parent"), receipt).unwrap();
        std::thread::sleep(Duration::from_secs(2));
    }

    #[test]
    #[ignore = "Windows Job descendant fixture; launched by the bounded parent contract test"]
    fn descendant_fixture_leaf() {
        const FIXTURE_MODE: &str = "VMCELL_WINDOWS_JOB_DESCENDANT_MODE";
        const FIXTURE_MARKER: &str = "VMCELL_WINDOWS_JOB_DESCENDANT_MARKER";
        if std::env::var(FIXTURE_MODE).as_deref() != Ok("leaf") {
            return;
        }
        let marker = std::path::PathBuf::from(std::env::var_os(FIXTURE_MARKER).unwrap());
        std::fs::write(marker.with_extension("leaf"), b"ready").unwrap();
        std::thread::sleep(Duration::from_secs(20));
    }

    #[test]
    fn exact_job_termination_cannot_target_a_missing_identity() {
        let job_name = format!(r"Local\vmcell-qemu-{}", uuid::Uuid::new_v4());
        let name = wide_nul(OsStr::new(&job_name)).unwrap();
        let handle =
            unsafe { OpenJobObjectW(JOB_OBJECT_QUERY | JOB_OBJECT_TERMINATE, 0, name.as_ptr()) };
        assert!(handle.is_null());
        assert!(job_is_empty_or_missing(&job_name));
    }
}
