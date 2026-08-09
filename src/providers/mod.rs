pub mod hyperv;
pub mod qemu;

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

use crate::core::capability::ProviderCapabilities;
use crate::core::cell::{CellId, CellPhase, CellRecord};
use crate::state::{CellRuntimeGuard, InstallationAuthority, MutationGuard};

/// Unforgeable authority for one bounded provider mutation.
///
/// The public provider contract deliberately requires this token so library
/// callers cannot bypass `CellEngine` ownership, installation, and physical
/// runtime checks. Only the engine can construct it, and its borrows keep the
/// installation and runtime identities pinned for the duration of the verb.
pub struct ProviderMutationAuthority<'a> {
    install_id: Uuid,
    cell_id: CellId,
    provider_name: &'a str,
    provider_id: Option<&'a str>,
    provider_marker: &'a str,
    configuration_path: &'a std::path::Path,
    overlay_path: &'a std::path::Path,
    parent_image_path: &'a std::path::Path,
    phase: CellPhase,
    cpu_count: u16,
    memory_mib: u64,
    _installation: &'a InstallationAuthority,
    _runtime: &'a CellRuntimeGuard,
    _mutation: &'a MutationGuard,
}

impl<'a> ProviderMutationAuthority<'a> {
    pub(crate) fn new(
        record: &'a CellRecord,
        installation: &'a InstallationAuthority,
        runtime: &'a CellRuntimeGuard,
        mutation: &'a MutationGuard,
    ) -> Self {
        Self {
            install_id: record.ownership.install_id,
            cell_id: record.id,
            provider_name: &record.provider,
            provider_id: record
                .provider_object
                .as_ref()
                .map(|identity| identity.id.as_str()),
            provider_marker: &record.ownership.provider_marker,
            configuration_path: &record.ownership.configuration_path,
            overlay_path: &record.ownership.overlay_path,
            parent_image_path: &record.image.path,
            phase: record.phase,
            cpu_count: record.spec.cpu_count,
            memory_mib: record.spec.memory_mib,
            _installation: installation,
            _runtime: runtime,
            _mutation: mutation,
        }
    }

    fn validate_common(&self) -> Result<(), ProviderError> {
        if self.provider_name != "hyperv"
            || self.install_id != self._installation.record().install_id
            || self.cell_id != self._runtime.cell_id()
            || !provider_paths_equal(self.configuration_path, self._runtime.configuration_path())
            || !provider_paths_equal(self.overlay_path, self._runtime.overlay_path())
        {
            return Err(ProviderError::Authority(
                "provider mutation authority no longer matches installation/runtime identity"
                    .to_owned(),
            ));
        }
        Ok(())
    }

    pub(crate) fn validate_overlay_request(
        &self,
        request: &CreateOverlayRequest,
    ) -> Result<(), ProviderError> {
        self.validate_common()?;
        if !provider_paths_equal(&request.parent_path, self.parent_image_path)
            || !provider_paths_equal(&request.overlay_path, self.overlay_path)
        {
            return Err(ProviderError::Authority(
                "overlay request is outside the authorized CellId runtime".to_owned(),
            ));
        }
        Ok(())
    }

    pub(crate) fn validate_create_request(
        &self,
        request: &CreateVmRequest,
    ) -> Result<(), ProviderError> {
        self.validate_common()?;
        if request.name != format!("vmcell-{}", self.cell_id)
            || !provider_paths_equal(&request.configuration_path, self.configuration_path)
            || !provider_paths_equal(&request.overlay_path, self.overlay_path)
            || request.memory_mib != self.memory_mib
        {
            return Err(ProviderError::Authority(
                "VM create request does not match the authorized CellId".to_owned(),
            ));
        }
        Ok(())
    }

