#[cfg(target_os = "windows")]
use std::io::{Read, Write};
#[cfg(target_os = "windows")]
use std::process::{Command, Stdio};
#[cfg(target_os = "windows")]
use std::thread;
#[cfg(target_os = "windows")]
use std::time::{Duration, Instant};

#[cfg(target_os = "windows")]
use std::os::windows::io::AsRawHandle;

use serde::Serialize;
use serde_json::Value;

use crate::providers::{
    ClaimVmRequest, ConfigureVmRequest, CreateVmRequest, ProviderError, ProviderVm, VmLookup,
};

const PROBE_SCRIPT: &str = include_str!("scripts/probe.ps1");
const INSPECT_VHD_SCRIPT: &str = include_str!("scripts/inspect_vhd.ps1");
const CREATE_DIFFERENCING_VHD_SCRIPT: &str = include_str!("scripts/create_differencing_vhd.ps1");
const CREATE_VM_SCRIPT: &str = include_str!("scripts/create_vm.ps1");
const CLAIM_VM_SCRIPT: &str = include_str!("scripts/claim_vm.ps1");
const CONFIGURE_VM_SCRIPT: &str = include_str!("scripts/configure_vm.ps1");
const INSPECT_VM_SCRIPT: &str = include_str!("scripts/inspect_vm.ps1");
const START_VM_SCRIPT: &str = include_str!("scripts/start_vm.ps1");
const STOP_VM_SCRIPT: &str = include_str!("scripts/stop_vm.ps1");
const REMOVE_VM_SCRIPT: &str = include_str!("scripts/remove_vm.ps1");
#[cfg(target_os = "windows")]
const PROVIDER_COMMAND_TIMEOUT: Duration = Duration::from_secs(120);
#[cfg(target_os = "windows")]
const PROVIDER_OUTPUT_LIMIT: usize = 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum HyperVAction {
    Probe,
    InspectVhd {
        path: std::path::PathBuf,
    },
    CreateDifferencingVhd {
        parent_path: std::path::PathBuf,
        overlay_path: std::path::PathBuf,
    },
    CreateVm {
        #[serde(flatten)]
        request: CreateVmRequest,
    },
    ClaimVm {
        #[serde(flatten)]
        request: ClaimVmRequest,
    },
    ConfigureVm {
        #[serde(flatten)]
        request: ConfigureVmRequest,
    },
    InspectVm {
        lookup: VmLookup,
    },
    StartVm {
        expected: ProviderVm,
    },
    StopVm {
        expected: ProviderVm,
    },
    RemoveVm {
        expected: ProviderVm,
    },
}

pub trait HyperVCommandExecutor: Send + Sync {
    fn execute(&self, action: HyperVAction) -> Result<Value, ProviderError>;
}

pub struct PowerShellHyperVExecutor;

impl HyperVCommandExecutor for PowerShellHyperVExecutor {
    fn execute(&self, action: HyperVAction) -> Result<Value, ProviderError> {
        #[cfg(not(target_os = "windows"))]
        {
            let _ = action;
            return Err(ProviderError::Unsupported {
                provider: "hyperv",
                operation: "powershell_command",
            });
        }

        #[cfg(target_os = "windows")]
        {
            execute_powershell(action)
        }
    }
}

