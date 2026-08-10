#[cfg(any(target_os = "windows", test))]
use std::io::Read;
#[cfg(target_os = "windows")]
use std::io::Write;
#[cfg(target_os = "windows")]
use std::process::{Command, Stdio};
#[cfg(target_os = "windows")]
use std::thread;
use std::time::Duration;
#[cfg(target_os = "windows")]
use std::time::Instant;

#[cfg(target_os = "windows")]
use std::os::windows::io::AsRawHandle;

use serde::Serialize;
use serde_json::Value;
#[cfg(target_os = "windows")]
use zeroize::{Zeroize, Zeroizing};

use crate::core::cell::CellId;
use crate::core::guest::GuestOperationId;
use crate::guest::{GuestCredentials, GuestIoError, OverwritePolicy};
use crate::providers::ProviderVm;

#[cfg(any(target_os = "windows", test))]
const COMMON_SCRIPT: &str = include_str!("scripts/common.ps1");
#[cfg(any(target_os = "windows", test))]
const PROBE_READY_SCRIPT: &str = include_str!("scripts/probe_ready.ps1");
#[cfg(any(target_os = "windows", test))]
const EXEC_SCRIPT: &str = include_str!("scripts/exec.ps1");
#[cfg(any(target_os = "windows", test))]
const COPY_IN_SCRIPT: &str = include_str!("scripts/copy_in.ps1");
#[cfg(any(target_os = "windows", test))]
const COPY_OUT_SCRIPT: &str = include_str!("scripts/copy_out.ps1");
#[cfg(target_os = "windows")]
const SMALL_RESPONSE_LIMIT: u64 = 65_536;
#[cfg(target_os = "windows")]
const STDERR_LIMIT: usize = 65_536;

#[derive(Serialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub(crate) enum PowerShellDirectAction {
    ProbeReady {
        cell_id: CellId,
        expected: ProviderVm,
    },
    Exec {
        cell_id: CellId,
        expected: ProviderVm,
        program: String,
        command_line: String,
        timeout_ms: u64,
        max_output_bytes: u64,
    },
    CopyIn {
        cell_id: CellId,
        operation_id: GuestOperationId,
        expected: ProviderVm,
        destination: String,
        overwrite: OverwritePolicy,
    },
    CopyOut {
        cell_id: CellId,
        operation_id: GuestOperationId,
        expected: ProviderVm,
        source: String,
        max_bytes: u64,
    },
}

pub(crate) trait PowerShellDirectCommandExecutor: Send + Sync {
    fn execute(
        &self,
        action: &PowerShellDirectAction,
        credentials: &GuestCredentials,
        payload: Option<&[u8]>,
        timeout: Duration,
    ) -> Result<Value, GuestIoError>;
}

pub struct PowerShellDirectExecutor;

impl PowerShellDirectCommandExecutor for PowerShellDirectExecutor {
    fn execute(
        &self,
        action: &PowerShellDirectAction,
        credentials: &GuestCredentials,
        payload: Option<&[u8]>,
        timeout: Duration,
    ) -> Result<Value, GuestIoError> {
        #[cfg(not(target_os = "windows"))]
        {
            let _ = (action, credentials, payload, timeout);
            Err(GuestIoError::NotImplemented("powershell-direct"))
        }
        #[cfg(target_os = "windows")]
        {
            execute_powershell_direct(action, credentials, payload, timeout)
        }
    }
}

#[cfg(target_os = "windows")]
fn execute_powershell_direct(
    action: &PowerShellDirectAction,
    credentials: &GuestCredentials,
    payload: Option<&[u8]>,
    timeout: Duration,
) -> Result<Value, GuestIoError> {
    const POWERSHELL: &str = r"C:\Windows\System32\WindowsPowerShell\v1.0\powershell.exe";

    let action_json = serde_json::to_vec(action).map_err(|_| GuestIoError::Transport)?;
    let mut input = Zeroizing::new(Vec::new());
    append_frame(&mut input, &action_json)?;
    append_frame(&mut input, credentials.username().as_bytes())?;
    append_frame(&mut input, credentials.password().as_bytes())?;
    append_frame(&mut input, payload.unwrap_or_default())?;
    let script = format!("{COMMON_SCRIPT}\n{}", script_for(action));
    let mut child = Command::new(POWERSHELL)
        .args(["-NoLogo", "-NoProfile", "-NonInteractive", "-Command"])
        .arg(script)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|_| GuestIoError::Transport)?;
    let job = ProcessJob::assign(&mut child)?;

    let stdout_limit =
        usize::try_from(stdout_limit(action)?).map_err(|_| GuestIoError::OutputLimit)?;
    let (status, stdout, stderr) =
        wait_for_bounded_child(child, job, input, timeout, stdout_limit)?;
    if !status.success() {
        return Err(classify_stderr(&stderr));
    }
    serde_json::from_slice(&stdout).map_err(|_| GuestIoError::InvalidResponse)
}