    pub(crate) fn validate_vm(&self, vm: &ProviderVm) -> Result<(), ProviderError> {
        self.validate_common()?;
        if vm.name != format!("vmcell-{}", self.cell_id)
            || self.provider_id != Some(vm.id.as_str())
            || vm.ownership_marker != self.provider_marker
            || !provider_paths_equal(&vm.configuration_path, self.configuration_path)
            || vm.attached_disks.len() != 1
            || !provider_paths_equal(&vm.attached_disks[0], self.overlay_path)
            || (matches!(self.phase, CellPhase::Ready | CellPhase::Destroying)
                && (vm.network_adapter_count != 0
                    || vm.cpu_count != self.cpu_count
                    || vm.memory_mib != self.memory_mib))
        {
            return Err(ProviderError::Authority(
                "provider VM snapshot does not match the authorized owned cell".to_owned(),
            ));
        }
        Ok(())
    }

    pub(crate) fn validate_claim_request(
        &self,
        request: &ClaimVmRequest,
    ) -> Result<(), ProviderError> {
        self.validate_common()?;
        if request.ownership_marker != self.provider_marker
            || self.provider_id != Some(request.expected.id.as_str())
            || request.expected.name != format!("vmcell-{}", self.cell_id)
            || !provider_paths_equal(
                &request.expected.configuration_path,
                self.configuration_path,
            )
            || request.expected.attached_disks.len() != 1
            || !provider_paths_equal(&request.expected.attached_disks[0], self.overlay_path)
            || request.expected.memory_mib != self.memory_mib
            || (!request.expected.ownership_marker.is_empty()
                && request.expected.ownership_marker != self.provider_marker)
        {
            return Err(ProviderError::Authority(
                "VM claim request does not match the authorized creation receipt".to_owned(),
            ));
        }
        Ok(())
    }

    pub(crate) fn validate_configure_request(
        &self,
        request: &ConfigureVmRequest,
    ) -> Result<(), ProviderError> {
        self.validate_common()?;
        if request.cpu_count != self.cpu_count
            || self.provider_id != Some(request.expected.id.as_str())
            || request.expected.name != format!("vmcell-{}", self.cell_id)
            || request.expected.ownership_marker != self.provider_marker
            || !provider_paths_equal(
                &request.expected.configuration_path,
                self.configuration_path,
            )
            || request.expected.attached_disks.len() != 1
            || !provider_paths_equal(&request.expected.attached_disks[0], self.overlay_path)
            || request.expected.memory_mib != self.memory_mib
            || request.expected.power_state != ProviderPowerState::Off
            || request.expected.network_adapter_count > 1
        {
            return Err(ProviderError::Authority(
                "VM configure request does not match the authorized cell specification".to_owned(),
            ));
        }
        Ok(())
    }
}

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
        _authority: &ProviderMutationAuthority<'_>,
        _request: &CreateOverlayRequest,
    ) -> Result<ProviderImageInfo, ProviderError> {
        Err(ProviderError::Unsupported {
            provider: self.name(),
            operation: "create_overlay",
        })
    }

    fn create_vm(
        &self,
        _authority: &ProviderMutationAuthority<'_>,
        _request: &CreateVmRequest,
    ) -> Result<ProviderVmIdentity, ProviderError> {
        Err(ProviderError::Unsupported {
            provider: self.name(),
            operation: "create_vm",
        })
    }

    fn claim_vm(
        &self,
        _authority: &ProviderMutationAuthority<'_>,
        _request: &ClaimVmRequest,
    ) -> Result<ProviderVm, ProviderError> {
        Err(ProviderError::Unsupported {
            provider: self.name(),
            operation: "claim_vm",
        })
    }

    fn configure_vm(
        &self,
        _authority: &ProviderMutationAuthority<'_>,
        _request: &ConfigureVmRequest,
    ) -> Result<ProviderVm, ProviderError> {
        Err(ProviderError::Unsupported {
            provider: self.name(),
            operation: "configure_vm",
        })
    }

    fn inspect_vm(&self, _lookup: &VmLookup) -> Result<Option<ProviderVm>, ProviderError> {
        Err(ProviderError::Unsupported {
            provider: self.name(),
            operation: "inspect_vm",
        })
    }

    fn start_vm(
        &self,
        _authority: &ProviderMutationAuthority<'_>,
        _expected: &ProviderVm,
    ) -> Result<(), ProviderError> {
        Err(ProviderError::Unsupported {
            provider: self.name(),
            operation: "start_vm",
        })
    }

    fn stop_vm(
        &self,
        _authority: &ProviderMutationAuthority<'_>,
        _expected: &ProviderVm,
    ) -> Result<(), ProviderError> {
        Err(ProviderError::Unsupported {
            provider: self.name(),
            operation: "stop_vm",
        })
    }

    fn remove_vm(
        &self,
        _authority: &ProviderMutationAuthority<'_>,
        _expected: &ProviderVm,
    ) -> Result<(), ProviderError> {
        Err(ProviderError::Unsupported {
            provider: self.name(),
            operation: "remove_vm",
        })
    }
}