#[cfg(target_os = "windows")]
fn execute_powershell(action: HyperVAction) -> Result<Value, ProviderError> {
    const POWERSHELL: &str = r"C:\Windows\System32\WindowsPowerShell\v1.0\powershell.exe";

    let input = serde_json::to_vec(&action)
        .map_err(|error| ProviderError::Command(format!("failed to encode request: {error}")))?;
    let mut child = Command::new(POWERSHELL)
        .args([
            "-NoLogo",
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            script_for(&action),
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| ProviderError::Command(format!("failed to launch PowerShell: {error}")))?;
    let job = ProviderProcessJob::assign(&mut child)?;

    let (status, stdout, stderr) = wait_for_bounded_provider_child(child, job, input)?;
    if !status.success() {
        let detail = String::from_utf8_lossy(&stderr).trim().to_owned();
        if let Some(reason) = detail.strip_prefix("OWNERSHIP_CHANGED: ") {
            return Err(ProviderError::OwnershipChanged(reason.to_owned()));
        }
        return Err(ProviderError::Command(if detail.is_empty() {
            format!("PowerShell exited with {status}")
        } else {
            detail
        }));
    }

    serde_json::from_slice(&stdout).map_err(|error| {
        ProviderError::InvalidResponse(format!("PowerShell JSON decode failed: {error}"))
    })
}

#[cfg(target_os = "windows")]
fn wait_for_bounded_provider_child(
    mut child: std::process::Child,
    job: ProviderProcessJob,
    input: Vec<u8>,
) -> Result<(std::process::ExitStatus, Vec<u8>, Vec<u8>), ProviderError> {
    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| ProviderError::Command("PowerShell stdin was not available".to_owned()))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| ProviderError::Command("PowerShell stdout was not available".to_owned()))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| ProviderError::Command("PowerShell stderr was not available".to_owned()))?;
    let stdin_writer = thread::spawn(move || {
        stdin
            .write_all(&input)
            .map_err(|_| ProviderError::Command("PowerShell stdin write failed".to_owned()))
    });
    let stdout_reader = thread::spawn(move || read_provider_pipe(stdout));
    let stderr_reader = thread::spawn(move || read_provider_pipe(stderr));
    let deadline = Instant::now() + PROVIDER_COMMAND_TIMEOUT;
    let status = loop {
        if let Some(status) = child
            .try_wait()
            .map_err(|_| ProviderError::Command("PowerShell wait failed".to_owned()))?
        {
            break status;
        }
        if Instant::now() >= deadline {
            job.terminate();
            let _ = child.kill();
            let _ = child.wait();
            let _ = stdin_writer.join();
            let _ = stdout_reader.join();
            let _ = stderr_reader.join();
            return Err(ProviderError::Timeout(
                "PowerShell provider command timed out".to_owned(),
            ));
        }
        thread::sleep(Duration::from_millis(20));
    };
    job.terminate();
    stdin_writer
        .join()
        .map_err(|_| ProviderError::Command("PowerShell stdin writer failed".to_owned()))??;
    let stdout = stdout_reader
        .join()
        .map_err(|_| ProviderError::Command("PowerShell stdout reader failed".to_owned()))??;
    let stderr = stderr_reader
        .join()
        .map_err(|_| ProviderError::Command("PowerShell stderr reader failed".to_owned()))??;
    Ok((status, stdout, stderr))
}

#[cfg(target_os = "windows")]
fn read_provider_pipe(mut pipe: impl Read) -> Result<Vec<u8>, ProviderError> {
    let mut bytes = Vec::new();
    let mut buffer = [0_u8; 8192];
    loop {
        let read = pipe
            .read(&mut buffer)
            .map_err(|_| ProviderError::Command("PowerShell pipe read failed".to_owned()))?;
        if read == 0 {
            break;
        }
        if bytes.len().saturating_add(read) > PROVIDER_OUTPUT_LIMIT {
            return Err(ProviderError::OutputLimit(
                "PowerShell provider output exceeded the limit".to_owned(),
            ));
        }
        bytes.extend_from_slice(&buffer[..read]);
    }
    Ok(bytes)
}

#[cfg(target_os = "windows")]
struct ProviderProcessJob(*mut core::ffi::c_void);