#[cfg(target_os = "windows")]
fn wait_for_bounded_child(
    mut child: std::process::Child,
    job: ProcessJob,
    mut input: Zeroizing<Vec<u8>>,
    timeout: Duration,
    stdout_limit: usize,
) -> Result<(std::process::ExitStatus, Vec<u8>, Vec<u8>), GuestIoError> {
    let mut stdin = child.stdin.take().ok_or(GuestIoError::Transport)?;
    let stdout = child.stdout.take().ok_or(GuestIoError::Transport)?;
    let stderr = child.stderr.take().ok_or(GuestIoError::Transport)?;
    let stdin_writer = thread::spawn(move || {
        let result = stdin.write_all(&input).map_err(|_| GuestIoError::Transport);
        drop(stdin);
        input.zeroize();
        result
    });
    let stdout_reader = thread::spawn(move || read_pipe_limited(stdout, stdout_limit));
    let stderr_reader = thread::spawn(move || read_pipe_limited(stderr, STDERR_LIMIT));
    let deadline = Instant::now() + timeout;
    let status = loop {
        if let Some(status) = child.try_wait().map_err(|_| GuestIoError::Transport)? {
            break status;
        }
        if Instant::now() >= deadline {
            job.terminate();
            let _ = child.kill();
            let _ = child.wait();
            let _ = stdin_writer.join();
            let _ = stdout_reader.join();
            let _ = stderr_reader.join();
            return Err(GuestIoError::Timeout);
        }
        thread::sleep(Duration::from_millis(20));
    };
    job.terminate();
    stdin_writer.join().map_err(|_| GuestIoError::Transport)??;
    let stdout = stdout_reader
        .join()
        .map_err(|_| GuestIoError::Transport)??;
    let stderr = stderr_reader
        .join()
        .map_err(|_| GuestIoError::Transport)??;
    Ok((status, stdout, stderr))
}

#[cfg(target_os = "windows")]
struct ProcessJob(*mut core::ffi::c_void);

#[cfg(target_os = "windows")]
impl ProcessJob {
    fn assign(child: &mut std::process::Child) -> Result<Self, GuestIoError> {
        use windows_sys::Win32::System::JobObjects::{
            JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
            JobObjectExtendedLimitInformation, SetInformationJobObject,
        };
        let handle = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
        if handle.is_null() {
            let _ = child.kill();
            let _ = child.wait();
            return Err(GuestIoError::Transport);
        }
        let job = Self(handle);
        let mut limits = unsafe { std::mem::zeroed::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() };
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        if unsafe {
            SetInformationJobObject(
                job.0,
                JobObjectExtendedLimitInformation,
                std::ptr::addr_of!(limits).cast(),
                std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            )
        } == 0
        {
            let _ = child.kill();
            let _ = child.wait();
            return Err(GuestIoError::Transport);
        }
        if unsafe { AssignProcessToJobObject(job.0, child.as_raw_handle()) } == 0 {
            let _ = child.kill();
            let _ = child.wait();
            return Err(GuestIoError::Transport);
        }
        Ok(job)
    }

    fn terminate(&self) {
        unsafe {
            TerminateJobObject(self.0, 1);
        }
    }
}

#[cfg(target_os = "windows")]
impl Drop for ProcessJob {
    fn drop(&mut self) {
        unsafe {
            TerminateJobObject(self.0, 1);
            CloseHandle(self.0);
        }
    }
}

#[cfg(target_os = "windows")]
#[link(name = "kernel32")]
unsafe extern "system" {
    fn CreateJobObjectW(
        job_attributes: *const core::ffi::c_void,
        name: *const u16,
    ) -> *mut core::ffi::c_void;
    fn AssignProcessToJobObject(
        job: *mut core::ffi::c_void,
        process: *mut core::ffi::c_void,
    ) -> i32;
    fn TerminateJobObject(job: *mut core::ffi::c_void, exit_code: u32) -> i32;
    fn CloseHandle(handle: *mut core::ffi::c_void) -> i32;
}

