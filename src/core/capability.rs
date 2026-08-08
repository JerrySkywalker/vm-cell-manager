use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderCapabilities {
    pub full_system_vm: bool,
    pub cow_overlay: bool,
    pub hardware_acceleration: bool,
    pub accelerators: Vec<String>,
    pub guest_os: Vec<String>,
    pub guest_arch: Vec<String>,
    pub guest_transports: Vec<String>,
    pub networkless_guest_exec: bool,
}

impl ProviderCapabilities {
    #[must_use]
    pub fn unavailable() -> Self {
        Self {
            full_system_vm: false,
            cow_overlay: false,
            hardware_acceleration: false,
            accelerators: Vec::new(),
            guest_os: Vec::new(),
            guest_arch: Vec::new(),
            guest_transports: Vec::new(),
            networkless_guest_exec: false,
        }
    }
}