fn provider_paths_equal(left: &std::path::Path, right: &std::path::Path) -> bool {
    let normalize = |path: &std::path::Path| {
        let mut value = path.to_string_lossy().replace('/', "\\");
        if value
            .get(..8)
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case(r"\\?\UNC\"))
        {
            value = format!(r"\\{}", &value[8..]);
        } else if value
            .get(..4)
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case(r"\\?\"))
        {
            value = value[4..].to_owned();
        }
        while value.len() > 3 && value.ends_with('\\') {
            value.pop();
        }
        value
    };
    normalize(left).eq_ignore_ascii_case(&normalize(right))
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
    pub memory_mib: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClaimVmRequest {
    pub expected: ProviderVm,
    pub ownership_marker: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConfigureVmRequest {
    pub expected: ProviderVm,
    pub cpu_count: u16,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderVmIdentity {
    pub id: String,
    pub name: String,
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

    #[error("provider ownership precondition changed: {0}")]
    OwnershipChanged(String),

    #[error("provider mutation authority rejected: {0}")]
    Authority(String),
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

#[cfg(test)]
pub(crate) fn test_mutation_fixture() -> (
    tempfile::TempDir,
    crate::state::StateStore,
    crate::state::InstallationAuthority,
    crate::state::CellRuntimeGuard,
    crate::core::cell::CellRecord,
) {
    use chrono::Utc;

    use crate::core::cell::{CELL_SCHEMA_VERSION, CellPhase, CellSpec, CellState};
    use crate::core::image::{ImageBinding, ImageId};
    use crate::core::ownership::CellOwnership;
    use crate::state::StateStore;

    let directory = tempfile::tempdir().unwrap();
    let state = StateStore::new(directory.path().join("state"));
    let installation = state.installation().unwrap();
    let installation_authority = state.acquire_installation_authority().unwrap();
    let cell_id = CellId::new();
    let runtime = state.prepare_cell_runtime(cell_id).unwrap();
    let image_id = ImageId::parse("test-image").unwrap();
    let ownership = CellOwnership::new(
        installation.install_id,
        cell_id,
        Uuid::new_v4(),
        state.cell_configuration_path(cell_id),
        state.cell_overlay_path(cell_id),
    );
    let record = crate::core::cell::CellRecord {
        schema_version: CELL_SCHEMA_VERSION,
        id: cell_id,
        provider: "hyperv".to_owned(),
        spec: CellSpec {
            image: image_id.clone(),
            provider: Some("hyperv".to_owned()),
            cpu_count: 2,
            memory_mib: 4096,
            ttl_seconds: None,
        },
        image: ImageBinding {
            image_id,
            guest_os: Some(crate::core::image::GuestOs::Windows),
            provider: "hyperv".to_owned(),
            disk_format: "vhdx".to_owned(),
            path: directory.path().join("base.vhdx"),
            sha256: "test".to_owned(),
            file_size: 1,
        },
        ownership,
        provider_object: None,
        state: CellState::Creating,
        phase: CellPhase::IntentRecorded,
        created_at: Utc::now(),
        updated_at: Utc::now(),
        expires_at: None,
        last_error: None,
    };
    (directory, state, installation_authority, runtime, record)
}
