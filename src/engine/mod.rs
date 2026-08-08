use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use chrono::{Duration, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;
use uuid::Uuid;

use crate::core::cell::{CellId, CellPhase, CellRecord, CellSpec, CellState};
use crate::core::image::{Architecture, GuestOs, ImageBinding, ImageId, ImageRecord, ImageVariant};
use crate::core::ownership::{CellOwnership, ProviderObjectIdentity};
use crate::providers::{
    CreateOverlayRequest, CreateVmRequest, LocalVmProvider, ProviderError, ProviderImageInfo,
    ProviderPowerState, ProviderVm, VmLookup,
};
use crate::state::{StateError, StateStore};

const IMAGE_SCHEMA_VERSION: u32 = 1;
const CELL_SCHEMA_VERSION: u32 = 1;
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
        self.require_hyperv()?;
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

    pub fn create_cell(&self, spec: CellSpec) -> Result<CellRecord, EngineError> {
        self.require_hyperv()?;
        validate_cell_spec(&spec)?;
        let _guard = self.state.acquire_mutation_lock()?;
        let image_record = self.state.load_image(&spec.image)?;
        let variant = hyperv_variant(&image_record)?;
        let parent_handle = self.verify_registered_image(variant)?;

        let installation = self.state.installation()?;
        let cell_id = CellId::new();
        self.state.ensure_cell_runtime(cell_id)?;
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

        let overlay = match self.provider.create_overlay(&CreateOverlayRequest {
            parent_path: record.image.path.clone(),
            overlay_path: record.ownership.overlay_path.clone(),
        }) {
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

        let provider_vm = match self.provider.create_vm(&CreateVmRequest {
            name: record.ownership.provider_object_name.clone(),
            configuration_path: record.ownership.configuration_path.clone(),
            overlay_path: record.ownership.overlay_path.clone(),
            ownership_marker: record.ownership.provider_marker.clone(),
            cpu_count: record.spec.cpu_count,
            memory_mib: record.spec.memory_mib,
        }) {
            Ok(provider_vm) => provider_vm,
            Err(error) => return self.fail_record(record, error.into()),
        };
        record.provider_object = Some(ProviderObjectIdentity {
            id: provider_vm.id.clone(),
            name: provider_vm.name.clone(),
        });
        record.phase = CellPhase::ProviderObjectCreated;
        record.updated_at = Utc::now();
        self.state.save_cell(&record)?;

        if let Err(error) = prove_ownership(&record, &provider_vm) {
            return self.fail_record(record, error);
        }
        if provider_vm.power_state != ProviderPowerState::Off {
            return self.fail_record(
                record,
                EngineError::UnexpectedPowerState(provider_vm.power_state),
            );
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
                self.provider.start_vm(&before.id)?;
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
                self.provider.stop_vm(&before.id)?;
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
        let _guard = self.state.acquire_mutation_lock()?;
        let mut record = self.state.load_cell(cell_id)?;
        if record.state == CellState::Destroyed {
            return Ok(operation_report(&record, false));
        }

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
            None
        };

        if let Some(mut vm) = provider_vm {
            prove_ownership(&record, &vm)?;
            record.state = CellState::Destroying;
            record.phase = CellPhase::Destroying;
            record.updated_at = Utc::now();
            self.state.save_cell(&record)?;

            if vm.power_state != ProviderPowerState::Off {
                self.provider.stop_vm(&vm.id)?;
                vm = self
                    .provider
                    .inspect_vm(&VmLookup::Id(vm.id.clone()))?
                    .ok_or_else(|| {
                        EngineError::ProviderDrift(
                            "provider object disappeared while stopping".to_owned(),
                        )
                    })?;
                prove_ownership(&record, &vm)?;
                if vm.power_state != ProviderPowerState::Off {
                    return Err(EngineError::UnexpectedPowerState(vm.power_state));
                }
            }
            self.provider.remove_vm(&vm.id)?;
            if self
                .provider
                .inspect_vm(&VmLookup::Id(vm.id.clone()))?
                .is_some()
            {
                return Err(EngineError::ProviderDrift(
                    "provider object still exists after remove".to_owned(),
                ));
            }
        } else if let Some(identity) = &record.provider_object {
            if self
                .provider
                .inspect_vm(&VmLookup::Name(identity.name.clone()))?
                .is_some()
            {
                return Err(EngineError::OwnershipNotProven(
                    "recorded provider id is absent but the name is occupied".to_owned(),
                ));
            }
        }

        self.state.remove_cell_runtime(cell_id)?;
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
        if record.state == CellState::Destroyed {
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

    fn fail_record<T>(&self, mut record: CellRecord, error: EngineError) -> Result<T, EngineError> {
        record.state = CellState::Failed;
        record.updated_at = Utc::now();
        record.last_error = Some(error.to_string());
        self.state.save_cell(&record)?;
        Err(error)
    }
}

fn validate_cell_spec(spec: &CellSpec) -> Result<(), EngineError> {
    if spec
        .provider
        .as_deref()
        .is_some_and(|value| value != "hyperv")
    {
        return Err(EngineError::UnsupportedProvider(
            spec.provider.clone().unwrap_or_default(),
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

fn prove_ownership(record: &CellRecord, vm: &ProviderVm) -> Result<(), EngineError> {
    let reasons = ownership_drift(record, vm);
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
        left.to_string_lossy()
            .trim_end_matches(['\\', '/'])
            .eq_ignore_ascii_case(right.to_string_lossy().trim_end_matches(['\\', '/']))
    }
    #[cfg(not(windows))]
    {
        left == right
    }
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
    use std::sync::Mutex;

    use tempfile::tempdir;

    use super::*;
    use crate::core::capability::ProviderCapabilities;
    use crate::providers::ProviderProbe;

    #[derive(Debug, Default)]
    struct MockState {
        vm: Option<ProviderVm>,
        calls: Vec<&'static str>,
        remove_calls: usize,
    }

    struct MockHyperV {
        base_size: u64,
        state: Mutex<MockState>,
    }

    impl MockHyperV {
        fn new(base_path: PathBuf) -> Self {
            let base_size = fs::metadata(&base_path).unwrap().len();
            Self {
                base_size,
                state: Mutex::new(MockState::default()),
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
                path,
                disk_format: "vhdx".to_owned(),
                disk_type: "dynamic".to_owned(),
                parent_path: None,
                file_size: self.base_size,
                virtual_size: 1024 * 1024,
            })
        }

        fn create_overlay(
            &self,
            request: &CreateOverlayRequest,
        ) -> Result<ProviderImageInfo, ProviderError> {
            self.state.lock().unwrap().calls.push("create_overlay");
            fs::write(&request.overlay_path, b"overlay").unwrap();
            Ok(ProviderImageInfo {
                path: request.overlay_path.clone(),
                disk_format: "vhdx".to_owned(),
                disk_type: "differencing".to_owned(),
                parent_path: Some(request.parent_path.clone()),
                file_size: 7,
                virtual_size: 1024 * 1024,
            })
        }

        fn create_vm(&self, request: &CreateVmRequest) -> Result<ProviderVm, ProviderError> {
            let mut state = self.state.lock().unwrap();
            state.calls.push("create_vm");
            let vm = ProviderVm {
                id: Uuid::new_v4().to_string(),
                name: request.name.clone(),
                power_state: ProviderPowerState::Off,
                ownership_marker: request.ownership_marker.clone(),
                configuration_path: request.configuration_path.clone(),
                attached_disks: vec![request.overlay_path.clone()],
                network_adapter_count: 0,
                cpu_count: request.cpu_count,
                memory_mib: request.memory_mib,
            };
            state.vm = Some(vm.clone());
            Ok(vm)
        }

        fn inspect_vm(&self, lookup: &VmLookup) -> Result<Option<ProviderVm>, ProviderError> {
            let mut state = self.state.lock().unwrap();
            state.calls.push("inspect_vm");
            Ok(state.vm.clone().filter(|vm| match lookup {
                VmLookup::Id(id) => vm.id == *id,
                VmLookup::Name(name) => vm.name == *name,
            }))
        }

        fn start_vm(&self, id: &str) -> Result<(), ProviderError> {
            let mut state = self.state.lock().unwrap();
            state.calls.push("start_vm");
            let vm = state
                .vm
                .as_mut()
                .ok_or_else(|| ProviderError::NotFound(id.to_owned()))?;
            vm.power_state = ProviderPowerState::Running;
            Ok(())
        }

        fn stop_vm(&self, id: &str) -> Result<(), ProviderError> {
            let mut state = self.state.lock().unwrap();
            state.calls.push("stop_vm");
            let vm = state
                .vm
                .as_mut()
                .ok_or_else(|| ProviderError::NotFound(id.to_owned()))?;
            vm.power_state = ProviderPowerState::Off;
            Ok(())
        }

        fn remove_vm(&self, id: &str) -> Result<(), ProviderError> {
            let mut state = self.state.lock().unwrap();
            state.calls.push("remove_vm");
            if state.vm.as_ref().is_none_or(|vm| vm.id != id) {
                return Err(ProviderError::NotFound(id.to_owned()));
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
}
