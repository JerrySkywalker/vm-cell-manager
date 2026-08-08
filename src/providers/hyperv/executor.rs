use std::io::Write;
use std::process::{Command, Stdio};

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

    child
        .stdin
        .take()
        .ok_or_else(|| ProviderError::Command("PowerShell stdin was not available".to_owned()))?
        .write_all(&input)
        .map_err(|error| ProviderError::Command(format!("failed to write request: {error}")))?;

    let output = child.wait_with_output().map_err(|error| {
        ProviderError::Command(format!("failed to wait for PowerShell: {error}"))
    })?;
    if !output.status.success() {
        let detail = String::from_utf8_lossy(&output.stderr).trim().to_owned();
        if let Some(reason) = detail.strip_prefix("OWNERSHIP_CHANGED: ") {
            return Err(ProviderError::OwnershipChanged(reason.to_owned()));
        }
        return Err(ProviderError::Command(if detail.is_empty() {
            format!("PowerShell exited with {}", output.status)
        } else {
            detail
        }));
    }

    serde_json::from_slice(&output.stdout).map_err(|error| {
        ProviderError::InvalidResponse(format!("PowerShell JSON decode failed: {error}"))
    })
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
        }
    }
}