#[cfg(target_os = "windows")]
fn append_frame(target: &mut Vec<u8>, bytes: &[u8]) -> Result<(), GuestIoError> {
    let length = u32::try_from(bytes.len()).map_err(|_| GuestIoError::Transport)?;
    target.extend_from_slice(&length.to_le_bytes());
    target.extend_from_slice(bytes);
    Ok(())
}

#[cfg(any(target_os = "windows", test))]
fn read_pipe_limited(mut pipe: impl Read, limit: usize) -> Result<Vec<u8>, GuestIoError> {
    let mut bytes = Vec::new();
    let mut buffer = [0_u8; 8192];
    loop {
        let read = pipe
            .read(&mut buffer)
            .map_err(|_| GuestIoError::Transport)?;
        if read == 0 {
            break;
        }
        if bytes.len().saturating_add(read) > limit {
            return Err(GuestIoError::OutputLimit);
        }
        bytes.extend_from_slice(&buffer[..read]);
    }
    Ok(bytes)
}

#[cfg(target_os = "windows")]
fn stdout_limit(action: &PowerShellDirectAction) -> Result<u64, GuestIoError> {
    match action {
        PowerShellDirectAction::ProbeReady { .. } | PowerShellDirectAction::CopyIn { .. } => {
            Ok(SMALL_RESPONSE_LIMIT)
        }
        PowerShellDirectAction::Exec {
            max_output_bytes, ..
        } => max_output_bytes
            .checked_mul(6)
            .and_then(|value| value.checked_add(SMALL_RESPONSE_LIMIT))
            .ok_or(GuestIoError::OutputLimit),
        PowerShellDirectAction::CopyOut { max_bytes, .. } => max_bytes
            .checked_add(2)
            .map(|value| value / 3)
            .and_then(|value| value.checked_mul(4))
            .and_then(|value| value.checked_add(SMALL_RESPONSE_LIMIT))
            .ok_or(GuestIoError::OutputLimit),
    }
}

#[cfg(target_os = "windows")]
fn classify_stderr(stderr: &[u8]) -> GuestIoError {
    let text = String::from_utf8_lossy(stderr);
    if text.contains("OWNERSHIP_CHANGED:") {
        GuestIoError::OwnershipChanged
    } else if text.contains("GUEST_AUTHENTICATION_FAILED:") {
        GuestIoError::AuthenticationFailed
    } else if text.contains("GUEST_NOT_READY:") {
        GuestIoError::GuestNotReady
    } else if text.contains("GUEST_SESSION_FAILED:") {
        GuestIoError::SessionFailed
    } else if text.contains("GUEST_TIMEOUT:") || text.contains("GUEST_UNKNOWN:") {
        GuestIoError::Timeout
    } else if text.contains("GUEST_OUTPUT_LIMIT:") {
        GuestIoError::OutputLimit
    } else if text.contains("GUEST_INVALID_ENCODING:") {
        GuestIoError::InvalidResponse
    } else if text.contains("GUEST_PATH_VIOLATION:") {
        GuestIoError::PathViolation
    } else if text.contains("GUEST_PARTIAL_COPY:") {
        GuestIoError::PartialCopy
    } else {
        GuestIoError::Transport
    }
}

