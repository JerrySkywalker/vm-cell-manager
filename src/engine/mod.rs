use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use chrono::{Duration, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;
use uuid::Uuid;

use crate::core::cell::{CELL_SCHEMA_VERSION, CellId, CellPhase, CellRecord, CellSpec, CellState};
use crate::core::image::{
    Architecture, GuestOs, IMAGE_SCHEMA_VERSION, ImageBinding, ImageId, ImageRecord, ImageVariant,
};
use crate::core::ownership::{CellOwnership, OWNERSHIP_MARKER_SCHEMA, ProviderObjectIdentity};
use crate::providers::{
    ClaimVmRequest, ConfigureVmRequest, CreateOverlayRequest, CreateVmRequest, LocalVmProvider,
    ProviderError, ProviderImageInfo, ProviderMutationAuthority, ProviderPowerState, ProviderVm,
    ProviderVmIdentity, VmLookup,
};
use crate::state::{InstallationAuthority, StateError, StateStore};

const MIN_MEMORY_MIB: u64 = 512;
const MAX_MEMORY_MIB: u64 = 1_048_576;
const MAX_CPU_COUNT: u16 = 64;

pub struct CellEngine<P> {
    state: StateStore,
    provider: P,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegisterImageRequest {
    pub id: ImageId,
    pub guest_os: GuestOs,
    pub guest_arch: Architecture,
    pub path: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OperationReport {
    pub schema_version: u32,
    pub cell_id: CellId,
    pub state: CellState,
    pub changed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CellInspection {
    pub schema_version: u32,
    pub cell: CellRecord,
    pub provider_vm: Option<ProviderVm>,
    pub reconciliation: ReconciliationStatus,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum ReconciliationStatus {
    ExactOwned,
    ManifestOnly,
    ProviderMissing,
    UnprovenProviderObject {
        id: String,
    },
    OwnershipMismatch {
        reasons: Vec<String>,
    },
    StateDrift {
        manifest_state: CellState,
        provider_state: ProviderPowerState,
    },
    Provisioning {
        phase: CellPhase,
    },
    Destroyed,
}

#[derive(Debug, Error)]
pub enum EngineError {
    #[error(transparent)]
    State(#[from] StateError),

    #[error(transparent)]
    Provider(#[from] ProviderError),

    #[error("unsupported M1 provider: {0}")]
    UnsupportedProvider(String),

    #[error("Hyper-V provider is unavailable: {0}")]
    ProviderUnavailable(String),

    #[error("invalid image: {0}")]
    InvalidImage(String),

    #[error("image id is already registered with different identity: {0}")]
    ImageConflict(ImageId),

    #[error("invalid cell request: {0}")]
    InvalidCellRequest(String),

    #[error("ownership is not proven: {0}")]
    OwnershipNotProven(String),

    #[error("provider object drift: {0}")]
    ProviderDrift(String),

    #[error("unexpected provider power state: {0:?}")]
    UnexpectedPowerState(ProviderPowerState),
}

impl<P: LocalVmProvider> CellEngine<P> {
    #[must_use]
    pub fn new(state: StateStore, provider: P) -> Self {
        Self { state, provider }
    }

    #[must_use]
    pub fn state(&self) -> &StateStore {
        &self.state
    }

    pub fn register_image(
        &self,
        request: RegisterImageRequest,
    ) -> Result<ImageRecord, EngineError> {
        self.require_hyperv_available()?;
        let _guard = self.state.acquire_mutation_lock()?;
        let canonical = canonical_vhdx_path(&request.path)?;
        let mut handle = open_immutable_parent(&canonical)?;
        let file_size = handle
            .metadata()
            .map_err(|error| EngineError::InvalidImage(error.to_string()))?
            .len();
        let provider_info = self.provider.inspect_image(canonical.clone())?;
        validate_base_vhdx(&canonical, file_size, &provider_info)?;
        let sha256 = sha256_file(&mut handle)?;

        let variant = ImageVariant {
            provider: "hyperv".to_owned(),
            disk_format: "vhdx".to_owned(),
            path: canonical,
            sha256,
            file_size,
        };
        let record = ImageRecord {
            schema_version: IMAGE_SCHEMA_VERSION,
            id: request.id.clone(),
            guest_os: request.guest_os,
            guest_arch: request.guest_arch,
            variants: vec![variant],
            registered_at: Utc::now(),
        };

        match self.state.load_image(&request.id) {
            Ok(existing) if same_image_identity(&existing, &record) => Ok(existing),
            Ok(_) => Err(EngineError::ImageConflict(request.id)),
            Err(StateError::NotFound(_)) => {
                self.state.save_image_new(&record)?;
                Ok(record)
            }
            Err(error) => Err(error.into()),
        }
    }

    pub fn list_images(&self) -> Result<Vec<ImageRecord>, EngineError> {
        Ok(self.state.list_images()?)
    }

    pub fn inspect_image(&self, image_id: &ImageId) -> Result<ImageRecord, EngineError> {
        Ok(self.state.load_image(image_id)?)
    }

    pub fn list_cells(&self) -> Result<Vec<CellRecord>, EngineError> {
        Ok(self.state.list_cells()?)
    }

    pub fn reconcile_all(&self) -> Result<Vec<CellInspection>, EngineError> {
        self.state
            .list_cells()?
            .into_iter()
            .map(|record| self.inspect_cell(record.id))
            .collect()
    }

    pub fn create_cell(&self, spec: CellSpec) -> Result<CellRecord, EngineError> {
        self.require_hyperv_available()?;
        validate_cell_spec(&spec)?;
        let _guard = self.state.acquire_mutation_lock()?;
        let image_record = self.state.load_image(&spec.image)?;
        let variant = hyperv_variant(&image_record)?;
        let parent_handle = self.verify_registered_image(variant)?;

        let installation = self.state.installation()?;
        let installation_authority = self.state.acquire_installation_authority()?;
        if installation != *installation_authority.record() {
            return Err(EngineError::OwnershipNotProven(
                "installation identity changed while creating a cell".to_owned(),
            ));
        }
        let cell_id = CellId::new();
        let now = Utc::now();
        let ownership = CellOwnership::new(
            installation.install_id,
            cell_id,
            Uuid::new_v4(),
            self.state.cell_configuration_path(cell_id),
            self.state.cell_overlay_path(cell_id),
        );
        let expires_at = spec
            .ttl_seconds
            .map(|seconds| now + Duration::seconds(i64::try_from(seconds).unwrap_or(i64::MAX)));
        let mut record = CellRecord {
            schema_version: CELL_SCHEMA_VERSION,
            id: cell_id,
            provider: "hyperv".to_owned(),
            spec,
            image: ImageBinding::from_variant(image_record.id.clone(), variant),
            ownership,
            provider_object: None,
            state: CellState::Creating,
            phase: CellPhase::IntentRecorded,
            created_at: now,
            updated_at: now,
            expires_at,
            last_error: None,
        };
        self.state.save_cell(&record)?;
        let runtime_authority = match self.state.prepare_cell_runtime(cell_id) {
            Ok(runtime) => runtime,
            Err(error) => return self.fail_record(record, error.into()),
        };

        if let Some(existing) = self.provider.inspect_vm(&VmLookup::Name(
            record.ownership.provider_object_name.clone(),
        ))? {
            return self.fail_record(
                record,
                EngineError::OwnershipNotProven(format!(
                    "VM name collision with provider id {}",
                    existing.id
                )),
            );
        }

        let overlay_request = CreateOverlayRequest {
            parent_path: record.image.path.clone(),
            overlay_path: record.ownership.overlay_path.clone(),
        };
        let authority =
            ProviderMutationAuthority::new(&record, &installation_authority, &runtime_authority);
        let overlay = match self.provider.create_overlay(&authority, &overlay_request) {
            Ok(overlay) => overlay,
            Err(error) => return self.fail_record(record, error.into()),
        };
        if let Err(error) = validate_overlay(&record, &overlay) {
            return self.fail_record(record, error);
        }
        record.phase = CellPhase::OverlayCreated;
        record.updated_at = Utc::now();
        self.state.save_cell(&record)?;
        drop(parent_handle);

        let create_request = CreateVmRequest {
            name: record.ownership.provider_object_name.clone(),
            configuration_path: record.ownership.configuration_path.clone(),
            overlay_path: record.ownership.overlay_path.clone(),
            memory_mib: record.spec.memory_mib,
        };
        let authority =
            ProviderMutationAuthority::new(&record, &installation_authority, &runtime_authority);
        let provider_identity = match self.provider.create_vm(&authority, &create_request) {
            Ok(provider_identity) => provider_identity,
            Err(error) => return self.fail_record(record, error.into()),
        };
        let provider_identity = match normalize_provider_identity(&record, provider_identity) {
            Ok(identity) => identity,
            Err(error) => return self.fail_record(record, error),
        };
        record.provider_object = Some(ProviderObjectIdentity {
            id: provider_identity.id.clone(),
            name: provider_identity.name.clone(),
        });
        record.phase = CellPhase::ProviderObjectCreated;
        record.updated_at = Utc::now();
        self.state.save_cell(&record)?;

        let provider_vm = match self
            .provider
            .inspect_vm(&VmLookup::Id(provider_identity.id.clone()))?
        {
            Some(provider_vm) => provider_vm,
            None => {
                return self.fail_record(
                    record,
                    EngineError::ProviderDrift(
                        "new provider object disappeared after its id was persisted".to_owned(),
                    ),
                );
            }
        };

        if let Err(error) = prove_creation_identity(&record, &provider_vm, false) {
            return self.fail_record(record, error);
        }
        if provider_vm.power_state != ProviderPowerState::Off {
            return self.fail_record(
                record,
                EngineError::UnexpectedPowerState(provider_vm.power_state),
            );
        }

        let claim_request = ClaimVmRequest {
            expected: provider_vm,
            ownership_marker: record.ownership.provider_marker.clone(),
        };
        let authority =
            ProviderMutationAuthority::new(&record, &installation_authority, &runtime_authority);
        let claimed_vm = match self.provider.claim_vm(&authority, &claim_request) {
            Ok(provider_vm) => provider_vm,
            Err(error) => return self.fail_record(record, error.into()),
        };
        if let Err(error) = prove_creation_identity(&record, &claimed_vm, true) {
            return self.fail_record(record, error);
        }
        record.phase = CellPhase::ProviderObjectClaimed;
        record.updated_at = Utc::now();
        self.state.save_cell(&record)?;

        let configure_request = ConfigureVmRequest {
            expected: claimed_vm,
            cpu_count: record.spec.cpu_count,
        };
        let authority =
            ProviderMutationAuthority::new(&record, &installation_authority, &runtime_authority);
        let provider_vm = match self.provider.configure_vm(&authority, &configure_request) {
            Ok(provider_vm) => provider_vm,
            Err(error) => return self.fail_record(record, error.into()),
        };
        if let Err(error) = prove_ownership(&record, &provider_vm) {
            return self.fail_record(record, error);
        }

        record.state = CellState::Stopped;
        record.phase = CellPhase::Ready;
        record.updated_at = Utc::now();
        self.state.save_cell(&record)?;
        Ok(record)
    }

    pub fn inspect_cell(&self, cell_id: CellId) -> Result<CellInspection, EngineError> {
        let record = self.state.load_cell(cell_id)?;
        let (provider_vm, reconciliation) = self.reconcile_record(&record)?;
        Ok(CellInspection {
            schema_version: 1,
            cell: record,
            provider_vm,
            reconciliation,
        })
    }

    pub fn reconcile_cell(&self, cell_id: CellId) -> Result<CellInspection, EngineError> {
        self.inspect_cell(cell_id)
    }

    pub fn start_cell(&self, cell_id: CellId) -> Result<OperationReport, EngineError> {
        self.require_hyperv()?;
        let _guard = self.state.acquire_mutation_lock()?;
        let mut record = self.state.load_cell(cell_id)?;
        require_lifecycle_state(&record, "start", &[CellState::Stopped, CellState::Running])?;
        let before = self.exact_owned_vm(&record)?;
        match before.power_state {
            ProviderPowerState::Running => {
                let changed = record.state != CellState::Running;
                record.state = CellState::Running;
                record.phase = CellPhase::Ready;
                record.updated_at = Utc::now();
                self.state.save_cell(&record)?;
                Ok(operation_report(&record, changed))
            }
            ProviderPowerState::Off if record.state != CellState::Destroyed => {
                let installation = self.state.acquire_installation_authority()?;
                self.validate_local_ownership_against(&record, &installation)?;
                let runtime = self.state.pin_cell_runtime(record.id)?;
                let authority = ProviderMutationAuthority::new(&record, &installation, &runtime);
                self.provider.start_vm(&authority, &before)?;
                let after = self.exact_owned_vm(&record)?;
                if after.power_state != ProviderPowerState::Running {
                    return Err(EngineError::UnexpectedPowerState(after.power_state));
                }
                record.state = CellState::Running;
                record.phase = CellPhase::Ready;
                record.updated_at = Utc::now();
                self.state.save_cell(&record)?;
                Ok(operation_report(&record, true))
            }
            state => Err(EngineError::UnexpectedPowerState(state)),
        }
    }

    pub fn stop_cell(&self, cell_id: CellId) -> Result<OperationReport, EngineError> {
        self.require_hyperv()?;
        let _guard = self.state.acquire_mutation_lock()?;
        let mut record = self.state.load_cell(cell_id)?;
        require_lifecycle_state(&record, "stop", &[CellState::Stopped, CellState::Running])?;
        let before = self.exact_owned_vm(&record)?;
        match before.power_state {
            ProviderPowerState::Off => {
                let changed = record.state != CellState::Stopped;
                record.state = CellState::Stopped;
                record.phase = CellPhase::Ready;
                record.updated_at = Utc::now();
                self.state.save_cell(&record)?;
                Ok(operation_report(&record, changed))
            }
            ProviderPowerState::Running => {
                let installation = self.state.acquire_installation_authority()?;
                self.validate_local_ownership_against(&record, &installation)?;
                let runtime = self.state.pin_cell_runtime(record.id)?;
                let authority = ProviderMutationAuthority::new(&record, &installation, &runtime);
                self.provider.stop_vm(&authority, &before)?;
                let after = self.exact_owned_vm(&record)?;
                if after.power_state != ProviderPowerState::Off {
                    return Err(EngineError::UnexpectedPowerState(after.power_state));
                }
                record.state = CellState::Stopped;
                record.phase = CellPhase::Ready;
                record.updated_at = Utc::now();
                self.state.save_cell(&record)?;
                Ok(operation_report(&record, true))
            }
            state => Err(EngineError::UnexpectedPowerState(state)),
        }
    }

    pub fn destroy_cell(&self, cell_id: CellId) -> Result<OperationReport, EngineError> {
        self.require_hyperv()?;
        let _guard = self.state.acquire_mutation_lock()?;
        let mut record = self.state.load_cell(cell_id)?;
        if record.state == CellState::Destroyed {
            let (_, reconciliation) = self.reconcile_record(&record)?;
            return if reconciliation == ReconciliationStatus::Destroyed {
                Ok(operation_report(&record, false))
            } else {
                Err(EngineError::ProviderDrift(format!(
                    "destroyed tombstone does not reconcile: {reconciliation:?}"
                )))
            };
        }
        let installation = self.state.acquire_installation_authority()?;
        self.validate_local_ownership_against(&record, &installation)?;

        let provider_vm = if let Some(identity) = &record.provider_object {
            self.provider
                .inspect_vm(&VmLookup::Id(identity.id.clone()))?
        } else {
            let by_name = self.provider.inspect_vm(&VmLookup::Name(
                record.ownership.provider_object_name.clone(),
            ))?;
            if let Some(vm) = by_name {
                return Err(EngineError::OwnershipNotProven(format!(
                    "provider object {} exists but its id was never recorded",
                    vm.id
                )));
            }
            return Err(EngineError::OwnershipNotProven(
                "provider id was never durably recorded; pre-id runtime remains quarantined"
                    .to_owned(),
            ));
        };

        if let Some(mut vm) = provider_vm {
            let runtime = self.state.pin_cell_runtime(record.id)?;
            let destroying_provisioning = match record.phase {
                CellPhase::ProviderObjectCreated => {
                    prove_creation_identity(&record, &vm, false)?;
                    let claim_request = ClaimVmRequest {
                        expected: vm,
                        ownership_marker: record.ownership.provider_marker.clone(),
                    };
                    let authority =
                        ProviderMutationAuthority::new(&record, &installation, &runtime);
                    vm = self.provider.claim_vm(&authority, &claim_request)?;
                    prove_creation_identity(&record, &vm, true)?;
                    record.phase = CellPhase::ProviderObjectClaimed;
                    record.updated_at = Utc::now();
                    self.state.save_cell(&record)?;
                    true
                }
                CellPhase::ProviderObjectClaimed => {
                    prove_creation_identity(&record, &vm, true)?;
                    true
                }
                CellPhase::DestroyingProvisioning => {
                    prove_destroy_identity(&record, &vm)?;
                    true
                }
                CellPhase::Ready | CellPhase::Destroying => {
                    prove_ownership(&record, &vm)?;
                    false
                }
                phase => {
                    return Err(EngineError::OwnershipNotProven(format!(
                        "provider id is incompatible with persisted phase {phase:?}"
                    )));
                }
            };
            record.state = CellState::Destroying;
            record.phase = if destroying_provisioning {
                CellPhase::DestroyingProvisioning
            } else {
                CellPhase::Destroying
            };
            record.updated_at = Utc::now();
            self.state.save_cell(&record)?;

            if vm.power_state != ProviderPowerState::Off {
                self.validate_local_ownership_against(&record, &installation)?;
                let authority = ProviderMutationAuthority::new(&record, &installation, &runtime);
                self.provider.stop_vm(&authority, &vm)?;
                vm = self
                    .provider
                    .inspect_vm(&VmLookup::Id(vm.id.clone()))?
                    .ok_or_else(|| {
                        EngineError::ProviderDrift(
                            "provider object disappeared while stopping".to_owned(),
                        )
                    })?;
                prove_destroy_identity(&record, &vm)?;
                if vm.power_state != ProviderPowerState::Off {
                    return Err(EngineError::UnexpectedPowerState(vm.power_state));
                }
            }
            self.validate_local_ownership_against(&record, &installation)?;
            prove_destroy_identity(&record, &vm)?;
            let authority = ProviderMutationAuthority::new(&record, &installation, &runtime);
            self.provider.remove_vm(&authority, &vm)?;
            if self
                .provider
                .inspect_vm(&VmLookup::Id(vm.id.clone()))?
                .is_some()
            {
                return Err(EngineError::ProviderDrift(
                    "provider object still exists after remove".to_owned(),
                ));
            }
            if self
                .provider
                .inspect_vm(&VmLookup::Name(
                    record.ownership.provider_object_name.clone(),
                ))?
                .is_some()
            {
                return Err(EngineError::OwnershipNotProven(
                    "provider name became occupied after removing the recorded id".to_owned(),
                ));
            }

            self.validate_local_ownership_against(&record, &installation)?;
            self.state.remove_cell_runtime(cell_id, runtime)?;
        } else if record.provider_object.is_some() {
            if self
                .provider
                .inspect_vm(&VmLookup::Name(
                    record.ownership.provider_object_name.clone(),
                ))?
                .is_some()
            {
                return Err(EngineError::OwnershipNotProven(
                    "recorded provider id is absent but the name is occupied".to_owned(),
                ));
            }
            if self.state.runtime_entry_exists(cell_id)? {
                let runtime = self.state.pin_cell_runtime(record.id)?;
                self.validate_local_ownership_against(&record, &installation)?;
                self.state.remove_cell_runtime(cell_id, runtime)?;
            }
        }

        self.validate_local_ownership_against(&record, &installation)?;
        record.state = CellState::Destroyed;
        record.phase = CellPhase::Destroyed;
        record.updated_at = Utc::now();
        self.state.save_cell(&record)?;
        Ok(operation_report(&record, true))
    }

    fn require_hyperv(&self) -> Result<(), EngineError> {
        if self.provider.name() != "hyperv" {
            return Err(EngineError::UnsupportedProvider(
                self.provider.name().to_owned(),
            ));
        }
        Ok(())
    }

    fn require_hyperv_available(&self) -> Result<(), EngineError> {
        self.require_hyperv()?;
        let probe = self.provider.probe();
        if probe.available {
            Ok(())
        } else {
            Err(EngineError::ProviderUnavailable(probe.detail))
        }
    }

    fn verify_registered_image(&self, variant: &ImageVariant) -> Result<File, EngineError> {
        let canonical = canonical_vhdx_path(&variant.path)?;
        if !paths_equal(&canonical, &variant.path) {
            return Err(EngineError::InvalidImage(
                "registered image path no longer resolves to the same file".to_owned(),
            ));
        }
        let mut handle = open_immutable_parent(&canonical)?;
        let file_size = handle
            .metadata()
            .map_err(|error| EngineError::InvalidImage(error.to_string()))?
            .len();
        if file_size != variant.file_size {
            return Err(EngineError::InvalidImage(
                "registered image size changed".to_owned(),
            ));
        }
        let provider_info = self.provider.inspect_image(canonical.clone())?;
        validate_base_vhdx(&canonical, file_size, &provider_info)?;
        let sha256 = sha256_file(&mut handle)?;
        if sha256 != variant.sha256 {
            return Err(EngineError::InvalidImage(
                "registered image SHA-256 changed".to_owned(),
            ));
        }
        Ok(handle)
    }

    fn exact_owned_vm(&self, record: &CellRecord) -> Result<ProviderVm, EngineError> {
        if record.state == CellState::Destroyed {
            return Err(EngineError::OwnershipNotProven(
                "cell is already destroyed".to_owned(),
            ));
        }
        self.validate_local_ownership(record)?;
        let identity = record.provider_object.as_ref().ok_or_else(|| {
            EngineError::OwnershipNotProven("provider object id is not recorded".to_owned())
        })?;
        let vm = self
            .provider
            .inspect_vm(&VmLookup::Id(identity.id.clone()))?
            .ok_or_else(|| {
                EngineError::OwnershipNotProven("recorded provider object is absent".to_owned())
            })?;
        prove_ownership(record, &vm)?;
        Ok(vm)
    }

    fn reconcile_record(
        &self,
        record: &CellRecord,
    ) -> Result<(Option<ProviderVm>, ReconciliationStatus), EngineError> {
        if let Err(error) = self.validate_local_ownership(record) {
            return Ok((
                None,
                ReconciliationStatus::OwnershipMismatch {
                    reasons: vec![error.to_string()],
                },
            ));
        }

        if record.state == CellState::Destroyed {
            if let Some(identity) = &record.provider_object {
                if let Some(vm) = self
                    .provider
                    .inspect_vm(&VmLookup::Id(identity.id.clone()))?
                {
                    return Ok((
                        Some(vm),
                        ReconciliationStatus::OwnershipMismatch {
                            reasons: vec![
                                "provider object exists after the cell was tombstoned".to_owned(),
                            ],
                        },
                    ));
                }
            }
            if let Some(vm) = self.provider.inspect_vm(&VmLookup::Name(
                record.ownership.provider_object_name.clone(),
            ))? {
                return Ok((
                    Some(vm),
                    ReconciliationStatus::OwnershipMismatch {
                        reasons: vec![
                            "provider name is occupied after the cell was tombstoned".to_owned(),
                        ],
                    },
                ));
            }
            if self.state.runtime_entry_exists(record.id)? {
                return Ok((
                    None,
                    ReconciliationStatus::OwnershipMismatch {
                        reasons: vec![
                            "runtime entry exists after the cell was tombstoned".to_owned(),
                        ],
                    },
                ));
            }
            return Ok((None, ReconciliationStatus::Destroyed));
        }

        let Some(identity) = &record.provider_object else {
            let vm = self.provider.inspect_vm(&VmLookup::Name(
                record.ownership.provider_object_name.clone(),
            ))?;
            return Ok(match vm {
                Some(vm) => (
                    Some(vm.clone()),
                    ReconciliationStatus::UnprovenProviderObject { id: vm.id },
                ),
                None => (None, ReconciliationStatus::ManifestOnly),
            });
        };

        let Some(vm) = self
            .provider
            .inspect_vm(&VmLookup::Id(identity.id.clone()))?
        else {
            return Ok((None, ReconciliationStatus::ProviderMissing));
        };
        let reasons = ownership_drift(record, &vm);
        if matches!(
            record.phase,
            CellPhase::ProviderObjectCreated | CellPhase::ProviderObjectClaimed
        ) && prove_creation_identity(
            record,
            &vm,
            record.phase == CellPhase::ProviderObjectClaimed
                || vm.ownership_marker == record.ownership.provider_marker,
        )
        .is_ok()
        {
            return Ok((
                Some(vm),
                ReconciliationStatus::Provisioning {
                    phase: record.phase,
                },
            ));
        }
        if record.phase == CellPhase::DestroyingProvisioning
            && prove_destroy_identity(record, &vm).is_ok()
        {
            return Ok((
                Some(vm),
                ReconciliationStatus::Provisioning {
                    phase: record.phase,
                },
            ));
        }
        if !reasons.is_empty() {
            return Ok((
                Some(vm),
                ReconciliationStatus::OwnershipMismatch { reasons },
            ));
        }
        if !power_state_matches(record.state, &vm.power_state) {
            return Ok((
                Some(vm.clone()),
                ReconciliationStatus::StateDrift {
                    manifest_state: record.state,
                    provider_state: vm.power_state,
                },
            ));
        }
        Ok((Some(vm), ReconciliationStatus::ExactOwned))
    }

    fn validate_local_ownership(&self, record: &CellRecord) -> Result<(), EngineError> {
        let installation = self.state.load_installation()?;
        self.validate_local_ownership_record(record, installation.install_id)
    }

    fn validate_local_ownership_against(
        &self,
        record: &CellRecord,
        installation: &InstallationAuthority,
    ) -> Result<(), EngineError> {
        self.validate_local_ownership_record(record, installation.record().install_id)
    }

    fn validate_local_ownership_record(
        &self,
        record: &CellRecord,
        current_install_id: Uuid,
    ) -> Result<(), EngineError> {
        if record.ownership.install_id != current_install_id {
            return Err(EngineError::OwnershipNotProven(
                "cell installation identity does not match the current state store".to_owned(),
            ));
        }
        if record.provider != "hyperv"
            || record.spec.provider.as_deref() != Some("hyperv")
            || record.image.provider != "hyperv"
        {
            return Err(EngineError::OwnershipNotProven(
                "manifest provider binding is not Hyper-V".to_owned(),
            ));
        }
        if record.spec.image != record.image.image_id {
            return Err(EngineError::OwnershipNotProven(
                "manifest image binding does not match the requested image id".to_owned(),
            ));
        }
        let expected_name = format!("vmcell-{}", record.id.0);
        if record.ownership.provider_object_name != expected_name {
            return Err(EngineError::OwnershipNotProven(
                "manifest provider name is not derived from the CellId".to_owned(),
            ));
        }
        if let Some(identity) = &record.provider_object {
            let provider_id = Uuid::parse_str(&identity.id).map_err(|_| {
                EngineError::OwnershipNotProven(
                    "recorded Hyper-V provider object id is not a GUID".to_owned(),
                )
            })?;
            if provider_id.to_string() != identity.id {
                return Err(EngineError::OwnershipNotProven(
                    "recorded Hyper-V provider object id is not canonical".to_owned(),
                ));
            }
            if identity.name != record.ownership.provider_object_name {
                return Err(EngineError::OwnershipNotProven(
                    "recorded provider object name does not match the ownership name".to_owned(),
                ));
            }
        }
        if record.ownership.schema_version != OWNERSHIP_MARKER_SCHEMA {
            return Err(EngineError::OwnershipNotProven(
                "unsupported ownership marker schema".to_owned(),
            ));
        }
        let expected_marker = format!(
            "vmcell:v{}:{}:{}:{}",
            OWNERSHIP_MARKER_SCHEMA,
            record.ownership.install_id,
            record.id.0,
            record.ownership.operation_id
        );
        if record.ownership.provider_marker != expected_marker {
            return Err(EngineError::OwnershipNotProven(
                "manifest ownership marker is internally inconsistent".to_owned(),
            ));
        }
        if !paths_equal(
            &record.ownership.configuration_path,
            &self.state.cell_configuration_path(record.id),
        ) || !paths_equal(
            &record.ownership.overlay_path,
            &self.state.cell_overlay_path(record.id),
        ) {
            return Err(EngineError::OwnershipNotProven(
                "manifest runtime paths are outside the CellId-scoped root".to_owned(),
            ));
        }
        Ok(())
    }

    fn fail_record<T>(&self, mut record: CellRecord, error: EngineError) -> Result<T, EngineError> {
        record.state = CellState::Failed;
        record.updated_at = Utc::now();
        record.last_error = Some(error.to_string());
        self.state.save_cell(&record)?;
        Err(error)
    }
}

fn validate_cell_spec(spec: &CellSpec) -> Result<(), EngineError> {
    if spec.provider.as_deref() != Some("hyperv") {
        return Err(EngineError::UnsupportedProvider(
            spec.provider
                .clone()
                .unwrap_or_else(|| "<missing>".to_owned()),
        ));
    }
    if !(1..=MAX_CPU_COUNT).contains(&spec.cpu_count) {
        return Err(EngineError::InvalidCellRequest(format!(
            "cpu_count must be between 1 and {MAX_CPU_COUNT}"
        )));
    }
    if !(MIN_MEMORY_MIB..=MAX_MEMORY_MIB).contains(&spec.memory_mib) {
        return Err(EngineError::InvalidCellRequest(format!(
            "memory_mib must be between {MIN_MEMORY_MIB} and {MAX_MEMORY_MIB}"
        )));
    }
    if spec.ttl_seconds.is_some() {
        return Err(EngineError::InvalidCellRequest(
            "TTL is not implemented until M2".to_owned(),
        ));
    }
    Ok(())
}

fn require_lifecycle_state(
    record: &CellRecord,
    operation: &str,
    allowed: &[CellState],
) -> Result<(), EngineError> {
    if allowed.contains(&record.state) {
        Ok(())
    } else {
        Err(EngineError::InvalidCellRequest(format!(
            "cannot {operation} cell in {:?} state",
            record.state
        )))
    }
}

fn canonical_vhdx_path(path: &Path) -> Result<PathBuf, EngineError> {
    for ancestor in path.ancestors() {
        if ancestor.as_os_str().is_empty() || !ancestor.exists() {
            continue;
        }
        let metadata = fs::symlink_metadata(ancestor)
            .map_err(|error| EngineError::InvalidImage(format!("{}: {error}", path.display())))?;
        if is_reparse_point(&metadata) {
            return Err(EngineError::InvalidImage(format!(
                "base image ancestors must be ordinary non-reparse directories: {}",
                ancestor.display()
            )));
        }
    }
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| EngineError::InvalidImage(format!("{}: {error}", path.display())))?;
    if !metadata.is_file() || is_reparse_point(&metadata) {
        return Err(EngineError::InvalidImage(format!(
            "base image must be an ordinary non-reparse file: {}",
            path.display()
        )));
    }
    if !path
        .extension()
        .and_then(|value| value.to_str())
        .is_some_and(|value| value.eq_ignore_ascii_case("vhdx"))
    {
        return Err(EngineError::InvalidImage(
            "Hyper-V base image must use the .vhdx extension".to_owned(),
        ));
    }
    path.canonicalize()
        .map_err(|error| EngineError::InvalidImage(error.to_string()))
}

fn open_immutable_parent(path: &Path) -> Result<File, EngineError> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;

        const FILE_SHARE_READ: u32 = 0x0000_0001;
        options.share_mode(FILE_SHARE_READ);
    }
    options
        .open(path)
        .map_err(|error| EngineError::InvalidImage(format!("{}: {error}", path.display())))
}

fn sha256_file(file: &mut File) -> Result<String, EngineError> {
    file.seek(SeekFrom::Start(0))
        .map_err(|error| EngineError::InvalidImage(error.to_string()))?;
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 1024 * 1024];
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|error| EngineError::InvalidImage(error.to_string()))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

fn validate_base_vhdx(
    expected_path: &Path,
    file_size: u64,
    info: &ProviderImageInfo,
) -> Result<(), EngineError> {
    if !paths_equal(expected_path, &info.path) {
        return Err(EngineError::InvalidImage(
            "Hyper-V reported a different VHDX path".to_owned(),
        ));
    }
    if !info.disk_format.eq_ignore_ascii_case("vhdx") {
        return Err(EngineError::InvalidImage(
            "Hyper-V image format is not VHDX".to_owned(),
        ));
    }
    if info.parent_path.is_some() || info.disk_type.eq_ignore_ascii_case("differencing") {
        return Err(EngineError::InvalidImage(
            "a registered base image cannot itself be differencing".to_owned(),
        ));
    }
    if info.file_size != file_size {
        return Err(EngineError::InvalidImage(
            "filesystem and Hyper-V VHDX sizes disagree".to_owned(),
        ));
    }
    Ok(())
}

fn validate_overlay(record: &CellRecord, info: &ProviderImageInfo) -> Result<(), EngineError> {
    if !paths_equal(&record.ownership.overlay_path, &info.path)
        || !info.disk_format.eq_ignore_ascii_case("vhdx")
        || !info.disk_type.eq_ignore_ascii_case("differencing")
        || info
            .parent_path
            .as_ref()
            .is_none_or(|path| !paths_equal(path, &record.image.path))
    {
        return Err(EngineError::ProviderDrift(
            "created overlay did not match the requested VHDX parent/path".to_owned(),
        ));
    }
    Ok(())
}

fn normalize_provider_identity(
    record: &CellRecord,
    identity: ProviderVmIdentity,
) -> Result<ProviderVmIdentity, EngineError> {
    let id = Uuid::parse_str(&identity.id)
        .map_err(|_| {
            EngineError::ProviderDrift(
                "Hyper-V create response did not contain a GUID provider id".to_owned(),
            )
        })?
        .to_string();
    if identity.name != record.ownership.provider_object_name {
        return Err(EngineError::ProviderDrift(
            "Hyper-V create response name did not match the requested CellId name".to_owned(),
        ));
    }
    Ok(ProviderVmIdentity {
        id,
        name: identity.name,
    })
}

fn prove_ownership(record: &CellRecord, vm: &ProviderVm) -> Result<(), EngineError> {
    let reasons = ownership_drift(record, vm);
    if reasons.is_empty() {
        Ok(())
    } else {
        Err(EngineError::OwnershipNotProven(reasons.join("; ")))
    }
}

fn prove_creation_identity(
    record: &CellRecord,
    vm: &ProviderVm,
    require_marker: bool,
) -> Result<(), EngineError> {
    let mut reasons = Vec::new();
    match &record.provider_object {
        Some(identity) if identity.id != vm.id => reasons.push("provider id mismatch"),
        Some(identity) if identity.name != vm.name => reasons.push("recorded name mismatch"),
        None => reasons.push("provider id is not recorded"),
        _ => {}
    }
    if vm.name != record.ownership.provider_object_name {
        reasons.push("ownership name mismatch");
    }
    if require_marker && vm.ownership_marker != record.ownership.provider_marker {
        reasons.push("ownership marker mismatch");
    }
    if !require_marker
        && !vm.ownership_marker.is_empty()
        && vm.ownership_marker != record.ownership.provider_marker
    {
        reasons.push("unexpected ownership marker");
    }
    if !paths_equal(&vm.configuration_path, &record.ownership.configuration_path) {
        reasons.push("configuration path mismatch");
    }
    if vm.attached_disks.len() != 1
        || !paths_equal(&vm.attached_disks[0], &record.ownership.overlay_path)
    {
        reasons.push("attached disk mismatch");
    }
    if vm.memory_mib != record.spec.memory_mib {
        reasons.push("memory configuration mismatch");
    }
    if vm.network_adapter_count > 1 {
        reasons.push("new provider object has unexpected network adapters");
    }
    if vm.power_state != ProviderPowerState::Off {
        reasons.push("new provider object is not off");
    }
    if reasons.is_empty() {
        Ok(())
    } else {
        Err(EngineError::OwnershipNotProven(reasons.join("; ")))
    }
}

fn prove_destroy_identity(record: &CellRecord, vm: &ProviderVm) -> Result<(), EngineError> {
    let mut reasons = Vec::new();
    match &record.provider_object {
        Some(identity) if identity.id != vm.id => reasons.push("provider id mismatch"),
        Some(identity) if identity.name != vm.name => reasons.push("recorded name mismatch"),
        None => reasons.push("provider id is not recorded"),
        _ => {}
    }
    if vm.name != record.ownership.provider_object_name {
        reasons.push("ownership name mismatch");
    }
    if vm.ownership_marker != record.ownership.provider_marker {
        reasons.push("ownership marker mismatch");
    }
    if !paths_equal(&vm.configuration_path, &record.ownership.configuration_path) {
        reasons.push("configuration path mismatch");
    }
    if vm.attached_disks.len() != 1
        || !paths_equal(&vm.attached_disks[0], &record.ownership.overlay_path)
    {
        reasons.push("attached disk mismatch");
    }
    if vm.memory_mib != record.spec.memory_mib {
        reasons.push("memory configuration mismatch");
    }
    if reasons.is_empty() {
        Ok(())
    } else {
        Err(EngineError::OwnershipNotProven(reasons.join("; ")))
    }
}

fn ownership_drift(record: &CellRecord, vm: &ProviderVm) -> Vec<String> {
    let mut reasons = Vec::new();
    match &record.provider_object {
        Some(identity) if identity.id != vm.id => reasons.push("provider id mismatch".to_owned()),
        Some(identity) if identity.name != vm.name => {
            reasons.push("recorded name mismatch".to_owned())
        }
        None => reasons.push("provider id is not recorded".to_owned()),
        _ => {}
    }
    if vm.name != record.ownership.provider_object_name {
        reasons.push("ownership name mismatch".to_owned());
    }
    if vm.ownership_marker != record.ownership.provider_marker {
        reasons.push("ownership marker mismatch".to_owned());
    }
    if !paths_equal(&vm.configuration_path, &record.ownership.configuration_path) {
        reasons.push("configuration path mismatch".to_owned());
    }
    if vm.attached_disks.len() != 1
        || !paths_equal(&vm.attached_disks[0], &record.ownership.overlay_path)
    {
        reasons.push("attached disk mismatch".to_owned());
    }
    if vm.network_adapter_count != 0 {
        reasons.push("networking is not disabled".to_owned());
    }
    if vm.cpu_count != record.spec.cpu_count {
        reasons.push("CPU configuration mismatch".to_owned());
    }
    if vm.memory_mib != record.spec.memory_mib {
        reasons.push("memory configuration mismatch".to_owned());
    }
    reasons
}

fn power_state_matches(cell: CellState, provider: &ProviderPowerState) -> bool {
    matches!(
        (cell, provider),
        (CellState::Stopped, ProviderPowerState::Off)
            | (CellState::Running, ProviderPowerState::Running)
            | (CellState::Creating, _)
            | (CellState::Destroying, _)
            | (CellState::Failed, _)
    )
}

fn hyperv_variant(image: &ImageRecord) -> Result<&ImageVariant, EngineError> {
    let mut variants = image
        .variants
        .iter()
        .filter(|variant| variant.provider == "hyperv");
    let variant = variants
        .next()
        .ok_or_else(|| EngineError::InvalidImage("image has no Hyper-V VHDX variant".to_owned()))?;
    if variants.next().is_some() {
        return Err(EngineError::InvalidImage(
            "image has more than one Hyper-V variant".to_owned(),
        ));
    }
    Ok(variant)
}

fn same_image_identity(left: &ImageRecord, right: &ImageRecord) -> bool {
    left.id == right.id
        && left.guest_os == right.guest_os
        && left.guest_arch == right.guest_arch
        && left.variants == right.variants
}

fn operation_report(record: &CellRecord, changed: bool) -> OperationReport {
    OperationReport {
        schema_version: 1,
        cell_id: record.id,
        state: record.state,
        changed,
    }
}

fn paths_equal(left: &Path, right: &Path) -> bool {
    #[cfg(windows)]
    {
        windows_path_identity(left).eq_ignore_ascii_case(&windows_path_identity(right))
    }
    #[cfg(not(windows))]
    {
        left == right
    }
}

#[cfg(windows)]
fn windows_path_identity(path: &Path) -> String {
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
    value.trim_end_matches('\\').to_owned()
}

#[cfg(windows)]
fn is_reparse_point(metadata: &fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;

    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
    metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

#[cfg(not(windows))]
fn is_reparse_point(metadata: &fs::Metadata) -> bool {
    metadata.file_type().is_symlink()
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use tempfile::tempdir;

    use super::*;
    use crate::core::capability::ProviderCapabilities;
    use crate::providers::{ProviderProbe, ProviderVmIdentity};

    #[cfg(windows)]
    fn alternate_windows_path(path: &Path) -> PathBuf {
        let value = path.to_string_lossy().replace('/', "\\");
        if value
            .get(..8)
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case(r"\\?\UNC\"))
        {
            PathBuf::from(format!(r"\\{}", &value[8..]))
        } else if value
            .get(..4)
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case(r"\\?\"))
        {
            PathBuf::from(&value[4..])
        } else if let Some(rest) = value.strip_prefix(r"\\") {
            PathBuf::from(format!(r"\\?\UNC\{rest}"))
        } else {
            PathBuf::from(format!(r"\\?\{value}"))
        }
    }

    #[cfg(not(windows))]
    fn alternate_windows_path(path: &Path) -> PathBuf {
        path.to_path_buf()
    }

    #[derive(Debug, Default)]
    struct MockState {
        vm: Option<ProviderVm>,
        calls: Vec<&'static str>,
        remove_calls: usize,
        drift_before_mutation: bool,
        fail_claim: bool,
        fail_configure: bool,
        fail_configure_after_network: bool,
        malformed_create_identity: bool,
        noncanonical_create_identity: bool,
        installation_rotation_path: Option<PathBuf>,
        installation_rotation_blocked: bool,
    }

    #[derive(Clone)]
    struct MockHyperV {
        base_size: u64,
        use_path_aliases: bool,
        state: Arc<Mutex<MockState>>,
    }

    impl MockHyperV {
        fn new(base_path: PathBuf) -> Self {
            let base_size = fs::metadata(&base_path).unwrap().len();
            Self {
                base_size,
                use_path_aliases: false,
                state: Arc::new(Mutex::new(MockState::default())),
            }
        }

        #[cfg(windows)]
        fn with_path_aliases(base_path: PathBuf) -> Self {
            let mut provider = Self::new(base_path);
            provider.use_path_aliases = true;
            provider
        }

        fn provider_path(&self, path: &Path) -> PathBuf {
            if self.use_path_aliases {
                alternate_windows_path(path)
            } else {
                path.to_path_buf()
            }
        }

        fn alter_marker(&self) {
            self.state
                .lock()
                .unwrap()
                .vm
                .as_mut()
                .unwrap()
                .ownership_marker = "foreign".to_owned();
        }

        fn drift_before_mutation(&self) {
            self.state.lock().unwrap().drift_before_mutation = true;
        }

        #[cfg(windows)]
        fn rotate_installation_during_mutation(&self, path: PathBuf) {
            self.state.lock().unwrap().installation_rotation_path = Some(path);
        }
    }

    impl LocalVmProvider for MockHyperV {
        fn name(&self) -> &'static str {
            "hyperv"
        }

        fn probe(&self) -> ProviderProbe {
            ProviderProbe {
                name: "hyperv",
                available: true,
                detail: "mock".to_owned(),
                capabilities: ProviderCapabilities::unavailable(),
            }
        }

        fn inspect_image(&self, path: PathBuf) -> Result<ProviderImageInfo, ProviderError> {
            self.state.lock().unwrap().calls.push("inspect_image");
            Ok(ProviderImageInfo {
                path: self.provider_path(&path),
                disk_format: "vhdx".to_owned(),
                disk_type: "dynamic".to_owned(),
                parent_path: None,
                file_size: self.base_size,
                virtual_size: 1024 * 1024,
            })
        }

        fn create_overlay(
            &self,
            _authority: &ProviderMutationAuthority<'_>,
            request: &CreateOverlayRequest,
        ) -> Result<ProviderImageInfo, ProviderError> {
            self.state.lock().unwrap().calls.push("create_overlay");
            fs::write(&request.overlay_path, b"overlay").unwrap();
            Ok(ProviderImageInfo {
                path: self.provider_path(&request.overlay_path),
                disk_format: "vhdx".to_owned(),
                disk_type: "differencing".to_owned(),
                parent_path: Some(self.provider_path(&request.parent_path)),
                file_size: 7,
                virtual_size: 1024 * 1024,
            })
        }

        fn create_vm(
            &self,
            _authority: &ProviderMutationAuthority<'_>,
            request: &CreateVmRequest,
        ) -> Result<ProviderVmIdentity, ProviderError> {
            let mut state = self.state.lock().unwrap();
            state.calls.push("create_vm");
            let canonical_id = Uuid::new_v4().to_string();
            let returned_id = if state.malformed_create_identity {
                "not-a-guid".to_owned()
            } else if state.noncanonical_create_identity {
                format!("{{{canonical_id}}}")
            } else {
                canonical_id.clone()
            };
            let vm = ProviderVm {
                id: if state.malformed_create_identity {
                    returned_id.clone()
                } else {
                    canonical_id
                },
                name: request.name.clone(),
                power_state: ProviderPowerState::Off,
                ownership_marker: String::new(),
                configuration_path: self.provider_path(&request.configuration_path),
                attached_disks: vec![self.provider_path(&request.overlay_path)],
                network_adapter_count: 1,
                cpu_count: 1,
                memory_mib: request.memory_mib,
            };
            state.vm = Some(vm.clone());
            Ok(ProviderVmIdentity {
                id: returned_id,
                name: vm.name,
            })
        }

        fn claim_vm(
            &self,
            _authority: &ProviderMutationAuthority<'_>,
            request: &ClaimVmRequest,
        ) -> Result<ProviderVm, ProviderError> {
            let mut state = self.state.lock().unwrap();
            state.calls.push("claim_vm");
            if state.fail_claim {
                return Err(ProviderError::Command("injected claim failure".to_owned()));
            }
            if state.vm.as_ref() != Some(&request.expected) {
                return Err(ProviderError::OwnershipChanged(
                    "creation receipt changed".to_owned(),
                ));
            }
            let vm = state.vm.as_mut().unwrap();
            vm.ownership_marker = request.ownership_marker.clone();
            Ok(vm.clone())
        }

        fn configure_vm(
            &self,
            _authority: &ProviderMutationAuthority<'_>,
            request: &ConfigureVmRequest,
        ) -> Result<ProviderVm, ProviderError> {
            let mut state = self.state.lock().unwrap();
            state.calls.push("configure_vm");
            if state.fail_configure {
                return Err(ProviderError::Command(
                    "injected configure failure".to_owned(),
                ));
            }
            if state.vm.as_ref() != Some(&request.expected) {
                return Err(ProviderError::OwnershipChanged(
                    "claimed VM changed".to_owned(),
                ));
            }
            let fail_after_network = state.fail_configure_after_network;
            let vm = state.vm.as_mut().unwrap();
            vm.network_adapter_count = 0;
            if fail_after_network {
                return Err(ProviderError::Command(
                    "injected failure after network removal".to_owned(),
                ));
            }
            vm.cpu_count = request.cpu_count;
            Ok(vm.clone())
        }

        fn inspect_vm(&self, lookup: &VmLookup) -> Result<Option<ProviderVm>, ProviderError> {
            let mut state = self.state.lock().unwrap();
            state.calls.push("inspect_vm");
            Ok(state.vm.clone().filter(|vm| match lookup {
                VmLookup::Id(id) => vm.id == *id,
                VmLookup::Name(name) => vm.name == *name,
            }))
        }

        fn start_vm(
            &self,
            _authority: &ProviderMutationAuthority<'_>,
            expected: &ProviderVm,
        ) -> Result<(), ProviderError> {
            let mut state = self.state.lock().unwrap();
            state.calls.push("start_vm");
            #[cfg(windows)]
            if let Some(path) = state.installation_rotation_path.take() {
                state.installation_rotation_blocked = fs::write(path, b"rotated").is_err();
            }
            if state.drift_before_mutation {
                state.vm.as_mut().unwrap().ownership_marker = "foreign-race".to_owned();
                state.drift_before_mutation = false;
            }
            if state.vm.as_ref() != Some(expected) {
                return Err(ProviderError::OwnershipChanged(
                    "VM changed before start".to_owned(),
                ));
            }
            let vm = state
                .vm
                .as_mut()
                .ok_or_else(|| ProviderError::NotFound(expected.id.clone()))?;
            vm.power_state = ProviderPowerState::Running;
            Ok(())
        }

        fn stop_vm(
            &self,
            _authority: &ProviderMutationAuthority<'_>,
            expected: &ProviderVm,
        ) -> Result<(), ProviderError> {
            let mut state = self.state.lock().unwrap();
            state.calls.push("stop_vm");
            if state.drift_before_mutation {
                state.vm.as_mut().unwrap().ownership_marker = "foreign-race".to_owned();
                state.drift_before_mutation = false;
            }
            if state.vm.as_ref() != Some(expected) {
                return Err(ProviderError::OwnershipChanged(
                    "VM changed before stop".to_owned(),
                ));
            }
            let vm = state
                .vm
                .as_mut()
                .ok_or_else(|| ProviderError::NotFound(expected.id.clone()))?;
            vm.power_state = ProviderPowerState::Off;
            Ok(())
        }

        fn remove_vm(
            &self,
            _authority: &ProviderMutationAuthority<'_>,
            expected: &ProviderVm,
        ) -> Result<(), ProviderError> {
            let mut state = self.state.lock().unwrap();
            state.calls.push("remove_vm");
            if state.drift_before_mutation {
                state.vm.as_mut().unwrap().ownership_marker = "foreign-race".to_owned();
                state.drift_before_mutation = false;
            }
            if state.vm.as_ref() != Some(expected) {
                return Err(ProviderError::OwnershipChanged(
                    "VM changed before remove".to_owned(),
                ));
            }
            state.remove_calls += 1;
            state.vm = None;
            Ok(())
        }
    }

    fn fixture() -> (tempfile::TempDir, CellEngine<MockHyperV>, ImageId) {
        let directory = tempdir().unwrap();
        let base_path = directory.path().join("base.vhdx");
        fs::write(&base_path, b"immutable base").unwrap();
        let provider = MockHyperV::new(base_path.clone());
        let engine = CellEngine::new(StateStore::new(directory.path().join("state")), provider);
        let image_id = ImageId::parse("windows-dev").unwrap();
        engine
            .register_image(RegisterImageRequest {
                id: image_id.clone(),
                guest_os: GuestOs::Windows,
                guest_arch: Architecture::X86_64,
                path: base_path,
            })
            .unwrap();
        (directory, engine, image_id)
    }

    fn spec(image: ImageId) -> CellSpec {
        CellSpec {
            image,
            provider: Some("hyperv".to_owned()),
            cpu_count: 2,
            memory_mib: 4096,
            ttl_seconds: None,
        }
    }

    #[test]
    fn image_registration_pins_sha256_and_is_idempotent() {
        let (directory, engine, image_id) = fixture();
        let record = engine.state.load_image(&image_id).unwrap();
        assert_eq!(record.variants.len(), 1);
        assert_eq!(record.variants[0].sha256.len(), 64);

        let second = engine
            .register_image(RegisterImageRequest {
                id: image_id,
                guest_os: GuestOs::Windows,
                guest_arch: Architecture::X86_64,
                path: directory.path().join("base.vhdx"),
            })
            .unwrap();
        assert_eq!(second, record);
    }

    #[test]
    fn create_finishes_stopped_with_one_networkless_overlay() {
        let (_directory, engine, image_id) = fixture();
        let cell = engine.create_cell(spec(image_id)).unwrap();

        assert_eq!(cell.state, CellState::Stopped);
        assert_eq!(cell.phase, CellPhase::Ready);
        let inspection = engine.inspect_cell(cell.id).unwrap();
        assert_eq!(inspection.reconciliation, ReconciliationStatus::ExactOwned);
        let vm = inspection.provider_vm.unwrap();
        assert_eq!(vm.attached_disks, vec![cell.ownership.overlay_path]);
        assert_eq!(vm.network_adapter_count, 0);
    }

    #[test]
    fn lifecycle_and_destroy_are_owned_and_idempotent() {
        let (_directory, engine, image_id) = fixture();
        let cell = engine.create_cell(spec(image_id)).unwrap();

        assert!(engine.start_cell(cell.id).unwrap().changed);
        assert_eq!(
            engine.inspect_cell(cell.id).unwrap().cell.state,
            CellState::Running
        );
        assert!(engine.stop_cell(cell.id).unwrap().changed);
        assert!(engine.destroy_cell(cell.id).unwrap().changed);
        assert!(!engine.destroy_cell(cell.id).unwrap().changed);
        assert!(!engine.state.cell_runtime_root(cell.id).exists());
        assert_eq!(engine.provider.state.lock().unwrap().remove_calls, 1);
    }

    #[test]
    fn destroy_refuses_marker_drift_without_provider_mutation() {
        let (_directory, engine, image_id) = fixture();
        let cell = engine.create_cell(spec(image_id)).unwrap();
        engine.provider.alter_marker();

        let error = engine.destroy_cell(cell.id).unwrap_err();

        assert!(matches!(error, EngineError::OwnershipNotProven(_)));
        let provider = engine.provider.state.lock().unwrap();
        assert_eq!(provider.remove_calls, 0);
        assert!(provider.vm.is_some());
    }

    #[test]
    fn destroy_refuses_name_only_identity_without_provider_mutation() {
        let (_directory, engine, image_id) = fixture();
        let cell = engine.create_cell(spec(image_id)).unwrap();
        let mut incomplete = engine.state.load_cell(cell.id).unwrap();
        incomplete.provider_object = None;
        incomplete.phase = CellPhase::OverlayCreated;
        engine.state.save_cell(&incomplete).unwrap();

        let inspection = engine.reconcile_cell(cell.id).unwrap();
        assert!(matches!(
            inspection.reconciliation,
            ReconciliationStatus::UnprovenProviderObject { .. }
        ));
        let error = engine.destroy_cell(cell.id).unwrap_err();

        assert!(matches!(error, EngineError::OwnershipNotProven(_)));
        assert_eq!(engine.provider.state.lock().unwrap().remove_calls, 0);
    }

    #[test]
    fn destroy_rejects_invalid_recorded_provider_identity_before_provider_or_runtime_mutation() {
        let (_directory, engine, image_id) = fixture();
        let cell = engine.create_cell(spec(image_id)).unwrap();

        for invalid_identity in [
            ProviderObjectIdentity {
                id: "not-a-guid".to_owned(),
                name: cell.ownership.provider_object_name.clone(),
            },
            ProviderObjectIdentity {
                id: format!("{{{}}}", cell.provider_object.as_ref().unwrap().id),
                name: cell.ownership.provider_object_name.clone(),
            },
            ProviderObjectIdentity {
                id: cell.provider_object.as_ref().unwrap().id.clone(),
                name: "foreign-name".to_owned(),
            },
        ] {
            let mut corrupted = engine.state.load_cell(cell.id).unwrap();
            corrupted.provider_object = Some(invalid_identity);
            engine.state.save_cell(&corrupted).unwrap();
            let calls_before = engine.provider.state.lock().unwrap().calls.len();

            assert!(matches!(
                engine.destroy_cell(cell.id),
                Err(EngineError::OwnershipNotProven(_))
            ));
            assert_eq!(
                engine.provider.state.lock().unwrap().calls.len(),
                calls_before
            );
            assert!(engine.provider.state.lock().unwrap().vm.is_some());
            assert!(engine.state.cell_runtime_root(cell.id).exists());
        }
    }

    #[test]
    fn provider_id_persistence_crash_window_remains_name_only_fail_closed() {
        let (_directory, engine, image_id) = fixture();
        let cell = engine.create_cell(spec(image_id)).unwrap();
        let mut interrupted = engine.state.load_cell(cell.id).unwrap();
        interrupted.provider_object = None;
        interrupted.phase = CellPhase::OverlayCreated;
        interrupted.state = CellState::Failed;
        engine.state.save_cell(&interrupted).unwrap();

        let inspection = engine.reconcile_cell(cell.id).unwrap();
        assert!(matches!(
            inspection.reconciliation,
            ReconciliationStatus::UnprovenProviderObject { .. }
        ));
        assert!(matches!(
            engine.destroy_cell(cell.id),
            Err(EngineError::OwnershipNotProven(_))
        ));
        assert_eq!(engine.provider.state.lock().unwrap().remove_calls, 0);
        assert!(engine.state.cell_runtime_root(cell.id).exists());
    }

    #[test]
    fn tampered_runtime_path_blocks_start_before_provider_mutation() {
        let (directory, engine, image_id) = fixture();
        let cell = engine.create_cell(spec(image_id)).unwrap();
        let mut tampered = engine.state.load_cell(cell.id).unwrap();
        tampered.ownership.overlay_path = directory.path().join("foreign.vhdx");
        engine.state.save_cell(&tampered).unwrap();
        let calls_before = engine.provider.state.lock().unwrap().calls.len();

        let error = engine.start_cell(cell.id).unwrap_err();

        assert!(matches!(error, EngineError::OwnershipNotProven(_)));
        let provider = engine.provider.state.lock().unwrap();
        assert_eq!(provider.calls.len(), calls_before);
        assert_eq!(
            provider.vm.as_ref().unwrap().power_state,
            ProviderPowerState::Off
        );
    }

    #[test]
    fn destroyed_tombstone_reports_provider_reappearance() {
        let (_directory, engine, image_id) = fixture();
        let cell = engine.create_cell(spec(image_id)).unwrap();
        let snapshot = engine.provider.state.lock().unwrap().vm.clone().unwrap();
        engine.destroy_cell(cell.id).unwrap();
        engine.provider.state.lock().unwrap().vm = Some(snapshot);

        let inspection = engine.reconcile_cell(cell.id).unwrap();

        assert!(matches!(
            inspection.reconciliation,
            ReconciliationStatus::OwnershipMismatch { .. }
        ));
        assert!(engine.destroy_cell(cell.id).is_err());
        assert_eq!(engine.provider.state.lock().unwrap().remove_calls, 1);
    }

    #[test]
    fn changed_parent_image_blocks_create_before_overlay() {
        let (directory, engine, image_id) = fixture();
        fs::write(directory.path().join("base.vhdx"), b"changed").unwrap();

        let error = engine.create_cell(spec(image_id)).unwrap_err();

        assert!(matches!(error, EngineError::InvalidImage(_)));
        assert!(
            !engine
                .provider
                .state
                .lock()
                .unwrap()
                .calls
                .contains(&"create_overlay")
        );
    }

    #[test]
    fn installation_rotation_blocks_all_cell_authority_before_provider_access() {
        let (_directory, engine, image_id) = fixture();
        let cell = engine.create_cell(spec(image_id)).unwrap();
        let calls_before = engine.provider.state.lock().unwrap().calls.len();
        let replacement = crate::state::InstallationRecord {
            schema_version: crate::state::INSTALL_SCHEMA_VERSION,
            install_id: Uuid::new_v4(),
        };
        fs::write(
            engine.state.root().join("installation.json"),
            serde_json::to_vec(&replacement).unwrap(),
        )
        .unwrap();

        assert!(matches!(
            engine.start_cell(cell.id),
            Err(EngineError::OwnershipNotProven(_))
        ));
        assert!(matches!(
            engine.stop_cell(cell.id),
            Err(EngineError::OwnershipNotProven(_))
        ));
        assert!(matches!(
            engine.destroy_cell(cell.id),
            Err(EngineError::OwnershipNotProven(_))
        ));
        let inspection = engine.reconcile_cell(cell.id).unwrap();
        assert!(matches!(
            inspection.reconciliation,
            ReconciliationStatus::OwnershipMismatch { .. }
        ));
        assert_eq!(
            engine.provider.state.lock().unwrap().calls.len(),
            calls_before
        );
        assert!(engine.state.cell_runtime_root(cell.id).exists());
    }

    #[test]
    fn distinct_or_cloned_state_root_cannot_authorize_another_roots_cell() {
        let directory = tempdir().unwrap();
        let base_path = directory.path().join("base.vhdx");
        fs::write(&base_path, b"immutable base").unwrap();
        let provider = MockHyperV::new(base_path.clone());
        let first = CellEngine::new(
            StateStore::new(directory.path().join("state-a")),
            provider.clone(),
        );
        let image_id = ImageId::parse("windows-dev").unwrap();
        first
            .register_image(RegisterImageRequest {
                id: image_id.clone(),
                guest_os: GuestOs::Windows,
                guest_arch: Architecture::X86_64,
                path: base_path,
            })
            .unwrap();
        let cell = first.create_cell(spec(image_id)).unwrap();

        let second = CellEngine::new(
            StateStore::new(directory.path().join("state-b")),
            provider.clone(),
        );
        let second_installation = second.state.installation().unwrap();
        assert_ne!(second_installation.install_id, cell.ownership.install_id);
        let second_manifest = second
            .state
            .root()
            .join("cells")
            .join(format!("{}.json", cell.id));
        fs::create_dir_all(second_manifest.parent().unwrap()).unwrap();
        fs::copy(
            first
                .state
                .root()
                .join("cells")
                .join(format!("{}.json", cell.id)),
            second_manifest,
        )
        .unwrap();
        let calls_before = second.provider.state.lock().unwrap().calls.len();

        assert!(matches!(
            second.start_cell(cell.id),
            Err(EngineError::OwnershipNotProven(_))
        ));
        assert!(matches!(
            second.destroy_cell(cell.id),
            Err(EngineError::OwnershipNotProven(_))
        ));
        assert!(matches!(
            second.reconcile_cell(cell.id).unwrap().reconciliation,
            ReconciliationStatus::OwnershipMismatch { .. }
        ));
        assert_eq!(
            second.provider.state.lock().unwrap().calls.len(),
            calls_before
        );
        assert!(second.provider.state.lock().unwrap().vm.is_some());

        let cloned_root = directory.path().join("state-cloned");
        fs::create_dir_all(cloned_root.join("cells")).unwrap();
        fs::copy(
            first.state.root().join("installation.json"),
            cloned_root.join("installation.json"),
        )
        .unwrap();
        fs::copy(
            first
                .state
                .root()
                .join("cells")
                .join(format!("{}.json", cell.id)),
            cloned_root.join("cells").join(format!("{}.json", cell.id)),
        )
        .unwrap();
        let cloned = CellEngine::new(StateStore::new(cloned_root), provider);
        assert_eq!(
            cloned.state.load_installation().unwrap().install_id,
            cell.ownership.install_id
        );
        let cloned_calls_before = cloned.provider.state.lock().unwrap().calls.len();
        assert!(matches!(
            cloned.destroy_cell(cell.id),
            Err(EngineError::OwnershipNotProven(_))
        ));
        assert!(matches!(
            cloned.reconcile_cell(cell.id).unwrap().reconciliation,
            ReconciliationStatus::OwnershipMismatch { .. }
        ));
        assert_eq!(
            cloned.provider.state.lock().unwrap().calls.len(),
            cloned_calls_before
        );
        assert!(cloned.provider.state.lock().unwrap().vm.is_some());
    }

    #[test]
    fn missing_installation_blocks_destroy_and_runtime_deletion() {
        let (_directory, engine, image_id) = fixture();
        let cell = engine.create_cell(spec(image_id)).unwrap();
        let calls_before = engine.provider.state.lock().unwrap().calls.len();
        fs::remove_file(engine.state.root().join("installation.json")).unwrap();

        assert!(matches!(
            engine.destroy_cell(cell.id),
            Err(EngineError::State(StateError::NotFound(_)))
        ));
        assert_eq!(
            engine.provider.state.lock().unwrap().calls.len(),
            calls_before
        );
        assert!(engine.state.cell_runtime_root(cell.id).exists());
        assert!(!engine.state.root().join("installation.json").exists());
    }

    #[test]
    fn cell_schema_drift_is_rejected_before_provider_access() {
        let (_directory, engine, image_id) = fixture();
        let cell = engine.create_cell(spec(image_id)).unwrap();
        let mut record = engine.state.load_cell(cell.id).unwrap();
        record.schema_version += 1;
        let path = engine
            .state
            .root()
            .join("cells")
            .join(format!("{}.json", cell.id.0));
        fs::write(&path, serde_json::to_vec(&record).unwrap()).unwrap();
        let calls_before = engine.provider.state.lock().unwrap().calls.len();

        assert!(matches!(
            engine.start_cell(cell.id),
            Err(EngineError::State(StateError::UnsupportedSchema {
                kind: "cell record",
                ..
            }))
        ));
        assert_eq!(
            engine.provider.state.lock().unwrap().calls.len(),
            calls_before
        );

        record.schema_version = CELL_SCHEMA_VERSION;
        record.ownership.schema_version += 1;
        fs::write(&path, serde_json::to_vec(&record).unwrap()).unwrap();
        assert!(matches!(
            engine.destroy_cell(cell.id),
            Err(EngineError::State(StateError::UnsupportedSchema {
                kind: "cell ownership",
                ..
            }))
        ));
        assert_eq!(
            engine.provider.state.lock().unwrap().calls.len(),
            calls_before
        );
    }

    #[test]
    fn image_schema_drift_is_rejected_before_overlay_creation() {
        let (_directory, engine, image_id) = fixture();
        let mut image = engine.state.load_image(&image_id).unwrap();
        image.schema_version += 1;
        let path = engine
            .state
            .root()
            .join("images")
            .join(format!("{}.json", image_id.as_str()));
        fs::write(&path, serde_json::to_vec(&image).unwrap()).unwrap();
        let calls_before = engine.provider.state.lock().unwrap().calls.len();

        assert!(matches!(
            engine.create_cell(spec(image_id)),
            Err(EngineError::State(StateError::UnsupportedSchema {
                kind: "image record",
                ..
            }))
        ));
        assert_eq!(
            engine.provider.state.lock().unwrap().calls.len(),
            calls_before
        );
    }

    #[test]
    fn provider_action_envelope_rejects_race_drift_before_mutation() {
        let (_directory, engine, image_id) = fixture();
        let cell = engine.create_cell(spec(image_id)).unwrap();
        engine.provider.drift_before_mutation();

        assert!(matches!(
            engine.start_cell(cell.id),
            Err(EngineError::Provider(ProviderError::OwnershipChanged(_)))
        ));
        assert_eq!(
            engine
                .provider
                .state
                .lock()
                .unwrap()
                .vm
                .as_ref()
                .unwrap()
                .power_state,
            ProviderPowerState::Off
        );
    }

    #[test]
    fn stop_action_envelope_rejects_race_drift_before_mutation() {
        let (_directory, engine, image_id) = fixture();
        let cell = engine.create_cell(spec(image_id)).unwrap();
        engine.start_cell(cell.id).unwrap();
        engine.provider.drift_before_mutation();

        assert!(matches!(
            engine.stop_cell(cell.id),
            Err(EngineError::Provider(ProviderError::OwnershipChanged(_)))
        ));
        assert_eq!(
            engine
                .provider
                .state
                .lock()
                .unwrap()
                .vm
                .as_ref()
                .unwrap()
                .power_state,
            ProviderPowerState::Running
        );
    }

    #[test]
    fn remove_action_envelope_rejects_race_drift_before_mutation() {
        let (_directory, engine, image_id) = fixture();
        let cell = engine.create_cell(spec(image_id)).unwrap();
        engine.provider.drift_before_mutation();

        assert!(matches!(
            engine.destroy_cell(cell.id),
            Err(EngineError::Provider(ProviderError::OwnershipChanged(_)))
        ));
        let provider = engine.provider.state.lock().unwrap();
        assert!(provider.vm.is_some());
        assert_eq!(provider.remove_calls, 0);
        assert!(engine.state.cell_runtime_root(cell.id).exists());
    }

    #[test]
    fn provider_id_is_persisted_before_claim_failure() {
        let (_directory, engine, image_id) = fixture();
        engine.provider.state.lock().unwrap().fail_claim = true;

        assert!(engine.create_cell(spec(image_id)).is_err());
        let cell = engine.state.list_cells().unwrap().pop().unwrap();
        assert_eq!(cell.phase, CellPhase::ProviderObjectCreated);
        assert!(cell.provider_object.is_some());
        let inspection = engine.reconcile_cell(cell.id).unwrap();
        assert!(matches!(
            inspection.reconciliation,
            ReconciliationStatus::Provisioning {
                phase: CellPhase::ProviderObjectCreated
            }
        ));
        assert_eq!(engine.provider.state.lock().unwrap().remove_calls, 0);
    }

    #[test]
    fn malformed_create_identity_is_quarantined_before_claim_or_configuration() {
        let (_directory, engine, image_id) = fixture();
        engine
            .provider
            .state
            .lock()
            .unwrap()
            .malformed_create_identity = true;

        assert!(matches!(
            engine.create_cell(spec(image_id)),
            Err(EngineError::ProviderDrift(_))
        ));
        let cell = engine.state.list_cells().unwrap().pop().unwrap();
        assert_eq!(cell.state, CellState::Failed);
        assert_eq!(cell.phase, CellPhase::OverlayCreated);
        assert!(cell.provider_object.is_none());
        let provider = engine.provider.state.lock().unwrap();
        assert!(provider.calls.contains(&"create_vm"));
        assert!(!provider.calls.contains(&"claim_vm"));
        assert!(!provider.calls.contains(&"configure_vm"));
    }

    #[test]
    fn noncanonical_create_guid_is_normalized_before_persistence_and_lifecycle() {
        let (_directory, engine, image_id) = fixture();
        engine
            .provider
            .state
            .lock()
            .unwrap()
            .noncanonical_create_identity = true;

        let cell = engine.create_cell(spec(image_id)).unwrap();
        let persisted = cell.provider_object.as_ref().unwrap();
        assert_eq!(
            persisted.id,
            Uuid::parse_str(&persisted.id).unwrap().to_string()
        );
        assert!(engine.destroy_cell(cell.id).unwrap().changed);
    }

    #[test]
    fn claimed_partial_configuration_is_deterministically_reconciled() {
        let (_directory, engine, image_id) = fixture();
        engine.provider.state.lock().unwrap().fail_configure = true;

        assert!(engine.create_cell(spec(image_id)).is_err());
        let cell = engine.state.list_cells().unwrap().pop().unwrap();
        assert_eq!(cell.phase, CellPhase::ProviderObjectClaimed);
        assert!(cell.provider_object.is_some());
        let inspection = engine.reconcile_cell(cell.id).unwrap();
        assert!(matches!(
            inspection.reconciliation,
            ReconciliationStatus::Provisioning {
                phase: CellPhase::ProviderObjectClaimed
            }
        ));
    }

    #[cfg(windows)]
    #[test]
    fn windows_verbatim_and_ordinary_paths_share_one_identity() {
        assert!(paths_equal(
            Path::new(r"C:\vmcell\base.vhdx"),
            Path::new(r"\\?\C:\vmcell\base.vhdx")
        ));
        assert!(paths_equal(
            Path::new(r"\\server\share\base.vhdx"),
            Path::new(r"\\?\UNC\server\share\base.vhdx")
        ));
    }

    #[cfg(windows)]
    #[test]
    fn provider_path_aliases_reconcile_as_exact_identity() {
        let directory = tempdir().unwrap();
        let base_path = directory.path().join("base.vhdx");
        fs::write(&base_path, b"immutable base").unwrap();
        let provider = MockHyperV::with_path_aliases(base_path.clone());
        let engine = CellEngine::new(StateStore::new(directory.path().join("state")), provider);
        let image_id = ImageId::parse("windows-alias").unwrap();
        engine
            .register_image(RegisterImageRequest {
                id: image_id.clone(),
                guest_os: GuestOs::Windows,
                guest_arch: Architecture::X86_64,
                path: base_path,
            })
            .unwrap();

        let cell = engine.create_cell(spec(image_id)).unwrap();
        assert_eq!(
            engine.reconcile_cell(cell.id).unwrap().reconciliation,
            ReconciliationStatus::ExactOwned
        );
    }

    #[test]
    fn missing_provider_is_rejected_before_any_local_or_provider_mutation() {
        let (_directory, engine, image_id) = fixture();
        let calls_before = engine.provider.state.lock().unwrap().calls.len();
        let mut request = spec(image_id);
        request.provider = None;

        assert!(matches!(
            engine.create_cell(request),
            Err(EngineError::UnsupportedProvider(provider)) if provider == "<missing>"
        ));
        assert_eq!(
            engine.provider.state.lock().unwrap().calls.len(),
            calls_before
        );
        assert!(engine.state.list_cells().unwrap().is_empty());
    }

    #[test]
    fn cell_filename_identity_mismatch_never_reaches_provider() {
        let (_directory, engine, image_id) = fixture();
        let cell = engine.create_cell(spec(image_id)).unwrap();
        let forged_id = CellId::new();
        let forged_path = engine
            .state
            .root()
            .join("cells")
            .join(format!("{forged_id}.json"));
        fs::write(&forged_path, serde_json::to_vec(&cell).unwrap()).unwrap();
        let calls_before = engine.provider.state.lock().unwrap().calls.len();

        assert!(matches!(
            engine.start_cell(forged_id),
            Err(EngineError::State(StateError::IdentityMismatch {
                kind: "cell record",
                ..
            }))
        ));
        assert_eq!(
            engine.provider.state.lock().unwrap().calls.len(),
            calls_before
        );
    }

    #[test]
    fn pre_id_manifest_without_provider_object_remains_quarantined() {
        let (_directory, engine, image_id) = fixture();
        let cell = engine.create_cell(spec(image_id)).unwrap();
        let mut interrupted = engine.state.load_cell(cell.id).unwrap();
        interrupted.provider_object = None;
        interrupted.phase = CellPhase::OverlayCreated;
        interrupted.state = CellState::Failed;
        engine.state.save_cell(&interrupted).unwrap();
        engine.provider.state.lock().unwrap().vm = None;

        assert!(matches!(
            engine.destroy_cell(cell.id),
            Err(EngineError::OwnershipNotProven(message))
                if message.contains("never durably recorded")
        ));
        assert!(engine.state.cell_runtime_root(cell.id).exists());
        assert_ne!(
            engine.state.load_cell(cell.id).unwrap().state,
            CellState::Destroyed
        );
    }

    #[test]
    fn provider_created_failure_can_be_claimed_then_exactly_destroyed() {
        let (_directory, engine, image_id) = fixture();
        engine.provider.state.lock().unwrap().fail_claim = true;
        assert!(engine.create_cell(spec(image_id)).is_err());
        let cell = engine.state.list_cells().unwrap().pop().unwrap();
        engine.provider.state.lock().unwrap().fail_claim = false;

        assert!(engine.destroy_cell(cell.id).unwrap().changed);
        assert!(engine.provider.state.lock().unwrap().vm.is_none());
        assert!(!engine.state.cell_runtime_root(cell.id).exists());
    }

    #[test]
    fn partially_configured_claimed_vm_is_exactly_destroyable() {
        let (_directory, engine, image_id) = fixture();
        engine
            .provider
            .state
            .lock()
            .unwrap()
            .fail_configure_after_network = true;
        assert!(engine.create_cell(spec(image_id)).is_err());
        let cell = engine.state.list_cells().unwrap().pop().unwrap();
        assert_eq!(cell.phase, CellPhase::ProviderObjectClaimed);

        assert!(engine.destroy_cell(cell.id).unwrap().changed);
        assert!(engine.provider.state.lock().unwrap().vm.is_none());
    }

    #[test]
    fn partial_destroy_intent_remains_retryable_after_crash() {
        let (_directory, engine, image_id) = fixture();
        engine
            .provider
            .state
            .lock()
            .unwrap()
            .fail_configure_after_network = true;
        assert!(engine.create_cell(spec(image_id)).is_err());
        let mut cell = engine.state.list_cells().unwrap().pop().unwrap();
        cell.state = CellState::Destroying;
        cell.phase = CellPhase::DestroyingProvisioning;
        engine.state.save_cell(&cell).unwrap();

        assert!(engine.destroy_cell(cell.id).unwrap().changed);
        assert!(engine.provider.state.lock().unwrap().vm.is_none());
    }

    #[test]
    fn destroyed_tombstone_detects_foreign_name_reuse() {
        let (_directory, engine, image_id) = fixture();
        let cell = engine.create_cell(spec(image_id)).unwrap();
        let mut replacement = engine.provider.state.lock().unwrap().vm.clone().unwrap();
        engine.destroy_cell(cell.id).unwrap();
        replacement.id = Uuid::new_v4().to_string();
        replacement.ownership_marker = "foreign".to_owned();
        engine.provider.state.lock().unwrap().vm = Some(replacement);

        assert!(matches!(
            engine.reconcile_cell(cell.id).unwrap().reconciliation,
            ReconciliationStatus::OwnershipMismatch { .. }
        ));
        assert!(engine.destroy_cell(cell.id).is_err());
        assert_eq!(engine.provider.state.lock().unwrap().remove_calls, 1);
    }

    #[test]
    fn destroyed_tombstone_detects_recreated_runtime_entry() {
        let (_directory, engine, image_id) = fixture();
        let cell = engine.create_cell(spec(image_id)).unwrap();
        engine.destroy_cell(cell.id).unwrap();
        engine.state.ensure_cell_runtime(cell.id).unwrap();

        assert!(matches!(
            engine.reconcile_cell(cell.id).unwrap().reconciliation,
            ReconciliationStatus::OwnershipMismatch { .. }
        ));
        assert!(engine.destroy_cell(cell.id).is_err());
    }

    #[test]
    fn destroyed_tombstone_stress_rejects_name_reuse_and_runtime_reappearance() {
        let (_directory, engine, image_id) = fixture();
        let cell = engine.create_cell(spec(image_id)).unwrap();
        let snapshot = engine.provider.state.lock().unwrap().vm.clone().unwrap();
        engine.destroy_cell(cell.id).unwrap();

        for iteration in 0..32 {
            let mut foreign = snapshot.clone();
            foreign.id = Uuid::new_v4().to_string();
            foreign.ownership_marker = format!("foreign-{iteration}");
            engine.provider.state.lock().unwrap().vm = Some(foreign);
            assert!(matches!(
                engine.reconcile_cell(cell.id).unwrap().reconciliation,
                ReconciliationStatus::OwnershipMismatch { .. }
            ));
            assert!(matches!(
                engine.destroy_cell(cell.id),
                Err(EngineError::ProviderDrift(_))
            ));
            engine.provider.state.lock().unwrap().vm = None;

            engine.state.ensure_cell_runtime(cell.id).unwrap();
            assert!(matches!(
                engine.reconcile_cell(cell.id).unwrap().reconciliation,
                ReconciliationStatus::OwnershipMismatch { .. }
            ));
            assert!(matches!(
                engine.destroy_cell(cell.id),
                Err(EngineError::ProviderDrift(_))
            ));
            let runtime = engine.state.pin_cell_runtime(cell.id).unwrap();
            engine.state.remove_cell_runtime(cell.id, runtime).unwrap();
        }

        assert!(!engine.destroy_cell(cell.id).unwrap().changed);
        assert_eq!(engine.provider.state.lock().unwrap().remove_calls, 1);
    }

    #[test]
    fn destroy_recovers_after_runtime_deleted_before_tombstone_commit() {
        let (_directory, engine, image_id) = fixture();
        let cell = engine.create_cell(spec(image_id)).unwrap();
        engine.provider.state.lock().unwrap().vm = None;
        let runtime = engine.state.pin_cell_runtime(cell.id).unwrap();
        engine.state.remove_cell_runtime(cell.id, runtime).unwrap();
        let mut interrupted = engine.state.load_cell(cell.id).unwrap();
        interrupted.state = CellState::Destroying;
        interrupted.phase = CellPhase::Destroying;
        engine.state.save_cell(&interrupted).unwrap();

        assert!(engine.destroy_cell(cell.id).unwrap().changed);
        assert_eq!(
            engine.state.load_cell(cell.id).unwrap().state,
            CellState::Destroyed
        );
    }

    #[test]
    fn destroy_recovers_when_runtime_is_partially_missing() {
        let (_directory, engine, image_id) = fixture();
        let cell = engine.create_cell(spec(image_id)).unwrap();
        engine.provider.state.lock().unwrap().vm = None;
        fs::remove_file(&cell.ownership.overlay_path).unwrap();

        assert!(engine.destroy_cell(cell.id).unwrap().changed);
        assert!(!engine.state.cell_runtime_root(cell.id).exists());
    }

    #[cfg(windows)]
    #[test]
    fn installation_identity_is_pinned_through_provider_mutation() {
        let (_directory, engine, image_id) = fixture();
        let cell = engine.create_cell(spec(image_id)).unwrap();
        engine
            .provider
            .rotate_installation_during_mutation(engine.state.root().join("installation.json"));

        engine.start_cell(cell.id).unwrap();

        assert!(
            engine
                .provider
                .state
                .lock()
                .unwrap()
                .installation_rotation_blocked
        );
        assert_eq!(
            engine.state.load_installation().unwrap().install_id,
            cell.ownership.install_id
        );
    }

    #[cfg(windows)]
    #[test]
    fn reparse_descendant_blocks_provider_mutation() {
        let (directory, engine, image_id) = fixture();
        let cell = engine.create_cell(spec(image_id)).unwrap();
        let external = directory.path().join("external-runtime");
        fs::create_dir_all(&external).unwrap();
        let link = engine.state.cell_runtime_root(cell.id).join("redirect");
        if std::os::windows::fs::symlink_dir(&external, &link).is_err() {
            return;
        }
        let starts_before = engine
            .provider
            .state
            .lock()
            .unwrap()
            .calls
            .iter()
            .filter(|call| **call == "start_vm")
            .count();

        assert!(matches!(
            engine.start_cell(cell.id),
            Err(EngineError::State(StateError::UnsafeRuntimePath(_)))
        ));
        let starts_after = engine
            .provider
            .state
            .lock()
            .unwrap()
            .calls
            .iter()
            .filter(|call| **call == "start_vm")
            .count();
        assert_eq!(starts_before, starts_after);
    }
}
