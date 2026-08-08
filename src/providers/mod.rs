pub mod hyperv;
pub mod qemu;

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::core::capability::ProviderCapabilities;

pub trait LocalVmProvider: Send + Sync {
    fn name(&self) -> &'static str;
    fn probe(&self) -> ProviderProbe;

    fn inspect_image(&self, _path: PathBuf) -> Result<ProviderImageInfo, ProviderError> {
        Err(ProviderError::Unsupported {
            provider: self.name(),
            operation: "inspect_image",
        })
    }

    fn create_overlay(
        &self,
        _request: &CreateOverlayRequest,
    ) -> Result<ProviderImageInfo, ProviderError> {
        Err(ProviderError::Unsupported {
            provider: self.name(),
            operation: "create_overlay",
        })
    }

    fn create_vm(&self, _request: &CreateVmRequest) -> Result<ProviderVm, ProviderError> {
        Err(ProviderError::Unsupported {
            provider: self.name(),
            operation: "create_vm",
        })
    }

    fn inspect_vm(&self, _lookup: &VmLookup) -> Result<Option<ProviderVm>, ProviderError> {
        Err(ProviderError::Unsupported {
            provider: self.name(),
            operation: "inspect_vm",
        })
    }

    fn start_vm(&self, _id: &str) -> Result<(), ProviderError> {
        Err(ProviderError::Unsupported {
            provider: self.name(),
            operation: "start_vm",
        })
    }

    fn stop_vm(&self, _id: &str) -> Result<(), ProviderError> {
        Err(ProviderError::Unsupported {
            provider: self.name(),
            operation: "stop_vm",
        })
    }

    fn remove_vm(&self, _id: &str) -> Result<(), ProviderError> {
        Err(ProviderError::Unsupported {
            provider: self.name(),
            operation: "remove_vm",
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderProbe {
    pub name: &'static str,
    pub available: bool,
    pub detail: String,
    pub capabilities: ProviderCapabilities,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderImageInfo {
    pub path: PathBuf,
    pub disk_format: String,
    pub disk_type: String,
    pub parent_path: Option<PathBuf>,
    pub file_size: u64,
    pub virtual_size: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreateOverlayRequest {
    pub parent_path: PathBuf,
    pub overlay_path: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreateVmRequest {
    pub name: String,
    pub configuration_path: PathBuf,
    pub overlay_path: PathBuf,
    pub ownership_marker: String,
    pub cpu_count: u16,
    pub memory_mib: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum VmLookup {
    Id(String),
    Name(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProviderPowerState {
    Off,
    Running,
    Paused,
    Saved,
    Other(String),
}

impl Serialize for ProviderPowerState {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let value = match self {
            Self::Off => "off",
            Self::Running => "running",
            Self::Paused => "paused",
            Self::Saved => "saved",
            Self::Other(value) => value,
        };
        serializer.serialize_str(value)
    }
}

impl<'de> Deserialize<'de> for ProviderPowerState {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Ok(match value.to_ascii_lowercase().as_str() {
            "off" => Self::Off,
            "running" => Self::Running,
            "paused" => Self::Paused,
            "saved" => Self::Saved,
            _ => Self::Other(value),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderVm {
    pub id: String,
    pub name: String,
    pub power_state: ProviderPowerState,
    pub ownership_marker: String,
    pub configuration_path: PathBuf,
    pub attached_disks: Vec<PathBuf>,
    pub network_adapter_count: u32,
    pub cpu_count: u16,
    pub memory_mib: u64,
}

#[derive(Debug, Error)]
pub enum ProviderError {
    #[error("provider {provider} does not support {operation}")]
    Unsupported {
        provider: &'static str,
        operation: &'static str,
    },

    #[error("provider command failed: {0}")]
    Command(String),

    #[error("provider returned invalid data: {0}")]
    InvalidResponse(String),

    #[error("provider object not found: {0}")]
    NotFound(String),

    #[error("provider object collision: {0}")]
    Collision(String),
}

#[must_use]
pub fn builtin_provider_probes() -> Vec<ProviderProbe> {
    let providers: Vec<Box<dyn LocalVmProvider>> = vec![
        Box::new(hyperv::HyperVProvider::system()),
        Box::new(qemu::QemuProvider),
    ];

    providers
        .into_iter()
        .map(|provider| provider.probe())
        .collect()
}
