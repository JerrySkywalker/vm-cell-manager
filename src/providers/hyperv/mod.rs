use std::process::Command;

use crate::core::capability::ProviderCapabilities;
use crate::providers::{LocalVmProvider, ProviderProbe};

pub struct HyperVProvider;

impl LocalVmProvider for HyperVProvider {
    fn name(&self) -> &'static str {
        "hyperv"
    }

    fn probe(&self) -> ProviderProbe {
        probe_hyperv()
    }
}

#[cfg(target_os = "windows")]
fn probe_hyperv() -> ProviderProbe {
    let result = Command::new("powershell.exe")
        .args([
            "-NoLogo",
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            "if (Get-Command Get-VM -ErrorAction SilentlyContinue) { exit 0 } else { exit 1 }",
        ])
        .status();

    match result {
        Ok(status) if status.success() => ProviderProbe {
            name: "hyperv",
            available: true,
            detail: "Hyper-V PowerShell module detected".to_owned(),
            capabilities: ProviderCapabilities {
                full_system_vm: true,
                cow_overlay: true,
                hardware_acceleration: true,
                accelerators: vec!["hyper-v".to_owned()],
                guest_os: vec!["windows".to_owned(), "linux".to_owned()],
                guest_arch: vec![std::env::consts::ARCH.to_owned()],
                guest_transports: vec!["powershell-direct".to_owned(), "ssh".to_owned()],
                networkless_guest_exec: true,
            },
        },
        Ok(status) => ProviderProbe {
            name: "hyperv",
            available: false,
            detail: format!("Hyper-V PowerShell module not detected (exit={status})"),
            capabilities: ProviderCapabilities::unavailable(),
        },
        Err(error) => ProviderProbe {
            name: "hyperv",
            available: false,
            detail: format!("failed to probe Hyper-V: {error}"),
            capabilities: ProviderCapabilities::unavailable(),
        },
    }
}

#[cfg(not(target_os = "windows"))]
fn probe_hyperv() -> ProviderProbe {
    ProviderProbe {
        name: "hyperv",
        available: false,
        detail: "Hyper-V provider is only available on Windows hosts".to_owned(),
        capabilities: ProviderCapabilities::unavailable(),
    }
}