#[cfg(target_os = "windows")]
impl ProviderProcessJob {
    fn assign(child: &mut std::process::Child) -> Result<Self, ProviderError> {
        let handle = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
        if handle.is_null() {
            let _ = child.kill();
            let _ = child.wait();
            return Err(ProviderError::Command(
                "PowerShell process job creation failed".to_owned(),
            ));
        }
        let job = Self(handle);
        if unsafe { AssignProcessToJobObject(job.0, child.as_raw_handle()) } == 0 {
            let _ = child.kill();
            let _ = child.wait();
            return Err(ProviderError::Command(
                "PowerShell process job assignment failed".to_owned(),
            ));
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
impl Drop for ProviderProcessJob {
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
fn script_for(action: &HyperVAction) -> &'static str {
    match action {
        HyperVAction::Probe => PROBE_SCRIPT,
        HyperVAction::InspectVhd { .. } => INSPECT_VHD_SCRIPT,
        HyperVAction::CreateDifferencingVhd { .. } => CREATE_DIFFERENCING_VHD_SCRIPT,
        HyperVAction::CreateVm { .. } => CREATE_VM_SCRIPT,
        HyperVAction::ClaimVm { .. } => CLAIM_VM_SCRIPT,
        HyperVAction::ConfigureVm { .. } => CONFIGURE_VM_SCRIPT,
        HyperVAction::InspectVm { .. } => INSPECT_VM_SCRIPT,
        HyperVAction::StartVm { .. } => START_VM_SCRIPT,
        HyperVAction::StopVm { .. } => STOP_VM_SCRIPT,
        HyperVAction::RemoveVm { .. } => REMOVE_VM_SCRIPT,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scripts_do_not_contain_host_global_mutation() {
        let scripts = [
            PROBE_SCRIPT,
            INSPECT_VHD_SCRIPT,
            CREATE_DIFFERENCING_VHD_SCRIPT,
            CREATE_VM_SCRIPT,
            CLAIM_VM_SCRIPT,
            CONFIGURE_VM_SCRIPT,
            INSPECT_VM_SCRIPT,
            START_VM_SCRIPT,
            STOP_VM_SCRIPT,
            REMOVE_VM_SCRIPT,
        ];
        let forbidden = [
            "Enable-WindowsOptionalFeature",
            "Disable-WindowsOptionalFeature",
            "Enable-WindowsFeature",
            "New-VMSwitch",
            "Set-VMSwitch",
            "Remove-VMSwitch",
            "Restart-Computer",
        ];

        for script in scripts {
            for token in forbidden {
                assert!(
                    !script.contains(token),
                    "forbidden PowerShell token: {token}"
                );
            }
        }
    }

    #[test]
    fn create_vm_returns_identity_before_configuration_actions() {
        for forbidden in [
            "Remove-VMNetworkAdapter",
            "Set-VMProcessor",
            "AutomaticCheckpointsEnabled",
            "-Notes",
        ] {
            assert!(
                !CREATE_VM_SCRIPT.contains(forbidden),
                "create_vm.ps1 must not contain deferred configuration token: {forbidden}"
            );
        }
    }

    #[test]
    fn privileged_vm_verbs_recheck_complete_expected_envelope() {
        for (script, verb) in [
            (START_VM_SCRIPT, "Start-VM"),
            (STOP_VM_SCRIPT, "Stop-VM"),
            (REMOVE_VM_SCRIPT, "Remove-VM"),
        ] {
            let verb_position = script.find(verb).expect("provider verb is present");
            for token in [
                "Expected.id",
                "Expected.name",
                "Expected.ownership_marker",
                "Expected.power_state",
                "Expected.configuration_path",
                "Expected.attached_disks",
                "Expected.network_adapter_count",
                "Expected.cpu_count",
                "Expected.memory_mib",
            ] {
                let token_position = script
                    .find(token)
                    .unwrap_or_else(|| panic!("missing ownership precondition token: {token}"));
                assert!(
                    token_position < verb_position,
                    "ownership precondition {token} must precede {verb}"
                );
            }
            assert!(
                script.matches("Assert-ExpectedVm $expected").count() >= 2,
                "{verb} must use a fresh second ownership snapshot immediately before mutation"
            );
        }
    }

    #[test]
    fn every_mutating_script_uses_the_cross_process_provider_mutex() {
        for script in [
            CREATE_DIFFERENCING_VHD_SCRIPT,
            CREATE_VM_SCRIPT,
            CLAIM_VM_SCRIPT,
            CONFIGURE_VM_SCRIPT,
            START_VM_SCRIPT,
            STOP_VM_SCRIPT,
            REMOVE_VM_SCRIPT,
        ] {
            assert!(script.contains("Global\\vmcell-hyperv-provider-v1"));
            assert!(script.contains("WaitOne(0)"));
            assert!(script.contains("AbandonedMutexException"));
            assert!(script.contains("ReleaseMutex"));
        }
    }
}