#[cfg(target_os = "windows")]
fn script_for(action: &PowerShellDirectAction) -> &'static str {
    match action {
        PowerShellDirectAction::ProbeReady { .. } => PROBE_READY_SCRIPT,
        PowerShellDirectAction::Exec { .. } => EXEC_SCRIPT,
        PowerShellDirectAction::CopyIn { .. } => COPY_IN_SCRIPT,
        PowerShellDirectAction::CopyOut { .. } => COPY_OUT_SCRIPT,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn guest_scripts_forbid_host_lifecycle_and_secret_channels() {
        let scripts = [
            COMMON_SCRIPT,
            PROBE_READY_SCRIPT,
            EXEC_SCRIPT,
            COPY_IN_SCRIPT,
            COPY_OUT_SCRIPT,
        ];
        for script in scripts {
            for forbidden in [
                "Enable-WindowsOptionalFeature",
                "Disable-WindowsOptionalFeature",
                "New-VMSwitch",
                "Set-VMSwitch",
                "Remove-VMSwitch",
                "Start-VM",
                "Stop-VM",
                "Remove-VM",
                "-VMName",
                "GetEnvironmentVariable",
            ] {
                assert!(!script.contains(forbidden), "forbidden token: {forbidden}");
            }
        }
    }

    #[test]
    fn every_action_uses_guid_snapshot_mutex_and_session_cleanup() {
        assert!(COMMON_SCRIPT.contains("Global\\vmcell-hyperv-provider-v1"));
        assert!(COMMON_SCRIPT.contains("Get-VM -Id"));
        assert!(COMMON_SCRIPT.contains("Assert-ExpectedVm"));
        for script in [
            PROBE_READY_SCRIPT,
            EXEC_SCRIPT,
            COPY_IN_SCRIPT,
            COPY_OUT_SCRIPT,
        ] {
            assert!(script.contains("Enter-GuestAction"));
            assert!(script.contains("Assert-ExpectedVm"));
            assert!(script.contains("Close-GuestSession"));
            assert!(script.contains("Exit-GuestAction"));
        }
        assert!(PROBE_READY_SCRIPT.contains("status = 'guest_not_ready'"));
        assert!(PROBE_READY_SCRIPT.contains("status = 'session_failed'"));
        assert!(COMMON_SCRIPT.contains("$script:providerMutexHeld = $true"));
        assert!(EXEC_SCRIPT.contains("GUEST_INVALID_ENCODING:"));
        assert!(EXEC_SCRIPT.contains("UTF8Encoding]::new($false, $true)"));
        assert!(EXEC_SCRIPT.contains("taskkill.exe"));
        assert!(
            COPY_OUT_SCRIPT
                .matches("Assert-OrdinaryDirectory $current")
                .count()
                >= 2
        );
        assert!(COPY_IN_SCRIPT.contains(",([byte[]]$guestPayloadBytes)"));
    }

    #[test]
    fn untrusted_child_pipe_is_hard_capped() {
        let bytes = vec![b'x'; 32];
        assert_eq!(read_pipe_limited(bytes.as_slice(), 32).unwrap(), bytes);
        assert_eq!(
            read_pipe_limited(bytes.as_slice(), 31).unwrap_err(),
            GuestIoError::OutputLimit
        );
    }

    #[cfg(windows)]
    #[test]
    fn timed_out_child_is_killed_and_reaped() {
        if std::env::var_os("VMCELL_TEST_GUEST_EXECUTOR_GRANDCHILD").is_some() {
            thread::sleep(Duration::from_secs(2));
            std::fs::write(
                std::env::var_os("VMCELL_TEST_GUEST_EXECUTOR_MARKER").unwrap(),
                b"escaped",
            )
            .unwrap();
            return;
        }
        if std::env::var_os("VMCELL_TEST_GUEST_EXECUTOR_SLEEP_CHILD").is_some() {
            thread::sleep(Duration::from_millis(250));
            let mut command = Command::new(std::env::current_exe().unwrap());
            let mut grandchild = command
                .arg("--exact")
                .arg(
                    "guest::powershell_direct::executor::tests::timed_out_child_is_killed_and_reaped",
                )
                .arg("--nocapture")
                .env("VMCELL_TEST_GUEST_EXECUTOR_GRANDCHILD", "1")
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .unwrap();
            thread::sleep(Duration::from_secs(60));
            let _ = grandchild.kill();
            let _ = grandchild.wait();
            return;
        }
        let directory = tempfile::tempdir().unwrap();
        let marker = directory.path().join("escaped-child");
        let mut command = Command::new(std::env::current_exe().unwrap());
        command
            .arg("--exact")
            .arg("guest::powershell_direct::executor::tests::timed_out_child_is_killed_and_reaped")
            .arg("--nocapture")
            .env("VMCELL_TEST_GUEST_EXECUTOR_SLEEP_CHILD", "1")
            .env("VMCELL_TEST_GUEST_EXECUTOR_MARKER", &marker)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = command.spawn().unwrap();
        let job = ProcessJob::assign(&mut child).unwrap();
        let started = Instant::now();
        assert_eq!(
            wait_for_bounded_child(
                child,
                job,
                Zeroizing::new(Vec::new()),
                Duration::from_secs(1),
                SMALL_RESPONSE_LIMIT as usize,
            )
            .unwrap_err(),
            GuestIoError::Timeout
        );
        assert!(started.elapsed() < Duration::from_secs(5));
        thread::sleep(Duration::from_secs(3));
        assert!(!marker.exists(), "a timed-out descendant escaped cleanup");
    }
}
