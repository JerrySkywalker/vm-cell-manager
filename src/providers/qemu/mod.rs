use std::process::Command;

use crate::core::capability::ProviderCapabilities;
use crate::providers::{LocalVmProvider, ProviderProbe};

pub struct QemuProvider;

impl LocalVmProvider for QemuProvider {
    fn name(&self) -> &'static str {
        "qemu"
    }

    fn probe(&self) -> ProviderProbe {
        probe_qemu()
    }
}

fn probe_qemu() -> ProviderProbe {
    let binary = match std::env::consts::ARCH {
        "aarch64" => "qemu-system-aarch64",
        _ => "qemu-system-x86_64",
    };

    let version = Command::new(binary).arg("--version").output();
    let output = match version {
        Ok(output) if output.status.success() => output,
        Ok(output) => {
            return ProviderProbe {
                name: "qemu",
                available: false,
                detail: format!("{binary} returned {}", output.status),
                capabilities: ProviderCapabilities::unavailable(),
            };
        }
        Err(error) => {
            return ProviderProbe {
                name: "qemu",
                available: false,
                detail: format!("{binary} not found or not runnable: {error}"),
                capabilities: ProviderCapabilities::unavailable(),
            };
        }
    };

    let version_line = String::from_utf8_lossy(&output.stdout)
        .lines()
        .next()
        .unwrap_or("QEMU detected")
        .to_owned();

    let accel_output = Command::new(binary).args(["-accel", "help"]).output();
    let accel_text = accel_output
        .ok()
        .map(|value| String::from_utf8_lossy(&value.stdout).into_owned())
        .unwrap_or_default();

    let wanted = match std::env::consts::OS {
        "linux" => "kvm",
        "macos" => "hvf",
        "windows" => "whpx",
        _ => "",
    };

    let detected_accelerators =
        if !wanted.is_empty() && accel_text.lines().any(|line| line.trim() == wanted) {
            vec![wanted.to_owned()]
        } else {
            Vec::new()
        };

    ProviderProbe {
        name: "qemu",
        available: true,
        detail: version_line,
        capabilities: ProviderCapabilities {
            full_system_vm: true,
            cow_overlay: true,
            hardware_acceleration: !detected_accelerators.is_empty(),
            accelerators: detected_accelerators,
            guest_os: vec!["windows".to_owned(), "linux".to_owned()],
            guest_arch: vec![std::env::consts::ARCH.to_owned()],
            guest_transports: vec!["qga".to_owned(), "ssh".to_owned()],
            networkless_guest_exec: true,
        },
    }
}
