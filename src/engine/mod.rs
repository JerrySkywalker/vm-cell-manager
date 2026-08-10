use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration as StdDuration, Instant};

use chrono::{Duration, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;
use uuid::Uuid;

use crate::core::automation::{AUTOMATION_SCHEMA_VERSION, OwnershipClassification, RequiredAction};
use crate::core::cell::{CELL_SCHEMA_VERSION, CellId, CellPhase, CellRecord, CellSpec, CellState};
use crate::core::guest::{
    ARTIFACT_SCHEMA_VERSION, ArtifactEntry, ArtifactRecord, GuestFailureClass, GuestOperationId,
    GuestOperationKind, GuestOperationPhase, GuestOperationRecord, MAX_ARTIFACT_FILES,
    MAX_ARTIFACT_TOTAL_BYTES,
};
use crate::core::image::{
    Architecture, GuestOs, IMAGE_SCHEMA_VERSION, ImageBinding, ImageId, ImageRecord, ImageVariant,
};
use crate::core::ownership::{CellOwnership, OWNERSHIP_MARKER_SCHEMA, ProviderObjectIdentity};
use crate::guest::{
    GuestActionAuthority, GuestCommand, GuestCommandResult, GuestCopyInAction, GuestCopyOutAction,
    GuestCredentials, GuestIoError, GuestPath, GuestReadiness, GuestTransport, MAX_COPY_BYTES,
    OverwritePolicy, ReadinessPolicy,
};
use crate::providers::{
    ClaimVmRequest, ConfigureVmRequest, CreateOverlayRequest, CreateVmRequest, LocalVmProvider,
    ProviderError, ProviderImageInfo, ProviderMutationAuthority, ProviderPowerState, ProviderVm,
    ProviderVmIdentity, VmLookup,
};
use crate::state::{InstallationAuthority, MutationGuard, StateError, StateStore};

const MIN_MEMORY_MIB: u64 = 512;
const MAX_MEMORY_MIB: u64 = 1_048_576;
const MAX_CPU_COUNT: u16 = 64;
const MIN_TTL_SECONDS: u64 = 1;
const MAX_TTL_SECONDS: u64 = 31_536_000;
const MAX_ARTIFACT_RETENTION_SECONDS: u64 = 31_536_000;
const MAX_ARTIFACT_PRUNE_BATCH: usize = 256;

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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidateImageRequest {
    pub guest_os: GuestOs,
    pub guest_arch: Architecture,
    pub path: PathBuf,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ImageValidationStatus {
    Usable,
    Unusable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ImageValidationIssue {
    UnsupportedExtension,
    PathUnavailableOrUnsafe,
    ContentReadFailed,
    ProviderPathMismatch,
    ProviderFormatMismatch,
    BackingParentPresent,
    DifferencingBase,
    ProviderSizeMismatch,
    RegisteredPathDrift,
    RegisteredFormatDrift,
    RegisteredSizeDrift,
    RegisteredHashDrift,
    RegisteredVariantMissing,
}

impl ImageValidationIssue {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::UnsupportedExtension => "unsupported_extension",
            Self::PathUnavailableOrUnsafe => "path_unavailable_or_unsafe",
            Self::ContentReadFailed => "content_read_failed",
            Self::ProviderPathMismatch => "provider_path_mismatch",
            Self::ProviderFormatMismatch => "provider_format_mismatch",
            Self::BackingParentPresent => "backing_parent_present",
            Self::DifferencingBase => "differencing_base",
            Self::ProviderSizeMismatch => "provider_size_mismatch",
            Self::RegisteredPathDrift => "registered_path_drift",
            Self::RegisteredFormatDrift => "registered_format_drift",
            Self::RegisteredSizeDrift => "registered_size_drift",
            Self::RegisteredHashDrift => "registered_hash_drift",
            Self::RegisteredVariantMissing => "registered_variant_missing",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImageValidationReport {
    pub schema_version: u32,
    pub image_id: Option<ImageId>,
    pub registered: bool,
    pub provider: String,
    pub guest_os: GuestOs,
    pub guest_arch: Architecture,
    pub path: PathBuf,
    pub expected_format: String,
    pub observed_format: Option<String>,
    pub disk_type: Option<String>,
    pub parent_path: Option<PathBuf>,
    pub file_size: Option<u64>,
    pub virtual_size: Option<u64>,
    pub sha256: Option<String>,
    pub registered_sha256: Option<String>,
    pub status: ImageValidationStatus,
    pub issues: Vec<ImageValidationIssue>,
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
    pub classification: ReconciliationClassification,
    pub reconciliation: ReconciliationStatus,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GuestExecRequest {
    pub cell_id: CellId,
    pub command: GuestCommand,
    pub readiness: ReadinessPolicy,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GuestExecReport {
    pub schema_version: u32,
    pub operation_id: GuestOperationId,
    pub cell_id: CellId,
    pub result: GuestCommandResult,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RunCleanupPolicy {
    pub keep: bool,
    pub keep_on_failure: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunCellRequest {
    pub spec: CellSpec,
    pub command: GuestCommand,
    pub readiness: ReadinessPolicy,
    pub cleanup: RunCleanupPolicy,
}

#[derive(Debug, Clone, Copy)]
struct GuestOperationPlan {
    kind: GuestOperationKind,
    readiness: ReadinessPolicy,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunOutcome {
    Success,
    GuestNonZero,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunCleanupDisposition {
    NothingCreated,
    Destroyed,
    RetainedByRequest,
    RetainedOnFailure,
    RefusedAmbiguous,
    Failed,
}

impl RunCleanupDisposition {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::NothingCreated => "nothing_created",
            Self::Destroyed => "destroyed",
            Self::RetainedByRequest => "retained_by_request",
            Self::RetainedOnFailure => "retained_on_failure",
            Self::RefusedAmbiguous => "refused_ambiguous",
            Self::Failed => "failed",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunStage {
    RequestValidation,
    ImageValidation,
    CellCreation,
    ProviderStart,
    GuestReadiness,
    GuestExecution,
    Cleanup,
    Interrupted,
}

impl RunStage {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::RequestValidation => "request_validation",
            Self::ImageValidation => "image_validation",
            Self::CellCreation => "cell_creation",
            Self::ProviderStart => "provider_start",
            Self::GuestReadiness => "guest_readiness",
            Self::GuestExecution => "guest_execution",
            Self::Cleanup => "cleanup",
            Self::Interrupted => "interrupted",
        }
    }
}

impl ImageValidationStatus {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Usable => "usable",
            Self::Unusable => "unusable",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunCellReport {
    pub schema_version: u32,
    pub cell_id: CellId,
    pub operation_id: GuestOperationId,
    pub outcome: RunOutcome,
    pub result: GuestCommandResult,
    pub cleanup: RunCleanupDisposition,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunFailureReport {
    pub schema_version: u32,
    pub cell_id: Option<CellId>,
    pub operation_id: Option<GuestOperationId>,
    pub stage: RunStage,
    pub cleanup: RunCleanupDisposition,
    pub error_code: String,
    pub cleanup_error_code: Option<String>,
    pub result: Option<GuestCommandResult>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunProgressEvent {
    ImageVerified {
        image: ImageId,
    },
    CellCreated {
        cell_id: CellId,
    },
    ProviderStarted {
        cell_id: CellId,
    },
    GuestReady {
        cell_id: CellId,
    },
    CommandCompleted {
        cell_id: CellId,
        exit_code: i32,
    },
    CleanupStarted {
        cell_id: CellId,
    },
    CellDestroyed {
        cell_id: CellId,
    },
    CellRetained {
        cell_id: CellId,
        disposition: RunCleanupDisposition,
    },
    CleanupRefused {
        cell_id: CellId,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunControl {
    Continue,
    Cancel,
}

pub trait RunObserver {
    fn observe(&mut self, event: &RunProgressEvent) -> RunControl;
}

impl<F> RunObserver for F
where
    F: FnMut(&RunProgressEvent) -> RunControl,
{
    fn observe(&mut self, event: &RunProgressEvent) -> RunControl {
        self(event)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GuestCopyInRequest {
    pub cell_id: CellId,
    pub source: PathBuf,
    pub destination: GuestPath,
    pub overwrite: OverwritePolicy,
    pub timeout: StdDuration,
    pub max_bytes: u64,
    pub readiness: ReadinessPolicy,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GuestCopyOutRequest {
    pub cell_id: CellId,
    pub source: GuestPath,
    pub timeout: StdDuration,
    pub max_bytes: u64,
    pub readiness: ReadinessPolicy,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtifactCollectRequest {
    pub cell_id: CellId,
    pub sources: Vec<GuestPath>,
    pub timeout: StdDuration,
    pub max_bytes_per_file: u64,
    pub readiness: ReadinessPolicy,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GuestCopyInReport {
    pub schema_version: u32,
    pub operation_id: GuestOperationId,
    pub cell_id: CellId,
    pub guest_path: GuestPath,
    pub size: u64,
    pub overwrite: OverwritePolicy,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArtifactReport {
    pub schema_version: u32,
    pub operation_id: GuestOperationId,
    pub cell_id: CellId,
    pub artifact: ArtifactRecord,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtifactPruneRequest {
    pub older_than: StdDuration,
    pub max_artifacts: usize,
    pub dry_run: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArtifactPruneReport {
    pub schema_version: u32,
    pub evaluated_at: chrono::DateTime<Utc>,
    pub cutoff: chrono::DateTime<Utc>,
    pub dry_run: bool,
    pub entries: Vec<ArtifactPruneEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArtifactPruneEntry {
    pub cell_id: CellId,
    pub operation_id: GuestOperationId,
    pub bytes: u64,
    pub disposition: ArtifactPruneDisposition,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactPruneDisposition {
    Eligible,
    Pruned,
    RecoveryCompleted,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GcReport {
    pub schema_version: u32,
    pub evaluated_at: chrono::DateTime<Utc>,
    pub entries: Vec<GcEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GcEntry {
    pub cell_id: CellId,
    pub expires_at: Option<chrono::DateTime<Utc>>,
    pub disposition: GcDisposition,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GuestOperationRecoveryDisposition {
    AlreadyTerminal,
    InterruptedBeforeTransport,
    ArtifactCompletionRecovered,
    RecoveryRequired,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GuestOperationRecoveryReport {
    pub schema_version: u32,
    pub operation: GuestOperationRecord,
    pub disposition: GuestOperationRecoveryDisposition,
    pub required_action: RequiredAction,
    pub changed: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GcDisposition {
    NoTtl,
    NotExpired,
    InFlightGuestOperation,
    Destroyed,
    AlreadyDestroyed,
    OwnershipMismatch,
    Failed,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReconciliationCode {
    ExactOwned,
    ManifestOnly,
    ProviderMissing,
    UnprovenProviderObject,
    OwnershipMismatch,
    StateDrift,
    Provisioning,
    Destroyed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReconciliationClassification {
    pub code: ReconciliationCode,
    pub ownership: OwnershipClassification,
    pub required_action: RequiredAction,
}

impl ReconciliationStatus {
    #[must_use]
    pub const fn classification(&self) -> ReconciliationClassification {
        match self {
            Self::ExactOwned => ReconciliationClassification {
                code: ReconciliationCode::ExactOwned,
                ownership: OwnershipClassification::Proven,
                required_action: RequiredAction::None,
            },
            Self::ManifestOnly => ReconciliationClassification {
                code: ReconciliationCode::ManifestOnly,
                ownership: OwnershipClassification::Unproven,
                required_action: RequiredAction::ManualReview,
            },
            Self::ProviderMissing => ReconciliationClassification {
                code: ReconciliationCode::ProviderMissing,
                ownership: OwnershipClassification::Unproven,
                required_action: RequiredAction::ManualReview,
            },
            Self::UnprovenProviderObject { .. } => ReconciliationClassification {
                code: ReconciliationCode::UnprovenProviderObject,
                ownership: OwnershipClassification::Unproven,
                required_action: RequiredAction::ManualReview,
            },
            Self::OwnershipMismatch { .. } => ReconciliationClassification {
                code: ReconciliationCode::OwnershipMismatch,
                ownership: OwnershipClassification::Mismatch,
                required_action: RequiredAction::ManualReview,
            },
            Self::StateDrift { .. } => ReconciliationClassification {
                code: ReconciliationCode::StateDrift,
                ownership: OwnershipClassification::Proven,
                required_action: RequiredAction::RetryLifecycle,
            },
            Self::Provisioning { .. } => ReconciliationClassification {
                code: ReconciliationCode::Provisioning,
                ownership: OwnershipClassification::PhaseProven,
                required_action: RequiredAction::RecoveryRequired,
            },
            Self::Destroyed => ReconciliationClassification {
                code: ReconciliationCode::Destroyed,
                ownership: OwnershipClassification::NotApplicable,
                required_action: RequiredAction::None,
            },
        }
    }
}

#[derive(Debug, Error)]
pub enum EngineError {
    #[error(transparent)]
    State(#[from] StateError),

    #[error(transparent)]
    Provider(#[from] ProviderError),

    #[error("unsupported local provider: {0}")]
    UnsupportedProvider(String),

    #[error("selected provider is unavailable: {0}")]
    ProviderUnavailable(String),

    #[error("invalid image: {0}")]
    InvalidImage(String),

    #[error("registered image integrity check failed: {0}")]
    ImageIntegrity(String),

    #[error("image id is already registered with different identity: {0}")]
    ImageConflict(ImageId),

    #[error("invalid cell request: {0}")]
    InvalidCellRequest(String),

    #[error("cell lifecycle conflicts with the requested operation: {0}")]
    LifecycleConflict(String),

    #[error("persisted cell integrity check failed: {0}")]
    Integrity(String),

    #[error("ownership is not proven: {0}")]
    OwnershipNotProven(String),

    #[error("provider object drift: {0}")]
    ProviderDrift(String),

    #[error("unexpected provider power state: {0:?}")]
    UnexpectedPowerState(ProviderPowerState),

    #[error(transparent)]
    Guest(#[from] GuestIoError),
}

#[derive(Debug, Error)]
#[error("run failed at {stage:?}", stage = report.stage)]
pub struct RunCellError {
    report: Box<RunFailureReport>,
    #[source]
    source: Box<EngineError>,
}

impl RunCellError {
    #[must_use]
    pub fn report(&self) -> &RunFailureReport {
        &self.report
    }

    #[must_use]
    pub fn engine_error(&self) -> &EngineError {
        &self.source
    }
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

    #[must_use]
    pub fn provider_name(&self) -> &'static str {
        self.provider.name()
    }

    pub fn register_image(
        &self,
        request: RegisterImageRequest,
    ) -> Result<ImageRecord, EngineError> {
        self.require_provider_available()?;
        let _mutation = self.state.acquire_mutation_lock()?;
        let validation = self.validate_image_path(
            None,
            request.guest_os,
            request.guest_arch,
            request.path,
            None,
        )?;
        if validation.status != ImageValidationStatus::Usable {
            let issues = validation
                .issues
                .iter()
                .map(|issue| issue.as_str())
                .collect::<Vec<_>>()
                .join(",");
            return Err(EngineError::InvalidImage(format!(
                "image validation failed: {issues}"
            )));
        }

        let variant = ImageVariant {
            provider: self.provider.name().to_owned(),
            disk_format: validation.observed_format.ok_or_else(|| {
                EngineError::InvalidImage("image format was not proven".to_owned())
            })?,
            path: validation.path,
            sha256: validation
                .sha256
                .ok_or_else(|| EngineError::InvalidImage("image hash was not proven".to_owned()))?,
            file_size: validation
                .file_size
                .ok_or_else(|| EngineError::InvalidImage("image size was not proven".to_owned()))?,
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

    pub fn validate_image(
        &self,
        request: ValidateImageRequest,
    ) -> Result<ImageValidationReport, EngineError> {
        self.require_provider_available()?;
        self.validate_image_path(
            None,
            request.guest_os,
            request.guest_arch,
            request.path,
            None,
        )
    }

    pub fn validate_registered_image(
        &self,
        image_id: &ImageId,
    ) -> Result<ImageValidationReport, EngineError> {
        self.require_provider_available()?;
        let record = self.state.load_image(image_id)?;
        let variants = record
            .variants
            .iter()
            .filter(|variant| variant.provider == self.provider.name())
            .collect::<Vec<_>>();
        if variants.len() != 1 {
            return Ok(ImageValidationReport {
                schema_version: AUTOMATION_SCHEMA_VERSION,
                image_id: Some(record.id),
                registered: true,
                provider: self.provider.name().to_owned(),
                guest_os: record.guest_os,
                guest_arch: record.guest_arch,
                path: variants
                    .first()
                    .map_or_else(PathBuf::new, |variant| variant.path.clone()),
                expected_format: provider_image_format(self.provider.name())?.to_owned(),
                observed_format: None,
                disk_type: None,
                parent_path: None,
                file_size: None,
                virtual_size: None,
                sha256: None,
                registered_sha256: variants.first().map(|variant| variant.sha256.clone()),
                status: ImageValidationStatus::Unusable,
                issues: vec![ImageValidationIssue::RegisteredVariantMissing],
            });
        }
        let variant = variants[0];
        self.validate_image_path(
            Some(record.id),
            record.guest_os,
            record.guest_arch,
            variant.path.clone(),
            Some(variant),
        )
    }

    fn validate_image_path(
        &self,
        image_id: Option<ImageId>,
        guest_os: GuestOs,
        guest_arch: Architecture,
        path: PathBuf,
        registered_variant: Option<&ImageVariant>,
    ) -> Result<ImageValidationReport, EngineError> {
        let expected_format = provider_image_format(self.provider.name())?;
        let mut report = ImageValidationReport {
            schema_version: AUTOMATION_SCHEMA_VERSION,
            image_id,
            registered: registered_variant.is_some(),
            provider: self.provider.name().to_owned(),
            guest_os,
            guest_arch,
            path: path.clone(),
            expected_format: expected_format.to_owned(),
            observed_format: None,
            disk_type: None,
            parent_path: None,
            file_size: None,
            virtual_size: None,
            sha256: None,
            registered_sha256: registered_variant.map(|variant| variant.sha256.clone()),
            status: ImageValidationStatus::Unusable,
            issues: Vec::new(),
        };
        if !path
            .extension()
            .and_then(|value| value.to_str())
            .is_some_and(|value| value.eq_ignore_ascii_case(expected_format))
        {
            report
                .issues
                .push(ImageValidationIssue::UnsupportedExtension);
            return Ok(report);
        }
        let canonical = match canonical_image_path(&path) {
            Ok(canonical) => canonical,
            Err(_) => {
                report
                    .issues
                    .push(ImageValidationIssue::PathUnavailableOrUnsafe);
                return Ok(report);
            }
        };
        report.path = canonical.clone();
        let mut handle = match open_immutable_parent(&canonical) {
            Ok(handle) => handle,
            Err(_) => {
                report
                    .issues
                    .push(ImageValidationIssue::PathUnavailableOrUnsafe);
                return Ok(report);
            }
        };
        let file_size = match handle.file.metadata() {
            Ok(metadata) => metadata.len(),
            Err(_) => {
                report.issues.push(ImageValidationIssue::ContentReadFailed);
                return Ok(report);
            }
        };
        let provider_info = self.provider.inspect_image(canonical.clone())?;
        report.observed_format = Some(provider_info.disk_format.clone());
        report.disk_type = Some(provider_info.disk_type.clone());
        report.parent_path = provider_info.parent_path.clone();
        report.file_size = Some(file_size);
        report.virtual_size = Some(provider_info.virtual_size);
        if !paths_equal(&canonical, &provider_info.path) {
            report
                .issues
                .push(ImageValidationIssue::ProviderPathMismatch);
        }
        if !provider_info
            .disk_format
            .eq_ignore_ascii_case(expected_format)
        {
            report
                .issues
                .push(ImageValidationIssue::ProviderFormatMismatch);
        }
        if provider_info.parent_path.is_some() {
            report
                .issues
                .push(ImageValidationIssue::BackingParentPresent);
        }
        if provider_info.disk_type.eq_ignore_ascii_case("differencing")
            || provider_info.disk_type.eq_ignore_ascii_case("overlay")
        {
            report.issues.push(ImageValidationIssue::DifferencingBase);
        }
        if provider_info.file_size != file_size {
            report
                .issues
                .push(ImageValidationIssue::ProviderSizeMismatch);
        }
        match sha256_file(&mut handle.file) {
            Ok(sha256) => report.sha256 = Some(sha256),
            Err(_) => report.issues.push(ImageValidationIssue::ContentReadFailed),
        }
        if let Some(variant) = registered_variant {
            if !paths_equal(&canonical, &variant.path) {
                report
                    .issues
                    .push(ImageValidationIssue::RegisteredPathDrift);
            }
            if !provider_info
                .disk_format
                .eq_ignore_ascii_case(&variant.disk_format)
            {
                report
                    .issues
                    .push(ImageValidationIssue::RegisteredFormatDrift);
            }
            if file_size != variant.file_size {
                report
                    .issues
                    .push(ImageValidationIssue::RegisteredSizeDrift);
            }
            if report.sha256.as_deref() != Some(variant.sha256.as_str()) {
                report
                    .issues
                    .push(ImageValidationIssue::RegisteredHashDrift);
            }
        }
        if report.issues.is_empty() {
            report.status = ImageValidationStatus::Usable;
        }
        Ok(report)
    }

    pub fn list_cells(&self) -> Result<Vec<CellRecord>, EngineError> {
        Ok(self.state.list_cells()?)
    }

    pub fn reconcile_all(&self) -> Result<Vec<CellInspection>, EngineError> {
        self.state
            .list_cells()?
            .into_iter()
            .filter(|record| record.provider == self.provider.name())
            .map(|record| self.inspect_cell(record.id))
            .collect()
    }

    pub fn create_cell(&self, spec: CellSpec) -> Result<CellRecord, EngineError> {
        self.create_cell_with_id(spec, CellId::new())
    }

    fn create_cell_with_id(
        &self,
        spec: CellSpec,
        cell_id: CellId,
    ) -> Result<CellRecord, EngineError> {
        self.require_provider_available()?;
        validate_cell_spec(&spec, self.provider.name())?;
        let mutation = self.state.acquire_mutation_lock()?;
        let image_record = self.state.load_image(&spec.image)?;
        let variant = provider_variant(&image_record, self.provider.name())?;
        let parent_handle = self.verify_registered_image(variant)?;

        let installation = self.state.installation()?;
        let installation_authority = self.state.acquire_installation_authority()?;
        if installation != *installation_authority.record() {
            return Err(EngineError::OwnershipNotProven(
                "installation identity changed while creating a cell".to_owned(),
            ));
        }
        let now = Utc::now();
        let ownership = CellOwnership::new(
            installation.install_id,
            cell_id,
            Uuid::new_v4(),
            self.state
                .cell_configuration_path_for(cell_id, self.provider.name()),
            self.state
                .cell_overlay_path_for(cell_id, &variant.disk_format),
        );
        let expires_at = spec
            .ttl_seconds
            .map(|seconds| now + Duration::seconds(i64::try_from(seconds).unwrap_or(i64::MAX)));
        let mut record = CellRecord {
            schema_version: CELL_SCHEMA_VERSION,
            id: cell_id,
            provider: self.provider.name().to_owned(),
            spec,
            image: ImageBinding::from_variant(
                image_record.id.clone(),
                image_record.guest_os,
                variant,
            ),
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
        let runtime_authority = match self.state.prepare_cell_runtime_for(
            cell_id,
            record.ownership.configuration_path.clone(),
            record.ownership.overlay_path.clone(),
        ) {
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
        let authority = ProviderMutationAuthority::new(
            &record,
            &installation_authority,
            &runtime_authority,
            &mutation,
        );
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
            parent_path: record.image.path.clone(),
            memory_mib: record.spec.memory_mib,
            cpu_count: record.spec.cpu_count,
            accelerator: record.spec.accelerator.clone(),
            allow_tcg: record.spec.allow_tcg,
        };
        let authority = ProviderMutationAuthority::new(
            &record,
            &installation_authority,
            &runtime_authority,
            &mutation,
        );
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
        let authority = ProviderMutationAuthority::new(
            &record,
            &installation_authority,
            &runtime_authority,
            &mutation,
        );
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
        let authority = ProviderMutationAuthority::new(
            &record,
            &installation_authority,
            &runtime_authority,
            &mutation,
        );
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
        let classification = reconciliation.classification();
        Ok(CellInspection {
            schema_version: AUTOMATION_SCHEMA_VERSION,
            cell: record,
            provider_vm,
            classification,
            reconciliation,
        })
    }

    pub fn reconcile_cell(&self, cell_id: CellId) -> Result<CellInspection, EngineError> {
        self.inspect_cell(cell_id)
    }

    pub fn start_cell(&self, cell_id: CellId) -> Result<OperationReport, EngineError> {
        self.require_provider()?;
        let mutation = self.state.acquire_mutation_lock()?;
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
                let _immutable_parent = if record.provider == "qemu" {
                    Some(self.verify_registered_image(&ImageVariant {
                        provider: record.image.provider.clone(),
                        disk_format: record.image.disk_format.clone(),
                        path: record.image.path.clone(),
                        sha256: record.image.sha256.clone(),
                        file_size: record.image.file_size,
                    })?)
                } else {
                    None
                };
                let installation = self.state.acquire_installation_authority()?;
                self.validate_local_ownership_against(&record, &installation)?;
                let runtime = self.state.pin_cell_runtime_for(
                    record.id,
                    record.ownership.configuration_path.clone(),
                    record.ownership.overlay_path.clone(),
                )?;
                let authority =
                    ProviderMutationAuthority::new(&record, &installation, &runtime, &mutation);
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
            ProviderPowerState::Paused
                if record.provider == "qemu" && record.state != CellState::Destroyed =>
            {
                let _immutable_parent = self.verify_registered_image(&ImageVariant {
                    provider: record.image.provider.clone(),
                    disk_format: record.image.disk_format.clone(),
                    path: record.image.path.clone(),
                    sha256: record.image.sha256.clone(),
                    file_size: record.image.file_size,
                })?;
                let installation = self.state.acquire_installation_authority()?;
                self.validate_local_ownership_against(&record, &installation)?;
                let runtime = self.state.pin_cell_runtime_for(
                    record.id,
                    record.ownership.configuration_path.clone(),
                    record.ownership.overlay_path.clone(),
                )?;
                let authority =
                    ProviderMutationAuthority::new(&record, &installation, &runtime, &mutation);
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
        self.require_provider()?;
        let mutation = self.state.acquire_mutation_lock()?;
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
                let runtime = self.state.pin_cell_runtime_for(
                    record.id,
                    record.ownership.configuration_path.clone(),
                    record.ownership.overlay_path.clone(),
                )?;
                let authority =
                    ProviderMutationAuthority::new(&record, &installation, &runtime, &mutation);
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

    pub fn run_cell<G: GuestTransport>(
        &self,
        transport: &G,
        credentials: &GuestCredentials,
        request: RunCellRequest,
    ) -> Result<RunCellReport, RunCellError> {
        let mut observer = |_event: &RunProgressEvent| RunControl::Continue;
        self.run_cell_observed(transport, credentials, request, &mut observer)
    }

    pub fn run_cell_observed<G: GuestTransport, O: RunObserver>(
        &self,
        transport: &G,
        credentials: &GuestCredentials,
        request: RunCellRequest,
        observer: &mut O,
    ) -> Result<RunCellReport, RunCellError> {
        if let Err(error) = request.command.validate().map_err(EngineError::from) {
            return Err(run_cell_error(
                None,
                None,
                RunStage::RequestValidation,
                RunCleanupDisposition::NothingCreated,
                error,
                None,
                None,
            ));
        }
        if let Err(error) = validate_readiness_policy(request.readiness) {
            return Err(run_cell_error(
                None,
                None,
                RunStage::RequestValidation,
                RunCleanupDisposition::NothingCreated,
                error,
                None,
                None,
            ));
        }
        if let Err(error) = validate_cell_spec(&request.spec, self.provider.name()) {
            return Err(run_cell_error(
                None,
                None,
                RunStage::RequestValidation,
                RunCleanupDisposition::NothingCreated,
                error,
                None,
                None,
            ));
        }

        let cell_id = CellId::new();
        let cell = match self.create_cell_with_id(request.spec, cell_id) {
            Ok(cell) => cell,
            Err(error) => {
                let stage = match &error {
                    EngineError::InvalidImage(_)
                    | EngineError::ImageIntegrity(_)
                    | EngineError::ImageConflict(_)
                    | EngineError::State(StateError::NotFound(_)) => RunStage::ImageValidation,
                    _ => RunStage::CellCreation,
                };
                let (cleanup, cleanup_error) =
                    self.cleanup_failed_run(cell_id, request.cleanup, false, observer);
                return Err(run_cell_error(
                    Some(cell_id),
                    None,
                    stage,
                    cleanup,
                    error,
                    cleanup_error.as_ref(),
                    None,
                ));
            }
        };

        if observer.observe(&RunProgressEvent::ImageVerified {
            image: cell.image.image_id.clone(),
        }) == RunControl::Cancel
            || observer.observe(&RunProgressEvent::CellCreated { cell_id }) == RunControl::Cancel
        {
            return Err(self.interrupted_run(cell_id, request.cleanup, false, observer));
        }

        if let Err(error) = self.start_cell(cell_id) {
            let (cleanup, cleanup_error) =
                self.cleanup_failed_run(cell_id, request.cleanup, false, observer);
            return Err(run_cell_error(
                Some(cell_id),
                None,
                RunStage::ProviderStart,
                cleanup,
                error,
                cleanup_error.as_ref(),
                None,
            ));
        }
        if observer.observe(&RunProgressEvent::ProviderStarted { cell_id }) == RunControl::Cancel {
            return Err(self.interrupted_run(cell_id, request.cleanup, false, observer));
        }

        let mut guest_was_ready = false;
        let mut interrupted_after_readiness = false;
        let mut guest_operation_id = None;
        let execution = self.exec_guest_with_ready_callback(
            transport,
            credentials,
            GuestExecRequest {
                cell_id,
                command: request.command,
                readiness: request.readiness,
            },
            || {
                guest_was_ready = true;
                if observer.observe(&RunProgressEvent::GuestReady { cell_id }) == RunControl::Cancel
                {
                    interrupted_after_readiness = true;
                    Err(EngineError::LifecycleConflict(
                        "run interrupted after guest readiness".to_owned(),
                    ))
                } else {
                    Ok(())
                }
            },
            |operation_id| guest_operation_id = Some(operation_id),
        );

        let execution = match execution {
            Ok(report) => report,
            Err(error) => {
                let (_, terminal) = guest_failure_class(&error);
                let stage = if interrupted_after_readiness {
                    RunStage::Interrupted
                } else if !guest_was_ready {
                    RunStage::GuestReadiness
                } else {
                    RunStage::GuestExecution
                };
                let (cleanup, cleanup_error) =
                    self.cleanup_failed_run(cell_id, request.cleanup, !terminal, observer);
                return Err(run_cell_error(
                    Some(cell_id),
                    guest_operation_id,
                    stage,
                    cleanup,
                    error,
                    cleanup_error.as_ref(),
                    None,
                ));
            }
        };

        let outcome = if execution.result.exit_code == 0 {
            RunOutcome::Success
        } else {
            RunOutcome::GuestNonZero
        };
        let _ = observer.observe(&RunProgressEvent::CommandCompleted {
            cell_id,
            exit_code: execution.result.exit_code,
        });

        let cleanup = if request.cleanup.keep {
            let disposition = RunCleanupDisposition::RetainedByRequest;
            let _ = observer.observe(&RunProgressEvent::CellRetained {
                cell_id,
                disposition,
            });
            disposition
        } else if outcome == RunOutcome::GuestNonZero && request.cleanup.keep_on_failure {
            let disposition = RunCleanupDisposition::RetainedOnFailure;
            let _ = observer.observe(&RunProgressEvent::CellRetained {
                cell_id,
                disposition,
            });
            disposition
        } else {
            let _ = observer.observe(&RunProgressEvent::CleanupStarted { cell_id });
            match self.destroy_cell_for_run(cell_id) {
                Ok(_) => {
                    let _ = observer.observe(&RunProgressEvent::CellDestroyed { cell_id });
                    RunCleanupDisposition::Destroyed
                }
                Err(error) => {
                    let disposition = cleanup_failure_disposition(&error);
                    if disposition == RunCleanupDisposition::RefusedAmbiguous {
                        let _ = observer.observe(&RunProgressEvent::CleanupRefused { cell_id });
                    }
                    return Err(run_cell_error(
                        Some(cell_id),
                        Some(execution.operation_id),
                        RunStage::Cleanup,
                        disposition,
                        error,
                        None,
                        Some(execution.result),
                    ));
                }
            }
        };

        Ok(RunCellReport {
            schema_version: AUTOMATION_SCHEMA_VERSION,
            cell_id,
            operation_id: execution.operation_id,
            outcome,
            result: execution.result,
            cleanup,
        })
    }

    fn interrupted_run<O: RunObserver>(
        &self,
        cell_id: CellId,
        cleanup_policy: RunCleanupPolicy,
        ambiguous: bool,
        observer: &mut O,
    ) -> RunCellError {
        let error =
            EngineError::LifecycleConflict("run interrupted at a safe checkpoint".to_owned());
        let (cleanup, cleanup_error) =
            self.cleanup_failed_run(cell_id, cleanup_policy, ambiguous, observer);
        run_cell_error(
            Some(cell_id),
            None,
            RunStage::Interrupted,
            cleanup,
            error,
            cleanup_error.as_ref(),
            None,
        )
    }

    fn cleanup_failed_run<O: RunObserver>(
        &self,
        cell_id: CellId,
        policy: RunCleanupPolicy,
        ambiguous: bool,
        observer: &mut O,
    ) -> (RunCleanupDisposition, Option<EngineError>) {
        match self.state.load_cell(cell_id) {
            Err(StateError::NotFound(_)) => {
                return (RunCleanupDisposition::NothingCreated, None);
            }
            Err(error) => {
                return (
                    RunCleanupDisposition::Failed,
                    Some(EngineError::State(error)),
                );
            }
            Ok(_) => {}
        }
        let retained = if policy.keep {
            Some(RunCleanupDisposition::RetainedByRequest)
        } else if policy.keep_on_failure {
            Some(RunCleanupDisposition::RetainedOnFailure)
        } else {
            None
        };
        if let Some(disposition) = retained {
            let _ = observer.observe(&RunProgressEvent::CellRetained {
                cell_id,
                disposition,
            });
            return (disposition, None);
        }
        if ambiguous {
            let _ = observer.observe(&RunProgressEvent::CleanupRefused { cell_id });
            return (RunCleanupDisposition::RefusedAmbiguous, None);
        }
        let _ = observer.observe(&RunProgressEvent::CleanupStarted { cell_id });
        match self.destroy_cell_for_run(cell_id) {
            Ok(_) => {
                let _ = observer.observe(&RunProgressEvent::CellDestroyed { cell_id });
                (RunCleanupDisposition::Destroyed, None)
            }
            Err(error) => {
                let disposition = cleanup_failure_disposition(&error);
                if disposition == RunCleanupDisposition::RefusedAmbiguous {
                    let _ = observer.observe(&RunProgressEvent::CleanupRefused { cell_id });
                }
                (disposition, Some(error))
            }
        }
    }

    pub fn exec_guest<G: GuestTransport>(
        &self,
        transport: &G,
        credentials: &GuestCredentials,
        request: GuestExecRequest,
    ) -> Result<GuestExecReport, EngineError> {
        self.exec_guest_with_ready_callback(transport, credentials, request, || Ok(()), |_| {})
    }

    pub fn exec_guest_observed<G, R>(
        &self,
        transport: &G,
        credentials: &GuestCredentials,
        request: GuestExecRequest,
        on_recorded: R,
    ) -> Result<GuestExecReport, EngineError>
    where
        G: GuestTransport,
        R: FnOnce(GuestOperationId),
    {
        self.exec_guest_with_ready_callback(transport, credentials, request, || Ok(()), on_recorded)
    }

    fn exec_guest_with_ready_callback<G, F, R>(
        &self,
        transport: &G,
        credentials: &GuestCredentials,
        request: GuestExecRequest,
        on_ready: F,
        on_recorded: R,
    ) -> Result<GuestExecReport, EngineError>
    where
        G: GuestTransport,
        F: FnOnce() -> Result<(), EngineError>,
        R: FnOnce(GuestOperationId),
    {
        request.command.validate()?;
        validate_readiness_policy(request.readiness)?;
        let cell_id = request.cell_id;
        let command = request.command;
        let result = self.run_guest_operation(
            transport,
            credentials,
            cell_id,
            GuestOperationPlan {
                kind: GuestOperationKind::Exec,
                readiness: request.readiness,
            },
            on_recorded,
            |authority, expected, operation_id| {
                on_ready()?;
                let result = transport.exec(authority, expected, credentials, &command)?;
                validate_guest_command_result(&command, &result)?;
                let completion = GuestCompletion {
                    exit_code: Some(result.exit_code),
                    stdout_bytes: Some(result.stdout_bytes),
                    stderr_bytes: Some(result.stderr_bytes),
                    artifact_id: None,
                };
                Ok((
                    GuestExecReport {
                        schema_version: AUTOMATION_SCHEMA_VERSION,
                        operation_id,
                        cell_id,
                        result,
                    },
                    completion,
                ))
            },
        )?;
        Ok(result)
    }

    pub fn copy_into_guest<G: GuestTransport>(
        &self,
        transport: &G,
        credentials: &GuestCredentials,
        request: GuestCopyInRequest,
    ) -> Result<GuestCopyInReport, EngineError> {
        validate_guest_timeout_and_size(request.timeout, request.max_bytes)?;
        validate_readiness_policy(request.readiness)?;
        let content = read_ordinary_copy_source(&request.source, request.max_bytes)?;
        let cell_id = request.cell_id;
        let destination = request.destination;
        let overwrite = request.overwrite;
        let timeout = request.timeout;
        let size = content.len() as u64;
        self.run_guest_operation(
            transport,
            credentials,
            cell_id,
            GuestOperationPlan {
                kind: GuestOperationKind::CopyIn,
                readiness: request.readiness,
            },
            |_| {},
            |authority, expected, operation_id| {
                transport.copy_in(
                    authority,
                    expected,
                    credentials,
                    GuestCopyInAction {
                        operation_id,
                        destination: &destination,
                        content: &content,
                        overwrite,
                        timeout,
                    },
                )?;
                Ok((
                    GuestCopyInReport {
                        schema_version: AUTOMATION_SCHEMA_VERSION,
                        operation_id,
                        cell_id,
                        guest_path: destination,
                        size,
                        overwrite,
                    },
                    GuestCompletion::default(),
                ))
            },
        )
    }

    pub fn copy_out_of_guest<G: GuestTransport>(
        &self,
        transport: &G,
        credentials: &GuestCredentials,
        request: GuestCopyOutRequest,
    ) -> Result<ArtifactReport, EngineError> {
        self.collect_artifacts_with_kind(
            transport,
            credentials,
            ArtifactCollectRequest {
                cell_id: request.cell_id,
                sources: vec![request.source],
                timeout: request.timeout,
                max_bytes_per_file: request.max_bytes,
                readiness: request.readiness,
            },
            GuestOperationKind::CopyOut,
        )
    }

    pub fn collect_artifacts<G: GuestTransport>(
        &self,
        transport: &G,
        credentials: &GuestCredentials,
        request: ArtifactCollectRequest,
    ) -> Result<ArtifactReport, EngineError> {
        self.collect_artifacts_with_kind(
            transport,
            credentials,
            request,
            GuestOperationKind::ArtifactCollect,
        )
    }

    pub fn inspect_guest_operation(
        &self,
        operation_id: GuestOperationId,
    ) -> Result<GuestOperationRecord, EngineError> {
        Ok(self.state.load_guest_operation(operation_id)?)
    }

    pub fn list_guest_operations(
        &self,
        cell_id: Option<CellId>,
    ) -> Result<Vec<GuestOperationRecord>, EngineError> {
        let mut operations = self.state.list_guest_operations()?;
        if let Some(cell_id) = cell_id {
            operations.retain(|record| record.cell_id == cell_id);
        }
        Ok(operations)
    }

    pub fn inspect_artifact(
        &self,
        cell_id: CellId,
        operation_id: GuestOperationId,
    ) -> Result<ArtifactRecord, EngineError> {
        let operation = self.state.load_guest_operation(operation_id)?;
        if operation.cell_id != cell_id {
            return Err(EngineError::Integrity(
                "artifact operation is bound to a different cell".to_owned(),
            ));
        }
        if operation.artifact_pruned_at.is_some() {
            return Err(StateError::NotFound(
                self.state
                    .root()
                    .join("artifacts")
                    .join(cell_id.to_string())
                    .join(operation_id.to_string()),
            )
            .into());
        }
        Ok(self.state.load_artifact(cell_id, operation_id)?)
    }

    pub fn prune_artifacts(
        &self,
        request: ArtifactPruneRequest,
    ) -> Result<ArtifactPruneReport, EngineError> {
        if request.max_artifacts == 0 || request.max_artifacts > MAX_ARTIFACT_PRUNE_BATCH {
            return Err(EngineError::InvalidCellRequest(format!(
                "artifact prune batch must be between 1 and {MAX_ARTIFACT_PRUNE_BATCH}"
            )));
        }
        if request.older_than.as_secs() > MAX_ARTIFACT_RETENTION_SECONDS {
            return Err(EngineError::InvalidCellRequest(format!(
                "artifact retention must not exceed {MAX_ARTIFACT_RETENTION_SECONDS} seconds"
            )));
        }
        let retention = Duration::from_std(request.older_than).map_err(|_| {
            EngineError::InvalidCellRequest("artifact retention duration is invalid".to_owned())
        })?;
        let evaluated_at = Utc::now();
        let cutoff = evaluated_at - retention;
        let mutation = self.state.acquire_mutation_lock()?;
        let mut operations = self.state.list_guest_operations()?;
        operations.sort_by_key(|operation| (operation.updated_at, operation.id.to_string()));
        let mut entries = Vec::new();

        for mut operation in operations {
            let is_recovery = operation.artifact_pruned_at.is_some();
            let is_expired_artifact = operation.phase == GuestOperationPhase::Completed
                && operation.artifact_id == Some(operation.id)
                && operation.artifact_pruned_at.is_none()
                && operation
                    .completed_at
                    .is_some_and(|completed_at| completed_at <= cutoff);
            if !is_recovery && !is_expired_artifact {
                continue;
            }
            if entries.len() == request.max_artifacts {
                break;
            }

            let artifact_exists = self
                .state
                .artifact_root_exists(operation.cell_id, operation.id)?;
            let bytes = if is_recovery {
                0
            } else if artifact_exists {
                self.state
                    .load_artifact(operation.cell_id, operation.id)?
                    .entries
                    .iter()
                    .try_fold(0_u64, |total, entry| total.checked_add(entry.size))
                    .ok_or_else(|| EngineError::Integrity("artifact size overflow".to_owned()))?
            } else {
                return Err(EngineError::Integrity(
                    "completed artifact operation is missing its artifact root".to_owned(),
                ));
            };

            let disposition = if request.dry_run {
                ArtifactPruneDisposition::Eligible
            } else {
                if !is_recovery {
                    operation.updated_at = evaluated_at;
                    operation.artifact_pruned_at = Some(evaluated_at);
                    self.state.save_guest_operation(&operation)?;
                }
                self.state
                    .remove_artifact_root(&mutation, operation.cell_id, operation.id)?;
                if is_recovery {
                    ArtifactPruneDisposition::RecoveryCompleted
                } else {
                    ArtifactPruneDisposition::Pruned
                }
            };
            entries.push(ArtifactPruneEntry {
                cell_id: operation.cell_id,
                operation_id: operation.id,
                bytes,
                disposition,
            });
        }

        Ok(ArtifactPruneReport {
            schema_version: AUTOMATION_SCHEMA_VERSION,
            evaluated_at,
            cutoff,
            dry_run: request.dry_run,
            entries,
        })
    }

    pub fn reconcile_guest_operation(
        &self,
        operation_id: GuestOperationId,
    ) -> Result<GuestOperationRecoveryReport, EngineError> {
        let _mutation = self.state.acquire_mutation_lock()?;
        let mut operation = self.state.load_guest_operation(operation_id)?;
        let (disposition, changed) = match operation.phase {
            GuestOperationPhase::Completed | GuestOperationPhase::Failed => {
                (GuestOperationRecoveryDisposition::AlreadyTerminal, false)
            }
            GuestOperationPhase::IntentRecorded => {
                let now = Utc::now();
                operation.phase = GuestOperationPhase::Failed;
                operation.failure = Some(GuestFailureClass::Interrupted);
                operation.updated_at = now;
                operation.completed_at = Some(now);
                self.state.save_guest_operation(&operation)?;
                (
                    GuestOperationRecoveryDisposition::InterruptedBeforeTransport,
                    true,
                )
            }
            GuestOperationPhase::ArtifactCommitted => {
                let artifact_id = operation.artifact_id.ok_or_else(|| {
                    EngineError::Integrity(
                        "artifact-committed operation is missing its artifact id".to_owned(),
                    )
                })?;
                if artifact_id != operation.id {
                    return Err(EngineError::Integrity(
                        "artifact id does not match its operation id".to_owned(),
                    ));
                }
                self.state.load_artifact(operation.cell_id, artifact_id)?;
                let now = Utc::now();
                operation.phase = GuestOperationPhase::Completed;
                operation.updated_at = now;
                operation.completed_at = Some(now);
                self.state.save_guest_operation(&operation)?;
                (
                    GuestOperationRecoveryDisposition::ArtifactCompletionRecovered,
                    true,
                )
            }
            GuestOperationPhase::TransportActive => {
                (GuestOperationRecoveryDisposition::RecoveryRequired, false)
            }
        };
        Ok(GuestOperationRecoveryReport {
            schema_version: AUTOMATION_SCHEMA_VERSION,
            operation,
            disposition,
            required_action: match disposition {
                GuestOperationRecoveryDisposition::RecoveryRequired => RequiredAction::ManualReview,
                GuestOperationRecoveryDisposition::AlreadyTerminal
                | GuestOperationRecoveryDisposition::InterruptedBeforeTransport
                | GuestOperationRecoveryDisposition::ArtifactCompletionRecovered => {
                    RequiredAction::None
                }
            },
            changed,
        })
    }

    pub fn gc_expired(&self) -> Result<GcReport, EngineError> {
        self.gc_expired_at(Utc::now())
    }

    pub fn gc_expired_at(
        &self,
        evaluated_at: chrono::DateTime<Utc>,
    ) -> Result<GcReport, EngineError> {
        self.require_provider()?;
        let mutation = self.state.acquire_mutation_lock()?;
        let operations = self.state.list_guest_operations()?;
        let mut entries = Vec::new();
        for record in self.state.list_cells()? {
            if record.provider != self.provider.name() {
                continue;
            }
            let disposition = match record.expires_at {
                None => GcDisposition::NoTtl,
                Some(expires_at) if expires_at > evaluated_at => GcDisposition::NotExpired,
                Some(_)
                    if operations.iter().any(|operation| {
                        operation.cell_id == record.id && !operation.phase.is_terminal()
                    }) =>
                {
                    GcDisposition::InFlightGuestOperation
                }
                Some(_) => match self.destroy_cell_locked(record.id, &mutation) {
                    Ok(report) if report.changed => GcDisposition::Destroyed,
                    Ok(_) => GcDisposition::AlreadyDestroyed,
                    Err(
                        EngineError::OwnershipNotProven(_)
                        | EngineError::ProviderDrift(_)
                        | EngineError::UnexpectedPowerState(_),
                    ) => GcDisposition::OwnershipMismatch,
                    Err(_) => GcDisposition::Failed,
                },
            };
            entries.push(GcEntry {
                cell_id: record.id,
                expires_at: record.expires_at,
                disposition,
            });
        }
        Ok(GcReport {
            schema_version: AUTOMATION_SCHEMA_VERSION,
            evaluated_at,
            entries,
        })
    }

    pub fn destroy_cell(&self, cell_id: CellId) -> Result<OperationReport, EngineError> {
        self.require_provider()?;
        let mutation = self.state.acquire_mutation_lock()?;
        self.destroy_cell_locked(cell_id, &mutation)
    }

    fn destroy_cell_for_run(&self, cell_id: CellId) -> Result<OperationReport, EngineError> {
        self.require_provider()?;
        let mutation = self.state.acquire_mutation_lock()?;
        if self
            .state
            .list_guest_operations()?
            .iter()
            .any(|operation| operation.cell_id == cell_id && !operation.phase.is_terminal())
        {
            return Err(EngineError::Integrity(
                "automatic run cleanup is blocked by a nonterminal guest operation".to_owned(),
            ));
        }
        self.destroy_cell_locked(cell_id, &mutation)
    }

    fn destroy_cell_locked(
        &self,
        cell_id: CellId,
        mutation: &MutationGuard,
    ) -> Result<OperationReport, EngineError> {
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
            let runtime = self.state.pin_cell_runtime_for(
                record.id,
                record.ownership.configuration_path.clone(),
                record.ownership.overlay_path.clone(),
            )?;
            let destroying_provisioning = match record.phase {
                CellPhase::ProviderObjectCreated => {
                    prove_creation_identity(&record, &vm, false)?;
                    let claim_request = ClaimVmRequest {
                        expected: vm,
                        ownership_marker: record.ownership.provider_marker.clone(),
                    };
                    let authority =
                        ProviderMutationAuthority::new(&record, &installation, &runtime, mutation);
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
                let authority =
                    ProviderMutationAuthority::new(&record, &installation, &runtime, mutation);
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
            let authority =
                ProviderMutationAuthority::new(&record, &installation, &runtime, mutation);
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
                let runtime = self.state.pin_cell_runtime_for(
                    record.id,
                    record.ownership.configuration_path.clone(),
                    record.ownership.overlay_path.clone(),
                )?;
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

    fn require_provider(&self) -> Result<(), EngineError> {
        if !matches!(self.provider.name(), "hyperv" | "qemu") {
            return Err(EngineError::UnsupportedProvider(
                self.provider.name().to_owned(),
            ));
        }
        Ok(())
    }

    fn require_provider_available(&self) -> Result<(), EngineError> {
        self.require_provider()?;
        let probe = self.provider.probe().normalized();
        if probe.available {
            Ok(())
        } else {
            Err(EngineError::ProviderUnavailable(probe.detail))
        }
    }

    fn verify_registered_image(
        &self,
        variant: &ImageVariant,
    ) -> Result<ImmutableParentGuard, EngineError> {
        let canonical = canonical_image_path(&variant.path).map_err(as_image_integrity)?;
        if !paths_equal(&canonical, &variant.path) {
            return Err(EngineError::ImageIntegrity(
                "registered image path no longer resolves to the same file".to_owned(),
            ));
        }
        let mut handle = open_immutable_parent(&canonical).map_err(as_image_integrity)?;
        let file_size = handle
            .file
            .metadata()
            .map_err(|error| EngineError::ImageIntegrity(error.to_string()))?
            .len();
        if file_size != variant.file_size {
            return Err(EngineError::ImageIntegrity(
                "registered image size changed".to_owned(),
            ));
        }
        let provider_info = self.provider.inspect_image(canonical.clone())?;
        validate_base_image(self.provider.name(), &canonical, file_size, &provider_info)
            .map_err(as_image_integrity)?;
        let sha256 = sha256_file(&mut handle.file).map_err(as_image_integrity)?;
        if sha256 != variant.sha256 {
            return Err(EngineError::ImageIntegrity(
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
        if record.provider != self.provider.name()
            || record.spec.provider.as_deref() != Some(self.provider.name())
            || record.image.provider != self.provider.name()
        {
            return Err(EngineError::OwnershipNotProven(
                "manifest provider binding does not match the selected provider".to_owned(),
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
                    "recorded provider object id is not a GUID".to_owned(),
                )
            })?;
            if provider_id.to_string() != identity.id {
                return Err(EngineError::OwnershipNotProven(
                    "recorded provider object id is not canonical".to_owned(),
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
            &self
                .state
                .cell_configuration_path_for(record.id, self.provider.name()),
        ) || !paths_equal(
            &record.ownership.overlay_path,
            &self
                .state
                .cell_overlay_path_for(record.id, &record.image.disk_format),
        ) {
            return Err(EngineError::OwnershipNotProven(
                "manifest runtime paths are outside the CellId-scoped root".to_owned(),
            ));
        }
        Ok(())
    }

    fn collect_artifacts_with_kind<G: GuestTransport>(
        &self,
        transport: &G,
        credentials: &GuestCredentials,
        request: ArtifactCollectRequest,
        kind: GuestOperationKind,
    ) -> Result<ArtifactReport, EngineError> {
        validate_guest_timeout_and_size(request.timeout, request.max_bytes_per_file)?;
        validate_readiness_policy(request.readiness)?;
        if request.sources.is_empty() || request.sources.len() > MAX_ARTIFACT_FILES {
            return Err(EngineError::InvalidCellRequest(format!(
                "artifact collection requires between 1 and {MAX_ARTIFACT_FILES} guest files"
            )));
        }
        let maximum_total = request
            .max_bytes_per_file
            .checked_mul(request.sources.len() as u64)
            .ok_or_else(|| {
                EngineError::InvalidCellRequest("artifact collection size overflow".to_owned())
            })?;
        if maximum_total > MAX_ARTIFACT_TOTAL_BYTES {
            return Err(EngineError::InvalidCellRequest(format!(
                "artifact collection maximum exceeds {MAX_ARTIFACT_TOTAL_BYTES} bytes"
            )));
        }
        let cell_id = request.cell_id;
        let sources = request.sources;
        let timeout = request.timeout;
        let max_bytes = request.max_bytes_per_file;
        self.run_guest_operation(
            transport,
            credentials,
            cell_id,
            GuestOperationPlan {
                kind,
                readiness: request.readiness,
            },
            |_| {},
            |authority, expected, operation_id| {
                let artifact_guard = self.state.prepare_artifact_root(cell_id, operation_id)?;
                let mut entries = Vec::with_capacity(sources.len());
                for (index, source) in sources.iter().enumerate() {
                    let bytes = transport.copy_out(
                        authority,
                        expected,
                        credentials,
                        GuestCopyOutAction {
                            operation_id,
                            source,
                            max_bytes,
                            timeout,
                        },
                    )?;
                    if bytes.len() as u64 > max_bytes {
                        return Err(GuestIoError::InvalidResponse.into());
                    }
                    let host_relative_path =
                        self.state
                            .write_artifact_file(&artifact_guard, index, &bytes)?;
                    entries.push(ArtifactEntry {
                        guest_path: source.as_str().to_owned(),
                        host_relative_path,
                        sha256: format!("{:x}", Sha256::digest(&bytes)),
                        size: bytes.len() as u64,
                    });
                }
                let artifact = ArtifactRecord {
                    schema_version: ARTIFACT_SCHEMA_VERSION,
                    id: operation_id,
                    cell_id,
                    created_at: Utc::now(),
                    entries,
                };
                self.state.save_artifact_new(&artifact_guard, &artifact)?;
                Ok((
                    ArtifactReport {
                        schema_version: AUTOMATION_SCHEMA_VERSION,
                        operation_id,
                        cell_id,
                        artifact,
                    },
                    GuestCompletion {
                        artifact_id: Some(operation_id),
                        ..GuestCompletion::default()
                    },
                ))
            },
        )
    }

    fn run_guest_operation<G, T, R, F>(
        &self,
        transport: &G,
        credentials: &GuestCredentials,
        cell_id: CellId,
        plan: GuestOperationPlan,
        on_recorded: R,
        action: F,
    ) -> Result<T, EngineError>
    where
        G: GuestTransport,
        R: FnOnce(GuestOperationId),
        F: FnOnce(
            &GuestActionAuthority<'_>,
            &ProviderVm,
            GuestOperationId,
        ) -> Result<(T, GuestCompletion), EngineError>,
    {
        self.require_provider()?;
        let mutation = self.state.acquire_mutation_lock()?;
        let record = self.state.load_cell(cell_id)?;
        require_lifecycle_state(&record, "run a guest operation on", &[CellState::Running])?;
        if record.phase != CellPhase::Ready {
            return Err(EngineError::LifecycleConflict(
                "guest operations require a ready cell".to_owned(),
            ));
        }
        if self
            .state
            .list_guest_operations()?
            .iter()
            .any(|operation| operation.cell_id == cell_id && !operation.phase.is_terminal())
        {
            return Err(EngineError::LifecycleConflict(
                "guest operations require all earlier operations to be terminal; reconcile the cell operation history first"
                    .to_owned(),
            ));
        }
        let guest_os = record.image.guest_os.ok_or_else(|| {
            EngineError::Integrity(
                "guest operations require a cell with a persisted guest OS".to_owned(),
            )
        })?;
        if !transport.supports(&record.provider, guest_os) {
            return Err(EngineError::InvalidCellRequest(
                "selected guest transport is incompatible with the provider/guest OS".to_owned(),
            ));
        }
        let expected = self.exact_owned_vm(&record)?;
        if expected.power_state != ProviderPowerState::Running {
            return Err(EngineError::UnexpectedPowerState(expected.power_state));
        }

        let mut operation = GuestOperationRecord::intent(cell_id, plan.kind, Utc::now());
        self.state.save_guest_operation(&operation)?;
        on_recorded(operation.id);
        let execution = (|| {
            let installation = self.state.acquire_installation_authority()?;
            self.validate_local_ownership_against(&record, &installation)?;
            let runtime = self.state.pin_cell_runtime_for(
                cell_id,
                record.ownership.configuration_path.clone(),
                record.ownership.overlay_path.clone(),
            )?;
            let authority =
                GuestActionAuthority::new(&record, &expected, &installation, &runtime, &mutation)?;
            operation.phase = GuestOperationPhase::TransportActive;
            operation.updated_at = Utc::now();
            self.state.save_guest_operation(&operation)?;
            wait_for_guest_ready(
                transport,
                &authority,
                &expected,
                credentials,
                plan.readiness,
            )?;
            action(&authority, &expected, operation.id)
        })();

        match execution {
            Ok((value, completion)) => {
                operation.phase = if completion.artifact_id.is_some() {
                    GuestOperationPhase::ArtifactCommitted
                } else {
                    GuestOperationPhase::Completed
                };
                operation.updated_at = Utc::now();
                operation.exit_code = completion.exit_code;
                operation.stdout_bytes = completion.stdout_bytes;
                operation.stderr_bytes = completion.stderr_bytes;
                operation.artifact_id = completion.artifact_id;
                if operation.phase == GuestOperationPhase::ArtifactCommitted {
                    self.state.save_guest_operation(&operation)?;
                    operation.phase = GuestOperationPhase::Completed;
                    operation.updated_at = Utc::now();
                }
                operation.completed_at = Some(operation.updated_at);
                self.state.save_guest_operation(&operation)?;
                Ok(value)
            }
            Err(error) => {
                let (failure, terminal) = guest_failure_class(&error);
                operation.failure = Some(failure);
                operation.updated_at = Utc::now();
                if terminal {
                    operation.phase = GuestOperationPhase::Failed;
                    operation.completed_at = Some(operation.updated_at);
                }
                self.state.save_guest_operation(&operation)?;
                Err(error)
            }
        }
    }

    fn fail_record<T>(&self, mut record: CellRecord, error: EngineError) -> Result<T, EngineError> {
        record.state = CellState::Failed;
        record.updated_at = Utc::now();
        record.last_error = Some(durable_error_code(&error).to_owned());
        self.state.save_cell(&record)?;
        Err(error)
    }
}

fn durable_error_code(error: &EngineError) -> &'static str {
    match error {
        EngineError::State(_) => "vmcell.state.failed",
        EngineError::Provider(ProviderError::Timeout(_)) => "vmcell.provider.timeout",
        EngineError::Provider(ProviderError::OutputLimit(_)) => "vmcell.provider.output_limit",
        EngineError::Provider(_) => "vmcell.provider.failed",
        EngineError::UnsupportedProvider(_) => "vmcell.provider.unsupported",
        EngineError::ProviderUnavailable(_) => "vmcell.provider.unavailable",
        EngineError::InvalidImage(_) => "vmcell.image.invalid",
        EngineError::ImageIntegrity(_) => "vmcell.image.integrity",
        EngineError::ImageConflict(_) => "vmcell.image.conflict",
        EngineError::InvalidCellRequest(_) => "vmcell.request.invalid",
        EngineError::LifecycleConflict(_) => "vmcell.lifecycle.conflict",
        EngineError::Integrity(_) => "vmcell.state.integrity",
        EngineError::OwnershipNotProven(_) => "vmcell.ownership.not_proven",
        EngineError::ProviderDrift(_) => "vmcell.ownership.drift",
        EngineError::UnexpectedPowerState(_) => "vmcell.lifecycle.power_state",
        EngineError::Guest(GuestIoError::Timeout) => "vmcell.guest.timeout",
        EngineError::Guest(GuestIoError::OutputLimit) => "vmcell.guest.output_limit",
        EngineError::Guest(_) => "vmcell.guest.failed",
    }
}

fn run_cell_error(
    cell_id: Option<CellId>,
    operation_id: Option<GuestOperationId>,
    stage: RunStage,
    cleanup: RunCleanupDisposition,
    source: EngineError,
    cleanup_error: Option<&EngineError>,
    result: Option<GuestCommandResult>,
) -> RunCellError {
    RunCellError {
        report: Box::new(RunFailureReport {
            schema_version: AUTOMATION_SCHEMA_VERSION,
            cell_id,
            operation_id,
            stage,
            cleanup,
            error_code: durable_error_code(&source).to_owned(),
            cleanup_error_code: cleanup_error.map(durable_error_code).map(str::to_owned),
            result,
        }),
        source: Box::new(source),
    }
}

fn cleanup_failure_disposition(error: &EngineError) -> RunCleanupDisposition {
    if matches!(
        error,
        EngineError::OwnershipNotProven(_)
            | EngineError::ProviderDrift(_)
            | EngineError::Integrity(_)
            | EngineError::ImageIntegrity(_)
            | EngineError::UnexpectedPowerState(_)
            | EngineError::Guest(GuestIoError::OwnershipChanged)
    ) {
        RunCleanupDisposition::RefusedAmbiguous
    } else {
        RunCleanupDisposition::Failed
    }
}

fn as_image_integrity(error: EngineError) -> EngineError {
    match error {
        EngineError::InvalidImage(message) => EngineError::ImageIntegrity(message),
        error => error,
    }
}

#[derive(Default)]
struct GuestCompletion {
    exit_code: Option<i32>,
    stdout_bytes: Option<u64>,
    stderr_bytes: Option<u64>,
    artifact_id: Option<GuestOperationId>,
}

fn wait_for_guest_ready<G: GuestTransport>(
    transport: &G,
    authority: &GuestActionAuthority<'_>,
    expected: &ProviderVm,
    credentials: &GuestCredentials,
    policy: ReadinessPolicy,
) -> Result<(), GuestIoError> {
    validate_readiness_policy(policy).map_err(|_| {
        GuestIoError::InvalidRequest("readiness policy is outside the supported bounds")
    })?;
    let deadline = Instant::now() + policy.timeout;
    let mut last = GuestReadiness::GuestNotReady;
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(match last {
                GuestReadiness::AuthenticationFailed => GuestIoError::AuthenticationFailed,
                GuestReadiness::SessionFailed => GuestIoError::SessionFailed,
                _ => GuestIoError::GuestNotReady,
            });
        }
        let attempt_timeout = remaining.min(StdDuration::from_secs(15));
        match transport.probe_ready(authority, expected, credentials, attempt_timeout)? {
            GuestReadiness::Ready => return Ok(()),
            GuestReadiness::AuthenticationFailed => {
                return Err(GuestIoError::AuthenticationFailed);
            }
            status @ (GuestReadiness::GuestNotReady | GuestReadiness::SessionFailed) => {
                last = status;
            }
        }
        let sleep_for = policy
            .poll_interval
            .min(deadline.saturating_duration_since(Instant::now()));
        if !sleep_for.is_zero() {
            thread::sleep(sleep_for);
        }
    }
}

fn validate_readiness_policy(policy: ReadinessPolicy) -> Result<(), EngineError> {
    if policy.timeout.is_zero()
        || policy.timeout > StdDuration::from_secs(crate::guest::MAX_ACTION_TIMEOUT_SECONDS)
        || policy.poll_interval > policy.timeout
    {
        return Err(EngineError::InvalidCellRequest(
            "readiness timeout/poll interval is outside the supported bounds".to_owned(),
        ));
    }
    Ok(())
}

fn validate_guest_timeout_and_size(
    timeout: StdDuration,
    max_bytes: u64,
) -> Result<(), EngineError> {
    if timeout.is_zero()
        || timeout > StdDuration::from_secs(crate::guest::MAX_ACTION_TIMEOUT_SECONDS)
    {
        return Err(EngineError::InvalidCellRequest(
            "guest copy timeout is outside the supported bounds".to_owned(),
        ));
    }
    if max_bytes == 0 || max_bytes > MAX_COPY_BYTES {
        return Err(EngineError::InvalidCellRequest(format!(
            "guest copy size must be between 1 and {MAX_COPY_BYTES} bytes"
        )));
    }
    Ok(())
}

fn validate_guest_command_result(
    command: &GuestCommand,
    result: &GuestCommandResult,
) -> Result<(), EngineError> {
    let stdout_bytes = result.stdout.len() as u64;
    let stderr_bytes = result.stderr.len() as u64;
    if result.encoding != "utf-8"
        || result.stdout_bytes != stdout_bytes
        || result.stderr_bytes != stderr_bytes
        || stdout_bytes.saturating_add(stderr_bytes) > command.max_output_bytes
        || result.truncated
    {
        return Err(GuestIoError::InvalidResponse.into());
    }
    Ok(())
}

fn read_ordinary_copy_source(path: &Path, max_bytes: u64) -> Result<Vec<u8>, EngineError> {
    if max_bytes == 0 || max_bytes > MAX_COPY_BYTES {
        return Err(EngineError::InvalidCellRequest(format!(
            "copy-in size must be between 1 and {MAX_COPY_BYTES} bytes"
        )));
    }
    for ancestor in path.ancestors() {
        if ancestor.as_os_str().is_empty() || !ancestor.exists() {
            continue;
        }
        let metadata = fs::symlink_metadata(ancestor).map_err(|_| {
            EngineError::InvalidCellRequest("copy-in source metadata is unavailable".to_owned())
        })?;
        if is_reparse_point(&metadata) {
            return Err(GuestIoError::PathViolation.into());
        }
    }
    let canonical = path
        .canonicalize()
        .map_err(|_| EngineError::InvalidCellRequest("copy-in source is absent".to_owned()))?;
    #[cfg(windows)]
    let _ancestor_handles = pin_ordinary_copy_source_ancestors(&canonical)?;
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;

        const FILE_SHARE_READ: u32 = 0x0000_0001;
        const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
        options
            .share_mode(FILE_SHARE_READ)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
    }
    let mut file = options.open(&canonical).map_err(|_| {
        EngineError::InvalidCellRequest("copy-in source could not be pinned".to_owned())
    })?;
    let metadata = file.metadata().map_err(|_| {
        EngineError::InvalidCellRequest("copy-in source metadata is unavailable".to_owned())
    })?;
    if !metadata.is_file() || is_reparse_point(&metadata) || metadata.len() > max_bytes {
        return Err(GuestIoError::PathViolation.into());
    }
    if path
        .canonicalize()
        .map(|rechecked| !paths_equal(&rechecked, &canonical))
        .unwrap_or(true)
    {
        return Err(GuestIoError::PartialCopy.into());
    }
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    file.by_ref()
        .take(max_bytes + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| EngineError::InvalidCellRequest("copy-in source read failed".to_owned()))?;
    if bytes.len() as u64 > max_bytes
        || file.metadata().map(|value| value.len()).ok() != Some(bytes.len() as u64)
    {
        return Err(GuestIoError::PartialCopy.into());
    }
    Ok(bytes)
}

#[cfg(windows)]
fn pin_ordinary_copy_source_ancestors(path: &Path) -> Result<Vec<File>, EngineError> {
    use std::os::windows::fs::OpenOptionsExt;

    const FILE_SHARE_READ: u32 = 0x0000_0001;
    const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
    const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;

    let mut ancestors = path
        .parent()
        .into_iter()
        .flat_map(Path::ancestors)
        .filter(|ancestor| !ancestor.as_os_str().is_empty())
        .collect::<Vec<_>>();
    ancestors.reverse();
    let mut handles = Vec::with_capacity(ancestors.len());
    for ancestor in ancestors {
        let mut options = OpenOptions::new();
        options
            .read(true)
            .share_mode(FILE_SHARE_READ)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT);
        let handle = options
            .open(ancestor)
            .map_err(|_| GuestIoError::PathViolation)?;
        let metadata = handle.metadata().map_err(|_| GuestIoError::PathViolation)?;
        if !metadata.is_dir() || is_reparse_point(&metadata) {
            return Err(GuestIoError::PathViolation.into());
        }
        handles.push(handle);
    }
    Ok(handles)
}

fn guest_failure_class(error: &EngineError) -> (GuestFailureClass, bool) {
    match error {
        EngineError::Guest(GuestIoError::GuestNotReady) => (GuestFailureClass::GuestNotReady, true),
        EngineError::Guest(GuestIoError::AuthenticationFailed) => {
            (GuestFailureClass::Authentication, true)
        }
        EngineError::Guest(GuestIoError::SessionFailed) => (GuestFailureClass::Session, true),
        EngineError::Guest(GuestIoError::OwnershipChanged)
        | EngineError::OwnershipNotProven(_)
        | EngineError::ProviderDrift(_) => (GuestFailureClass::OwnershipChanged, true),
        EngineError::Guest(GuestIoError::PathViolation)
        | EngineError::Guest(GuestIoError::InvalidRequest(_)) => {
            (GuestFailureClass::PathViolation, true)
        }
        EngineError::Guest(GuestIoError::Timeout) => (GuestFailureClass::Timeout, false),
        EngineError::Guest(GuestIoError::OutputLimit) => (GuestFailureClass::OutputLimit, false),
        EngineError::Guest(GuestIoError::PartialCopy) => (GuestFailureClass::PartialCopy, false),
        EngineError::Guest(GuestIoError::InvalidResponse) => {
            (GuestFailureClass::InvalidEncoding, false)
        }
        _ => (GuestFailureClass::Unknown, false),
    }
}

fn validate_cell_spec(spec: &CellSpec, provider: &str) -> Result<(), EngineError> {
    if !matches!(provider, "hyperv" | "qemu") || spec.provider.as_deref() != Some(provider) {
        return Err(EngineError::UnsupportedProvider(
            spec.provider
                .clone()
                .unwrap_or_else(|| "<missing>".to_owned()),
        ));
    }
    if provider == "hyperv" && (spec.accelerator.is_some() || spec.allow_tcg) {
        return Err(EngineError::InvalidCellRequest(
            "QEMU accelerator policy cannot be applied to Hyper-V".to_owned(),
        ));
    }
    if let Some(accelerator) = &spec.accelerator {
        if !matches!(
            accelerator.as_str(),
            "auto" | "whpx" | "kvm" | "hvf" | "tcg"
        ) {
            return Err(EngineError::InvalidCellRequest(
                "accelerator must be auto, whpx, kvm, hvf, or tcg".to_owned(),
            ));
        }
    }
    if spec.accelerator.as_deref() == Some("tcg") && !spec.allow_tcg {
        return Err(EngineError::InvalidCellRequest(
            "TCG requires explicit allow_tcg".to_owned(),
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
    if let Some(ttl_seconds) = spec.ttl_seconds {
        if !(MIN_TTL_SECONDS..=MAX_TTL_SECONDS).contains(&ttl_seconds) {
            return Err(EngineError::InvalidCellRequest(format!(
                "ttl_seconds must be between {MIN_TTL_SECONDS} and {MAX_TTL_SECONDS}"
            )));
        }
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
        Err(EngineError::LifecycleConflict(format!(
            "cannot {operation} cell in {:?} state",
            record.state
        )))
    }
}

fn canonical_image_path(path: &Path) -> Result<PathBuf, EngineError> {
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
        .is_some_and(|value| {
            value.eq_ignore_ascii_case("vhdx") || value.eq_ignore_ascii_case("qcow2")
        })
    {
        return Err(EngineError::InvalidImage(
            "base image must use the provider-native .vhdx or .qcow2 extension".to_owned(),
        ));
    }
    path.canonicalize()
        .map_err(|error| EngineError::InvalidImage(error.to_string()))
}

struct ImmutableParentGuard {
    file: File,
    _ancestor_handles: Vec<File>,
}

fn open_immutable_parent(path: &Path) -> Result<ImmutableParentGuard, EngineError> {
    #[cfg(windows)]
    let ancestor_handles = pin_ordinary_copy_source_ancestors(path).map_err(|_| {
        EngineError::InvalidImage(
            "base image ancestors could not be pinned as ordinary directories".to_owned(),
        )
    })?;
    #[cfg(not(windows))]
    let ancestor_handles = Vec::new();
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;

        const FILE_SHARE_READ: u32 = 0x0000_0001;
        const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
        options
            .share_mode(FILE_SHARE_READ)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
    }
    let file = options
        .open(path)
        .map_err(|error| EngineError::InvalidImage(format!("{}: {error}", path.display())))?;
    let metadata = file
        .metadata()
        .map_err(|error| EngineError::InvalidImage(error.to_string()))?;
    if !metadata.is_file() || is_reparse_point(&metadata) {
        return Err(EngineError::InvalidImage(
            "base image handle is not an ordinary file".to_owned(),
        ));
    }
    if path
        .canonicalize()
        .map(|rechecked| !paths_equal(&rechecked, path))
        .unwrap_or(true)
    {
        return Err(EngineError::InvalidImage(
            "base image identity changed while acquiring its immutable handle".to_owned(),
        ));
    }
    Ok(ImmutableParentGuard {
        file,
        _ancestor_handles: ancestor_handles,
    })
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

fn validate_base_image(
    provider: &str,
    expected_path: &Path,
    file_size: u64,
    info: &ProviderImageInfo,
) -> Result<(), EngineError> {
    let expected_format = provider_image_format(provider)?;
    if !paths_equal(expected_path, &info.path) {
        return Err(EngineError::InvalidImage(
            "provider reported a different base image path".to_owned(),
        ));
    }
    if !info.disk_format.eq_ignore_ascii_case(expected_format) {
        return Err(EngineError::InvalidImage(format!(
            "provider image format is not {expected_format}"
        )));
    }
    if info.parent_path.is_some() || info.disk_type.eq_ignore_ascii_case("differencing") {
        return Err(EngineError::InvalidImage(
            "a registered base image cannot itself be differencing".to_owned(),
        ));
    }
    if info.file_size != file_size {
        return Err(EngineError::InvalidImage(
            "filesystem and provider image sizes disagree".to_owned(),
        ));
    }
    Ok(())
}

fn provider_image_format(provider: &str) -> Result<&'static str, EngineError> {
    match provider {
        "hyperv" => Ok("vhdx"),
        "qemu" => Ok("qcow2"),
        _ => Err(EngineError::UnsupportedProvider(provider.to_owned())),
    }
}

fn validate_overlay(record: &CellRecord, info: &ProviderImageInfo) -> Result<(), EngineError> {
    if !paths_equal(&record.ownership.overlay_path, &info.path)
        || !info
            .disk_format
            .eq_ignore_ascii_case(&record.image.disk_format)
        || !(info.disk_type.eq_ignore_ascii_case("differencing")
            || info.disk_type.eq_ignore_ascii_case("overlay"))
        || info
            .parent_path
            .as_ref()
            .is_none_or(|path| !paths_equal(path, &record.image.path))
    {
        return Err(EngineError::ProviderDrift(
            "created overlay did not match the requested immutable parent/path".to_owned(),
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
                "provider create response did not contain a GUID provider id".to_owned(),
            )
        })?
        .to_string();
    if identity.name != record.ownership.provider_object_name {
        return Err(EngineError::ProviderDrift(
            "provider create response name did not match the requested CellId name".to_owned(),
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

fn provider_variant<'a>(
    image: &'a ImageRecord,
    provider: &str,
) -> Result<&'a ImageVariant, EngineError> {
    let mut variants = image
        .variants
        .iter()
        .filter(|variant| variant.provider == provider);
    let variant = variants
        .next()
        .ok_or_else(|| EngineError::ImageIntegrity(format!("image has no {provider} variant")))?;
    if variants.next().is_some() {
        return Err(EngineError::ImageIntegrity(format!(
            "image has more than one {provider} variant"
        )));
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
        schema_version: AUTOMATION_SCHEMA_VERSION,
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
    use std::collections::VecDeque;
    use std::sync::{Arc, Mutex};

    use tempfile::tempdir;

    use super::*;
    use crate::core::capability::ProviderCapabilities;
    use crate::providers::{ProviderProbe, ProviderProbeStatus, ProviderVmIdentity};

    #[test]
    fn reconciliation_classification_is_stable_and_provider_neutral() {
        let cases = [
            (
                ReconciliationStatus::ExactOwned,
                ReconciliationCode::ExactOwned,
                OwnershipClassification::Proven,
                RequiredAction::None,
            ),
            (
                ReconciliationStatus::ManifestOnly,
                ReconciliationCode::ManifestOnly,
                OwnershipClassification::Unproven,
                RequiredAction::ManualReview,
            ),
            (
                ReconciliationStatus::ProviderMissing,
                ReconciliationCode::ProviderMissing,
                OwnershipClassification::Unproven,
                RequiredAction::ManualReview,
            ),
            (
                ReconciliationStatus::UnprovenProviderObject {
                    id: "id".to_owned(),
                },
                ReconciliationCode::UnprovenProviderObject,
                OwnershipClassification::Unproven,
                RequiredAction::ManualReview,
            ),
            (
                ReconciliationStatus::OwnershipMismatch {
                    reasons: vec!["drift".to_owned()],
                },
                ReconciliationCode::OwnershipMismatch,
                OwnershipClassification::Mismatch,
                RequiredAction::ManualReview,
            ),
            (
                ReconciliationStatus::StateDrift {
                    manifest_state: CellState::Stopped,
                    provider_state: ProviderPowerState::Running,
                },
                ReconciliationCode::StateDrift,
                OwnershipClassification::Proven,
                RequiredAction::RetryLifecycle,
            ),
            (
                ReconciliationStatus::Provisioning {
                    phase: CellPhase::ProviderObjectClaimed,
                },
                ReconciliationCode::Provisioning,
                OwnershipClassification::PhaseProven,
                RequiredAction::RecoveryRequired,
            ),
            (
                ReconciliationStatus::Destroyed,
                ReconciliationCode::Destroyed,
                OwnershipClassification::NotApplicable,
                RequiredAction::None,
            ),
        ];

        for (status, code, ownership, required_action) in cases {
            assert_eq!(
                status.classification(),
                ReconciliationClassification {
                    code,
                    ownership,
                    required_action
                }
            );
        }

        let json = serde_json::to_value(
            ReconciliationStatus::Provisioning {
                phase: CellPhase::ProviderObjectClaimed,
            }
            .classification(),
        )
        .unwrap();
        assert_eq!(
            json,
            serde_json::json!({
                "code": "provisioning",
                "ownership": "phase_proven",
                "required_action": "recovery_required"
            })
        );
    }

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
        fail_start: bool,
        fail_stop: bool,
        fail_remove: bool,
        malformed_create_identity: bool,
        noncanonical_create_identity: bool,
        installation_rotation_path: Option<PathBuf>,
        installation_rotation_blocked: bool,
    }

    #[derive(Clone)]
    struct MockHyperV {
        base_size: u64,
        provider_name: &'static str,
        disk_format: &'static str,
        base_disk_type: &'static str,
        overlay_disk_type: &'static str,
        initial_network_adapters: u32,
        use_path_aliases: bool,
        state: Arc<Mutex<MockState>>,
    }

    impl MockHyperV {
        fn new(base_path: PathBuf) -> Self {
            let base_size = fs::metadata(&base_path).unwrap().len();
            Self {
                base_size,
                provider_name: "hyperv",
                disk_format: "vhdx",
                base_disk_type: "dynamic",
                overlay_disk_type: "differencing",
                initial_network_adapters: 1,
                use_path_aliases: false,
                state: Arc::new(Mutex::new(MockState::default())),
            }
        }

        fn new_qemu(base_path: PathBuf) -> Self {
            let mut provider = Self::new(base_path);
            provider.provider_name = "qemu";
            provider.disk_format = "qcow2";
            provider.base_disk_type = "base";
            provider.overlay_disk_type = "overlay";
            provider.initial_network_adapters = 0;
            provider
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
            self.provider_name
        }

        fn probe(&self) -> ProviderProbe {
            ProviderProbe {
                name: self.provider_name,
                status: ProviderProbeStatus::Ready,
                available: true,
                detail: "mock".to_owned(),
                capabilities: ProviderCapabilities {
                    full_system_vm: true,
                    cow_overlay: true,
                    ..ProviderCapabilities::unavailable()
                },
            }
        }

        fn inspect_image(&self, path: PathBuf) -> Result<ProviderImageInfo, ProviderError> {
            self.state.lock().unwrap().calls.push("inspect_image");
            Ok(ProviderImageInfo {
                path: self.provider_path(&path),
                disk_format: self.disk_format.to_owned(),
                disk_type: self.base_disk_type.to_owned(),
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
                disk_format: self.disk_format.to_owned(),
                disk_type: self.overlay_disk_type.to_owned(),
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
                network_adapter_count: self.initial_network_adapters,
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
            if state.fail_start {
                return Err(ProviderError::Command("injected start failure".to_owned()));
            }
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
            if state.fail_stop {
                return Err(ProviderError::Command("injected stop failure".to_owned()));
            }
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
            if state.fail_remove {
                return Err(ProviderError::Command("injected remove failure".to_owned()));
            }
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

    fn qemu_fixture() -> (tempfile::TempDir, CellEngine<MockHyperV>, ImageId) {
        let directory = tempdir().unwrap();
        let base_path = directory.path().join("base.qcow2");
        fs::write(&base_path, b"immutable qcow2 base").unwrap();
        let provider = MockHyperV::new_qemu(base_path.clone());
        let engine = CellEngine::new(StateStore::new(directory.path().join("state")), provider);
        let image_id = ImageId::parse("linux-qemu").unwrap();
        engine
            .register_image(RegisterImageRequest {
                id: image_id.clone(),
                guest_os: GuestOs::Linux,
                guest_arch: Architecture::X86_64,
                path: base_path,
            })
            .unwrap();
        (directory, engine, image_id)
    }

    fn qemu_spec(image: ImageId) -> CellSpec {
        CellSpec {
            image,
            provider: Some("qemu".to_owned()),
            cpu_count: 2,
            memory_mib: 2048,
            ttl_seconds: None,
            accelerator: Some("auto".to_owned()),
            allow_tcg: true,
        }
    }

    #[test]
    fn image_validation_is_read_only_and_reports_provider_identity() {
        let directory = tempdir().unwrap();
        let base_path = directory.path().join("candidate.vhdx");
        fs::write(&base_path, b"prepared immutable base").unwrap();
        let provider = MockHyperV::new(base_path.clone());
        let engine = CellEngine::new(StateStore::new(directory.path().join("state")), provider);

        let report = engine
            .validate_image(ValidateImageRequest {
                guest_os: GuestOs::Windows,
                guest_arch: Architecture::X86_64,
                path: base_path.canonicalize().unwrap(),
            })
            .unwrap();

        assert_eq!(report.status, ImageValidationStatus::Usable);
        assert!(!report.registered);
        assert_eq!(report.provider, "hyperv");
        assert_eq!(report.expected_format, "vhdx");
        assert_eq!(report.observed_format.as_deref(), Some("vhdx"));
        assert!(report.parent_path.is_none());
        assert_eq!(report.sha256.as_deref().map(str::len), Some(64));
        assert!(engine.list_images().unwrap().is_empty());
    }

    #[test]
    fn image_validation_reports_unsupported_extension_and_backing_parent() {
        let directory = tempdir().unwrap();
        let wrong_extension = directory.path().join("candidate.qcow2");
        fs::write(&wrong_extension, b"not a hyperv base").unwrap();
        let provider = MockHyperV::new(wrong_extension.clone());
        let engine = CellEngine::new(StateStore::new(directory.path().join("state")), provider);
        let extension = engine
            .validate_image(ValidateImageRequest {
                guest_os: GuestOs::Windows,
                guest_arch: Architecture::X86_64,
                path: wrong_extension,
            })
            .unwrap();
        assert_eq!(extension.status, ImageValidationStatus::Unusable);
        assert_eq!(
            extension.issues,
            vec![ImageValidationIssue::UnsupportedExtension]
        );
        assert!(
            !engine
                .provider
                .state
                .lock()
                .unwrap()
                .calls
                .contains(&"inspect_image")
        );

        let base_path = directory.path().join("child.vhdx");
        fs::write(&base_path, b"differencing child").unwrap();
        let mut provider = MockHyperV::new(base_path.clone());
        provider.base_disk_type = "differencing";
        let engine = CellEngine::new(StateStore::new(directory.path().join("state2")), provider);
        let backing = engine
            .validate_image(ValidateImageRequest {
                guest_os: GuestOs::Windows,
                guest_arch: Architecture::X86_64,
                path: base_path,
            })
            .unwrap();
        assert_eq!(backing.status, ImageValidationStatus::Unusable);
        assert!(
            backing
                .issues
                .contains(&ImageValidationIssue::DifferencingBase)
        );
    }

    #[test]
    fn registered_image_validation_reports_content_drift() {
        let (directory, engine, image_id) = fixture();
        fs::write(
            directory.path().join("base.vhdx"),
            b"changed base content and size",
        )
        .unwrap();

        let report = engine.validate_registered_image(&image_id).unwrap();

        assert_eq!(report.status, ImageValidationStatus::Unusable);
        assert_eq!(report.image_id.as_ref(), Some(&image_id));
        assert!(
            report
                .issues
                .contains(&ImageValidationIssue::ProviderSizeMismatch)
        );
        assert!(
            report
                .issues
                .contains(&ImageValidationIssue::RegisteredSizeDrift)
        );
        assert!(
            report
                .issues
                .contains(&ImageValidationIssue::RegisteredHashDrift)
        );
    }

    #[test]
    fn provider_neutral_engine_runs_qemu_lifecycle_with_qcow2_paths() {
        let (_directory, engine, image_id) = qemu_fixture();
        let cell = engine.create_cell(qemu_spec(image_id)).unwrap();
        assert_eq!(cell.provider, "qemu");
        assert_eq!(cell.image.disk_format, "qcow2");
        assert_eq!(cell.ownership.overlay_path.extension().unwrap(), "qcow2");
        assert_eq!(
            cell.ownership.configuration_path.file_name().unwrap(),
            "qemu"
        );
        assert_eq!(cell.state, CellState::Stopped);
        assert_eq!(
            engine.start_cell(cell.id).unwrap().state,
            CellState::Running
        );
        assert_eq!(engine.stop_cell(cell.id).unwrap().state, CellState::Stopped);
        assert!(engine.destroy_cell(cell.id).unwrap().changed);
        assert!(!engine.destroy_cell(cell.id).unwrap().changed);
        let calls = engine.provider.state.lock().unwrap().calls.clone();
        assert!(calls.contains(&"create_overlay"));
        assert!(calls.contains(&"start_vm"));
        assert!(calls.contains(&"remove_vm"));
    }

    #[test]
    fn qemu_overlay_without_exact_parent_is_rejected() {
        let (_directory, engine, image_id) = qemu_fixture();
        let cell = engine.create_cell(qemu_spec(image_id)).unwrap();
        let missing_parent = ProviderImageInfo {
            path: cell.ownership.overlay_path.clone(),
            disk_format: "qcow2".to_owned(),
            disk_type: "overlay".to_owned(),
            parent_path: None,
            file_size: cell.image.file_size,
            virtual_size: cell.image.file_size,
        };
        assert!(matches!(
            validate_overlay(&cell, &missing_parent),
            Err(EngineError::ProviderDrift(_))
        ));
    }

    fn spec(image: ImageId) -> CellSpec {
        CellSpec {
            image,
            provider: Some("hyperv".to_owned()),
            cpu_count: 2,
            memory_mib: 4096,
            ttl_seconds: None,
            accelerator: None,
            allow_tcg: false,
        }
    }

    fn run_request(image: ImageId) -> RunCellRequest {
        RunCellRequest {
            spec: spec(image),
            command: GuestCommand {
                program: "cmd.exe".to_owned(),
                args: vec!["/c".to_owned(), "exit 0".to_owned()],
                timeout: StdDuration::from_secs(30),
                max_output_bytes: 1024,
            },
            readiness: readiness_for_test(),
            cleanup: RunCleanupPolicy {
                keep: false,
                keep_on_failure: false,
            },
        }
    }

    fn run_fixture() -> (
        tempfile::TempDir,
        CellEngine<MockHyperV>,
        ImageId,
        MockGuest,
        GuestCredentials,
    ) {
        let (directory, engine, image_id) = fixture();
        let guest = MockGuest::new(engine.provider.state.clone());
        let credentials =
            GuestCredentials::new("Administrator".to_owned(), "credential-sentinel".to_owned())
                .unwrap();
        (directory, engine, image_id, guest, credentials)
    }

    #[derive(Debug, Clone, Copy)]
    enum InjectedGuestFailure {
        Timeout,
        PartialCopy,
        OutputLimit,
        Transport,
    }

    #[derive(Debug)]
    struct MockGuestState {
        calls: Vec<&'static str>,
        readiness: VecDeque<GuestReadiness>,
        failure: Option<InjectedGuestFailure>,
        exec_result: GuestCommandResult,
        copy_in: Option<(GuestPath, Vec<u8>, OverwritePolicy)>,
        copy_out: VecDeque<Vec<u8>>,
        drift_on_probe: bool,
    }

    impl Default for MockGuestState {
        fn default() -> Self {
            Self {
                calls: Vec::new(),
                readiness: VecDeque::from([GuestReadiness::Ready]),
                failure: None,
                exec_result: GuestCommandResult {
                    exit_code: 0,
                    stdout: String::new(),
                    stderr: String::new(),
                    encoding: "utf-8".to_owned(),
                    stdout_bytes: 0,
                    stderr_bytes: 0,
                    truncated: false,
                },
                copy_in: None,
                copy_out: VecDeque::new(),
                drift_on_probe: false,
            }
        }
    }

    #[derive(Clone)]
    struct MockGuest {
        provider: Arc<Mutex<MockState>>,
        state: Arc<Mutex<MockGuestState>>,
    }

    impl MockGuest {
        fn new(provider: Arc<Mutex<MockState>>) -> Self {
            Self {
                provider,
                state: Arc::new(Mutex::new(MockGuestState::default())),
            }
        }

        fn take_failure(&self) -> Result<(), GuestIoError> {
            match self.state.lock().unwrap().failure.take() {
                None => Ok(()),
                Some(InjectedGuestFailure::Timeout) => Err(GuestIoError::Timeout),
                Some(InjectedGuestFailure::PartialCopy) => Err(GuestIoError::PartialCopy),
                Some(InjectedGuestFailure::OutputLimit) => Err(GuestIoError::OutputLimit),
                Some(InjectedGuestFailure::Transport) => Err(GuestIoError::Transport),
            }
        }

        fn prove_current(&self, expected: &ProviderVm) -> Result<(), GuestIoError> {
            if self.provider.lock().unwrap().vm.as_ref() == Some(expected) {
                Ok(())
            } else {
                Err(GuestIoError::OwnershipChanged)
            }
        }
    }

    impl GuestTransport for MockGuest {
        fn name(&self) -> &'static str {
            "mock-guest"
        }

        fn supports(&self, _provider: &str, _guest_os: GuestOs) -> bool {
            true
        }

        fn probe_ready(
            &self,
            authority: &GuestActionAuthority<'_>,
            expected: &ProviderVm,
            _credentials: &GuestCredentials,
            _timeout: StdDuration,
        ) -> Result<GuestReadiness, GuestIoError> {
            authority.validate(expected)?;
            let drift = {
                let mut state = self.state.lock().unwrap();
                state.calls.push("probe_ready");
                std::mem::take(&mut state.drift_on_probe)
            };
            if drift {
                self.provider
                    .lock()
                    .unwrap()
                    .vm
                    .as_mut()
                    .unwrap()
                    .ownership_marker = "foreign-guest-race".to_owned();
            }
            self.prove_current(expected)?;
            Ok(self
                .state
                .lock()
                .unwrap()
                .readiness
                .pop_front()
                .unwrap_or(GuestReadiness::GuestNotReady))
        }

        fn exec(
            &self,
            authority: &GuestActionAuthority<'_>,
            expected: &ProviderVm,
            _credentials: &GuestCredentials,
            _command: &GuestCommand,
        ) -> Result<GuestCommandResult, GuestIoError> {
            authority.validate(expected)?;
            self.prove_current(expected)?;
            self.state.lock().unwrap().calls.push("exec");
            self.take_failure()?;
            Ok(self.state.lock().unwrap().exec_result.clone())
        }

        fn copy_in(
            &self,
            authority: &GuestActionAuthority<'_>,
            expected: &ProviderVm,
            _credentials: &GuestCredentials,
            action: GuestCopyInAction<'_>,
        ) -> Result<(), GuestIoError> {
            authority.validate(expected)?;
            self.prove_current(expected)?;
            self.state.lock().unwrap().calls.push("copy_in");
            self.take_failure()?;
            self.state.lock().unwrap().copy_in = Some((
                action.destination.clone(),
                action.content.to_vec(),
                action.overwrite,
            ));
            Ok(())
        }

        fn copy_out(
            &self,
            authority: &GuestActionAuthority<'_>,
            expected: &ProviderVm,
            _credentials: &GuestCredentials,
            action: GuestCopyOutAction<'_>,
        ) -> Result<Vec<u8>, GuestIoError> {
            authority.validate(expected)?;
            self.prove_current(expected)?;
            self.state.lock().unwrap().calls.push("copy_out");
            self.take_failure()?;
            let bytes = self
                .state
                .lock()
                .unwrap()
                .copy_out
                .pop_front()
                .unwrap_or_default();
            if bytes.len() as u64 > action.max_bytes {
                return Err(GuestIoError::OutputLimit);
            }
            Ok(bytes)
        }
    }

    fn running_fixture() -> (
        tempfile::TempDir,
        CellEngine<MockHyperV>,
        CellRecord,
        MockGuest,
        GuestCredentials,
    ) {
        let (directory, engine, image_id) = fixture();
        let cell = engine.create_cell(spec(image_id)).unwrap();
        engine.start_cell(cell.id).unwrap();
        let cell = engine.state.load_cell(cell.id).unwrap();
        let guest = MockGuest::new(engine.provider.state.clone());
        let credentials =
            GuestCredentials::new("Administrator".to_owned(), "credential-sentinel".to_owned())
                .unwrap();
        (directory, engine, cell, guest, credentials)
    }

    fn readiness_for_test() -> ReadinessPolicy {
        ReadinessPolicy {
            // Keep the fake transport retries load-insensitive when the full
            // fault suite runs concurrently on a busy self-hosted runner.
            timeout: StdDuration::from_secs(2),
            poll_interval: StdDuration::ZERO,
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
    fn persisted_provider_variant_cardinality_is_image_integrity() {
        let mut image = ImageRecord {
            schema_version: IMAGE_SCHEMA_VERSION,
            id: "variant-integrity".parse().unwrap(),
            guest_os: GuestOs::Linux,
            guest_arch: Architecture::X86_64,
            variants: Vec::new(),
            registered_at: Utc::now(),
        };
        assert!(matches!(
            provider_variant(&image, "qemu"),
            Err(EngineError::ImageIntegrity(_))
        ));

        let variant = ImageVariant {
            provider: "qemu".to_owned(),
            disk_format: "qcow2".to_owned(),
            path: PathBuf::from("base.qcow2"),
            sha256: "00".repeat(32),
            file_size: 4096,
        };
        image.variants = vec![variant.clone(), variant];
        assert!(matches!(
            provider_variant(&image, "qemu"),
            Err(EngineError::ImageIntegrity(_))
        ));
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
    fn run_composes_existing_lifecycle_guest_and_destroy_paths() {
        let (_directory, engine, image_id, guest, credentials) = run_fixture();
        guest.state.lock().unwrap().exec_result.stdout = "ok\n".to_owned();
        guest.state.lock().unwrap().exec_result.stdout_bytes = 3;

        let report = engine
            .run_cell(&guest, &credentials, run_request(image_id))
            .unwrap();

        assert_eq!(report.outcome, RunOutcome::Success);
        assert_eq!(report.result.exit_code, 0);
        assert_eq!(report.result.stdout, "ok\n");
        assert_eq!(report.cleanup, RunCleanupDisposition::Destroyed);
        assert!(!engine.destroy_cell(report.cell_id).unwrap().changed);
        let calls = engine.provider.state.lock().unwrap().calls.clone();
        for required in [
            "create_overlay",
            "create_vm",
            "claim_vm",
            "configure_vm",
            "start_vm",
            "stop_vm",
            "remove_vm",
        ] {
            assert!(
                calls.contains(&required),
                "missing lifecycle call {required}"
            );
        }
        assert_eq!(guest.state.lock().unwrap().calls, ["probe_ready", "exec"]);
    }

    #[test]
    fn run_guest_nonzero_is_reported_and_destroyed_by_default() {
        let (_directory, engine, image_id, guest, credentials) = run_fixture();
        guest.state.lock().unwrap().exec_result.exit_code = 23;
        guest.state.lock().unwrap().exec_result.stderr = "failed\n".to_owned();
        guest.state.lock().unwrap().exec_result.stderr_bytes = 7;

        let report = engine
            .run_cell(&guest, &credentials, run_request(image_id))
            .unwrap();

        assert_eq!(report.outcome, RunOutcome::GuestNonZero);
        assert_eq!(report.result.exit_code, 23);
        assert_eq!(report.cleanup, RunCleanupDisposition::Destroyed);
    }

    #[test]
    fn run_keep_and_keep_on_failure_retain_exact_owned_cells() {
        let (_directory, engine, image_id, guest, credentials) = run_fixture();
        let mut keep = run_request(image_id.clone());
        keep.cleanup.keep = true;
        let kept = engine.run_cell(&guest, &credentials, keep).unwrap();
        assert_eq!(kept.cleanup, RunCleanupDisposition::RetainedByRequest);
        assert_eq!(
            engine.inspect_cell(kept.cell_id).unwrap().cell.state,
            CellState::Running
        );
        engine.destroy_cell(kept.cell_id).unwrap();

        guest.state.lock().unwrap().readiness = VecDeque::from([GuestReadiness::Ready]);
        guest.state.lock().unwrap().exec_result.exit_code = 9;
        let mut keep_failure = run_request(image_id);
        keep_failure.cleanup.keep_on_failure = true;
        let retained = engine.run_cell(&guest, &credentials, keep_failure).unwrap();
        assert_eq!(retained.outcome, RunOutcome::GuestNonZero);
        assert_eq!(retained.cleanup, RunCleanupDisposition::RetainedOnFailure);
        assert_eq!(
            engine.inspect_cell(retained.cell_id).unwrap().cell.state,
            CellState::Running
        );
        engine.destroy_cell(retained.cell_id).unwrap();
    }

    #[test]
    fn run_readiness_failure_is_terminal_and_cleanup_is_safe() {
        let (_directory, engine, image_id, guest, credentials) = run_fixture();
        guest.state.lock().unwrap().readiness = VecDeque::new();
        let mut request = run_request(image_id);
        request.readiness = ReadinessPolicy {
            timeout: StdDuration::from_millis(1),
            poll_interval: StdDuration::ZERO,
        };

        let error = engine.run_cell(&guest, &credentials, request).unwrap_err();

        assert_eq!(error.report().stage, RunStage::GuestReadiness);
        assert_eq!(error.report().cleanup, RunCleanupDisposition::Destroyed);
        assert_eq!(error.report().error_code, "vmcell.guest.failed");
    }

    #[test]
    fn run_exec_timeout_is_unknown_and_never_auto_destroyed() {
        let (_directory, engine, image_id, guest, credentials) = run_fixture();
        guest.state.lock().unwrap().failure = Some(InjectedGuestFailure::Timeout);

        let error = engine
            .run_cell(&guest, &credentials, run_request(image_id))
            .unwrap_err();

        assert_eq!(error.report().stage, RunStage::GuestExecution);
        assert_eq!(
            error.report().cleanup,
            RunCleanupDisposition::RefusedAmbiguous
        );
        let cell_id = error.report().cell_id.unwrap();
        assert_eq!(
            engine.inspect_cell(cell_id).unwrap().cell.state,
            CellState::Running
        );
        let operation_id = error.report().operation_id.unwrap();
        let operation = engine.inspect_guest_operation(operation_id).unwrap();
        assert_eq!(operation.cell_id, cell_id);
        assert!(!operation.phase.is_terminal());
    }

    #[test]
    fn run_create_and_start_failures_use_phase_aware_cleanup() {
        let (_directory, engine, image_id, guest, credentials) = run_fixture();
        engine.provider.state.lock().unwrap().fail_claim = true;
        let create_error = engine
            .run_cell(&guest, &credentials, run_request(image_id.clone()))
            .unwrap_err();
        assert_eq!(create_error.report().stage, RunStage::CellCreation);
        assert_eq!(create_error.report().cleanup, RunCleanupDisposition::Failed);
        assert!(create_error.report().cell_id.is_some());

        engine.provider.state.lock().unwrap().fail_claim = false;
        engine.provider.state.lock().unwrap().vm = None;
        engine.provider.state.lock().unwrap().fail_start = true;
        let start_error = engine
            .run_cell(&guest, &credentials, run_request(image_id))
            .unwrap_err();
        assert_eq!(start_error.report().stage, RunStage::ProviderStart);
        assert_eq!(
            start_error.report().cleanup,
            RunCleanupDisposition::Destroyed
        );
    }

    #[test]
    fn run_cleanup_stop_and_remove_failures_retain_a_proven_record() {
        let (_directory, engine, image_id, guest, credentials) = run_fixture();
        engine.provider.state.lock().unwrap().fail_stop = true;
        let stop_error = engine
            .run_cell(&guest, &credentials, run_request(image_id.clone()))
            .unwrap_err();
        assert_eq!(stop_error.report().stage, RunStage::Cleanup);
        assert_eq!(stop_error.report().cleanup, RunCleanupDisposition::Failed);
        assert!(stop_error.report().result.is_some());

        engine.provider.state.lock().unwrap().fail_stop = false;
        engine.provider.state.lock().unwrap().vm = None;
        engine.provider.state.lock().unwrap().fail_remove = true;
        guest.state.lock().unwrap().readiness = VecDeque::from([GuestReadiness::Ready]);
        let remove_error = engine
            .run_cell(&guest, &credentials, run_request(image_id))
            .unwrap_err();
        assert_eq!(remove_error.report().stage, RunStage::Cleanup);
        assert_eq!(remove_error.report().cleanup, RunCleanupDisposition::Failed);
        assert!(remove_error.report().result.is_some());
    }

    #[test]
    fn run_cleanup_refuses_a_concurrent_nonterminal_guest_operation() {
        let (_directory, engine, image_id, guest, credentials) = run_fixture();
        let mut injected_operation_id = None;
        let mut observer = |event: &RunProgressEvent| {
            if let RunProgressEvent::CommandCompleted { cell_id, .. } = event {
                let mut operation =
                    GuestOperationRecord::intent(*cell_id, GuestOperationKind::Exec, Utc::now());
                operation.phase = GuestOperationPhase::TransportActive;
                operation.updated_at = Utc::now();
                injected_operation_id = Some(operation.id);
                engine.state.save_guest_operation(&operation).unwrap();
            }
            RunControl::Continue
        };

        let error = engine
            .run_cell_observed(&guest, &credentials, run_request(image_id), &mut observer)
            .unwrap_err();

        assert_eq!(error.report().stage, RunStage::Cleanup);
        assert_eq!(
            error.report().cleanup,
            RunCleanupDisposition::RefusedAmbiguous
        );
        assert!(error.report().operation_id.is_some());
        assert!(error.report().result.is_some());
        assert_eq!(engine.provider.state.lock().unwrap().remove_calls, 0);
        let cell_id = error.report().cell_id.unwrap();
        assert_eq!(
            engine.inspect_cell(cell_id).unwrap().cell.state,
            CellState::Running
        );
        assert!(
            !engine
                .inspect_guest_operation(injected_operation_id.unwrap())
                .unwrap()
                .phase
                .is_terminal()
        );
    }

    #[test]
    fn run_ttl_is_persisted_for_retained_cells_but_not_a_cleanup_override() {
        let (_directory, engine, image_id, guest, credentials) = run_fixture();
        let mut request = run_request(image_id.clone());
        request.spec.ttl_seconds = Some(3600);
        request.cleanup.keep = true;
        let retained = engine.run_cell(&guest, &credentials, request).unwrap();
        assert!(
            engine
                .state
                .load_cell(retained.cell_id)
                .unwrap()
                .expires_at
                .is_some()
        );
        engine.destroy_cell(retained.cell_id).unwrap();

        guest.state.lock().unwrap().readiness = VecDeque::from([GuestReadiness::Ready]);
        let mut disposable = run_request(image_id);
        disposable.spec.ttl_seconds = Some(3600);
        let destroyed = engine.run_cell(&guest, &credentials, disposable).unwrap();
        assert_eq!(destroyed.cleanup, RunCleanupDisposition::Destroyed);
    }

    #[test]
    fn run_reports_never_serialize_credentials() {
        let (_directory, engine, image_id, guest, credentials) = run_fixture();
        let report = engine
            .run_cell(&guest, &credentials, run_request(image_id))
            .unwrap();
        let serialized = serde_json::to_string(&report).unwrap();
        assert!(!serialized.contains("credential-sentinel"));
        assert!(!format!("{report:?}").contains("credential-sentinel"));
    }

    #[test]
    fn run_provider_drift_between_stages_refuses_cleanup() {
        let (_directory, engine, image_id, guest, credentials) = run_fixture();
        let mut observer = |event: &RunProgressEvent| {
            if matches!(event, RunProgressEvent::ProviderStarted { .. }) {
                engine.provider.alter_marker();
            }
            RunControl::Continue
        };

        let error = engine
            .run_cell_observed(&guest, &credentials, run_request(image_id), &mut observer)
            .unwrap_err();

        assert_eq!(error.report().stage, RunStage::GuestReadiness);
        assert_eq!(
            error.report().cleanup,
            RunCleanupDisposition::RefusedAmbiguous
        );
        assert_eq!(engine.provider.state.lock().unwrap().remove_calls, 0);
    }

    #[test]
    fn run_cancellation_is_safe_before_guest_action_and_ambiguous_after_readiness() {
        let (_directory, engine, image_id, guest, credentials) = run_fixture();
        let mut cancel_after_start = |event: &RunProgressEvent| {
            if matches!(event, RunProgressEvent::ProviderStarted { .. }) {
                RunControl::Cancel
            } else {
                RunControl::Continue
            }
        };
        let safe = engine
            .run_cell_observed(
                &guest,
                &credentials,
                run_request(image_id.clone()),
                &mut cancel_after_start,
            )
            .unwrap_err();
        assert_eq!(safe.report().stage, RunStage::Interrupted);
        assert_eq!(safe.report().cleanup, RunCleanupDisposition::Destroyed);

        let mut cancel_after_ready = |event: &RunProgressEvent| {
            if matches!(event, RunProgressEvent::GuestReady { .. }) {
                RunControl::Cancel
            } else {
                RunControl::Continue
            }
        };
        let ambiguous = engine
            .run_cell_observed(
                &guest,
                &credentials,
                run_request(image_id),
                &mut cancel_after_ready,
            )
            .unwrap_err();
        assert_eq!(ambiguous.report().stage, RunStage::Interrupted);
        assert_eq!(
            ambiguous.report().cleanup,
            RunCleanupDisposition::RefusedAmbiguous
        );
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

        assert!(matches!(error, EngineError::ImageIntegrity(_)));
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

    #[test]
    fn guest_exec_retries_readiness_returns_nonzero_and_persists_no_secrets() {
        let (_directory, engine, cell, guest, credentials) = running_fixture();
        {
            let mut state = guest.state.lock().unwrap();
            state.readiness =
                VecDeque::from([GuestReadiness::GuestNotReady, GuestReadiness::Ready]);
            state.exec_result = GuestCommandResult {
                exit_code: 7,
                stdout: "utf8-✓".to_owned(),
                stderr: "expected nonzero".to_owned(),
                encoding: "utf-8".to_owned(),
                stdout_bytes: "utf8-✓".len() as u64,
                stderr_bytes: "expected nonzero".len() as u64,
                truncated: false,
            };
        }
        let report = engine
            .exec_guest(
                &guest,
                &credentials,
                GuestExecRequest {
                    cell_id: cell.id,
                    command: GuestCommand {
                        program: "cmd.exe".to_owned(),
                        args: vec!["/c".to_owned(), "argument-token-sentinel".to_owned()],
                        timeout: StdDuration::from_secs(1),
                        max_output_bytes: 1024,
                    },
                    readiness: readiness_for_test(),
                },
            )
            .unwrap();
        assert_eq!(report.result.exit_code, 7);
        assert_eq!(report.result.stdout, "utf8-✓");
        let operation = engine.inspect_guest_operation(report.operation_id).unwrap();
        assert_eq!(operation.phase, GuestOperationPhase::Completed);
        assert_eq!(operation.exit_code, Some(7));
        let persisted = serde_json::to_string(&operation).unwrap();
        assert!(!persisted.contains("credential-sentinel"));
        assert!(!persisted.contains("argument-token-sentinel"));
        assert_eq!(
            guest
                .state
                .lock()
                .unwrap()
                .calls
                .iter()
                .filter(|call| **call == "probe_ready")
                .count(),
            2
        );
    }

    #[test]
    fn line_oriented_shell_exec_uses_one_fresh_operation_per_line_and_never_cleans_up() {
        let (_directory, engine, cell, guest, credentials) = running_fixture();
        let mut operation_ids = Vec::new();
        for exit_code in [0, 23] {
            {
                let mut state = guest.state.lock().unwrap();
                state.readiness = VecDeque::from([GuestReadiness::Ready]);
                state.exec_result = GuestCommandResult {
                    exit_code,
                    stdout: format!("line-{exit_code}\n"),
                    stderr: String::new(),
                    encoding: "utf-8".to_owned(),
                    stdout_bytes: format!("line-{exit_code}\n").len() as u64,
                    stderr_bytes: 0,
                    truncated: false,
                };
            }
            let recorded = std::cell::Cell::new(None);
            let report = engine
                .exec_guest_observed(
                    &guest,
                    &credentials,
                    GuestExecRequest {
                        cell_id: cell.id,
                        command: GuestCommand {
                            program: "powershell.exe".to_owned(),
                            args: vec!["-Command".to_owned(), format!("line-{exit_code}")],
                            timeout: StdDuration::from_secs(1),
                            max_output_bytes: 1024,
                        },
                        readiness: ReadinessPolicy {
                            timeout: StdDuration::from_millis(10),
                            poll_interval: StdDuration::from_millis(10),
                        },
                    },
                    |id| recorded.set(Some(id)),
                )
                .unwrap();
            assert_eq!(recorded.get(), Some(report.operation_id));
            assert_eq!(report.result.exit_code, exit_code);
            operation_ids.push(report.operation_id);
        }

        assert_ne!(operation_ids[0], operation_ids[1]);
        for operation_id in operation_ids {
            assert_eq!(
                engine.inspect_guest_operation(operation_id).unwrap().phase,
                GuestOperationPhase::Completed
            );
        }
        assert_eq!(
            engine.state.load_cell(cell.id).unwrap().state,
            CellState::Running
        );
        let provider_state = engine.provider.state.lock().unwrap();
        let provider_calls = &provider_state.calls;
        assert!(!provider_calls.contains(&"stop_vm"));
        assert!(!provider_calls.contains(&"remove_vm"));
        drop(provider_state);
        let guest_state = guest.state.lock().unwrap();
        let guest_calls = &guest_state.calls;
        assert_eq!(
            guest_calls
                .iter()
                .filter(|call| **call == "probe_ready")
                .count(),
            2
        );
        assert_eq!(
            guest_calls.iter().filter(|call| **call == "exec").count(),
            2
        );
    }

    #[test]
    fn guest_operation_reconcile_never_replays_and_validates_committed_artifacts() {
        let (_directory, engine, cell, _guest, _credentials) = running_fixture();

        let intent = GuestOperationRecord::intent(cell.id, GuestOperationKind::Exec, Utc::now());
        engine.state.save_guest_operation(&intent).unwrap();
        let report = engine.reconcile_guest_operation(intent.id).unwrap();
        assert_eq!(
            report.disposition,
            GuestOperationRecoveryDisposition::InterruptedBeforeTransport
        );
        assert!(report.changed);
        assert_eq!(report.required_action, RequiredAction::None);
        assert_eq!(report.operation.phase, GuestOperationPhase::Failed);
        assert_eq!(
            report.operation.failure,
            Some(GuestFailureClass::Interrupted)
        );

        let mut active =
            GuestOperationRecord::intent(cell.id, GuestOperationKind::CopyIn, Utc::now());
        active.phase = GuestOperationPhase::TransportActive;
        engine.state.save_guest_operation(&active).unwrap();
        let report = engine.reconcile_guest_operation(active.id).unwrap();
        assert_eq!(
            report.disposition,
            GuestOperationRecoveryDisposition::RecoveryRequired
        );
        assert!(!report.changed);
        assert_eq!(report.required_action, RequiredAction::ManualReview);
        assert_eq!(report.operation.phase, GuestOperationPhase::TransportActive);

        let mut committed =
            GuestOperationRecord::intent(cell.id, GuestOperationKind::ArtifactCollect, Utc::now());
        committed.phase = GuestOperationPhase::ArtifactCommitted;
        committed.artifact_id = Some(committed.id);
        let artifact_guard = engine
            .state
            .prepare_artifact_root(cell.id, committed.id)
            .unwrap();
        let host_relative_path = engine
            .state
            .write_artifact_file(&artifact_guard, 0, b"recovery-artifact")
            .unwrap();
        let artifact_file = engine.state.root().join(&host_relative_path);
        let artifact = ArtifactRecord {
            schema_version: ARTIFACT_SCHEMA_VERSION,
            id: committed.id,
            cell_id: cell.id,
            created_at: Utc::now(),
            entries: vec![ArtifactEntry {
                guest_path: "recovered.bin".to_owned(),
                host_relative_path,
                sha256: format!("{:x}", Sha256::digest(b"recovery-artifact")),
                size: 17,
            }],
        };
        engine
            .state
            .save_artifact_new(&artifact_guard, &artifact)
            .unwrap();
        engine.state.save_guest_operation(&committed).unwrap();
        let report = engine.reconcile_guest_operation(committed.id).unwrap();
        assert_eq!(
            report.disposition,
            GuestOperationRecoveryDisposition::ArtifactCompletionRecovered
        );
        assert!(report.changed);
        assert_eq!(report.required_action, RequiredAction::None);
        assert_eq!(report.operation.phase, GuestOperationPhase::Completed);

        fs::write(artifact_file, b"tampered").unwrap();
        let mut interrupted_completion = report.operation;
        interrupted_completion.phase = GuestOperationPhase::ArtifactCommitted;
        interrupted_completion.completed_at = None;
        engine
            .state
            .save_guest_operation(&interrupted_completion)
            .unwrap();
        assert!(matches!(
            engine.reconcile_guest_operation(interrupted_completion.id),
            Err(EngineError::State(StateError::ArtifactIntegrity { .. }))
        ));
    }

    #[test]
    fn credential_failure_is_terminal_but_timeout_and_large_output_are_nonreplayable() {
        let request = |cell_id| GuestExecRequest {
            cell_id,
            command: GuestCommand {
                program: "cmd.exe".to_owned(),
                args: vec!["/c".to_owned(), "exit 0".to_owned()],
                timeout: StdDuration::from_secs(1),
                max_output_bytes: 16,
            },
            readiness: readiness_for_test(),
        };

        let (_directory, engine, cell, guest, credentials) = running_fixture();
        guest.state.lock().unwrap().readiness =
            VecDeque::from([GuestReadiness::AuthenticationFailed]);
        assert!(matches!(
            engine.exec_guest(&guest, &credentials, request(cell.id)),
            Err(EngineError::Guest(GuestIoError::AuthenticationFailed))
        ));
        let mut operations = engine.list_guest_operations(Some(cell.id)).unwrap();
        assert_eq!(operations.pop().unwrap().phase, GuestOperationPhase::Failed);

        let (_directory, engine, cell, guest, credentials) = running_fixture();
        {
            let mut state = guest.state.lock().unwrap();
            state.readiness = VecDeque::from([GuestReadiness::Ready]);
            state.failure = Some(InjectedGuestFailure::Timeout);
        }
        assert!(matches!(
            engine.exec_guest(&guest, &credentials, request(cell.id)),
            Err(EngineError::Guest(GuestIoError::Timeout))
        ));
        operations = engine.list_guest_operations(Some(cell.id)).unwrap();
        assert!(operations.iter().any(|operation| {
            operation.phase == GuestOperationPhase::TransportActive
                && operation.failure == Some(GuestFailureClass::Timeout)
        }));
        assert!(matches!(
            engine.exec_guest(&guest, &credentials, request(cell.id)),
            Err(EngineError::LifecycleConflict(message))
                if message.contains("earlier operations")
        ));

        let (_directory, engine, cell, guest, credentials) = running_fixture();
        {
            let mut state = guest.state.lock().unwrap();
            state.readiness = VecDeque::from([GuestReadiness::Ready]);
            state.failure = None;
            state.exec_result = GuestCommandResult {
                exit_code: 0,
                stdout: "too-large-for-limit".to_owned(),
                stderr: String::new(),
                encoding: "utf-8".to_owned(),
                stdout_bytes: "too-large-for-limit".len() as u64,
                stderr_bytes: 0,
                truncated: false,
            };
        }
        assert!(matches!(
            engine.exec_guest(&guest, &credentials, request(cell.id)),
            Err(EngineError::Guest(GuestIoError::InvalidResponse))
        ));
        assert!(
            engine
                .list_guest_operations(Some(cell.id))
                .unwrap()
                .iter()
                .any(|operation| {
                    operation.phase == GuestOperationPhase::TransportActive
                        && operation.failure == Some(GuestFailureClass::InvalidEncoding)
                })
        );

        let (_directory, engine, cell, guest, credentials) = running_fixture();
        {
            let mut state = guest.state.lock().unwrap();
            state.readiness = VecDeque::from([GuestReadiness::Ready]);
            state.failure = Some(InjectedGuestFailure::OutputLimit);
        }
        assert!(matches!(
            engine.exec_guest(&guest, &credentials, request(cell.id)),
            Err(EngineError::Guest(GuestIoError::OutputLimit))
        ));
        assert!(
            engine
                .list_guest_operations(Some(cell.id))
                .unwrap()
                .iter()
                .any(|operation| {
                    operation.phase == GuestOperationPhase::TransportActive
                        && operation.failure == Some(GuestFailureClass::OutputLimit)
                })
        );

        let (_directory, engine, cell, guest, credentials) = running_fixture();
        {
            let mut state = guest.state.lock().unwrap();
            state.readiness = VecDeque::from([GuestReadiness::Ready]);
            state.failure = Some(InjectedGuestFailure::Transport);
        }
        assert!(matches!(
            engine.exec_guest(&guest, &credentials, request(cell.id)),
            Err(EngineError::Guest(GuestIoError::Transport))
        ));
        assert!(
            engine
                .list_guest_operations(Some(cell.id))
                .unwrap()
                .iter()
                .any(|operation| {
                    operation.phase == GuestOperationPhase::TransportActive
                        && operation.failure == Some(GuestFailureClass::Unknown)
                })
        );
    }

    #[test]
    fn line_oriented_shell_faults_preserve_durable_phase_and_never_cleanup() {
        let cases = [
            (
                GuestReadiness::AuthenticationFailed,
                None,
                false,
                GuestFailureClass::Authentication,
                true,
            ),
            (
                GuestReadiness::SessionFailed,
                None,
                false,
                GuestFailureClass::Session,
                true,
            ),
            (
                GuestReadiness::Ready,
                Some(InjectedGuestFailure::Timeout),
                false,
                GuestFailureClass::Timeout,
                false,
            ),
            (
                GuestReadiness::Ready,
                Some(InjectedGuestFailure::Transport),
                false,
                GuestFailureClass::Unknown,
                false,
            ),
            (
                GuestReadiness::Ready,
                None,
                true,
                GuestFailureClass::OwnershipChanged,
                true,
            ),
        ];

        for (readiness, failure, drift, expected_failure, terminal) in cases {
            let (_directory, engine, cell, guest, credentials) = running_fixture();
            {
                let mut state = guest.state.lock().unwrap();
                state.readiness = VecDeque::from([readiness]);
                state.failure = failure;
                state.drift_on_probe = drift;
            }
            let recorded = std::cell::Cell::new(None);
            let result = engine.exec_guest_observed(
                &guest,
                &credentials,
                GuestExecRequest {
                    cell_id: cell.id,
                    command: GuestCommand {
                        program: "powershell.exe".to_owned(),
                        args: vec!["-Command".to_owned(), "failure-case".to_owned()],
                        timeout: StdDuration::from_secs(1),
                        max_output_bytes: 1024,
                    },
                    readiness: ReadinessPolicy {
                        timeout: StdDuration::from_millis(10),
                        poll_interval: StdDuration::from_millis(10),
                    },
                },
                |id| recorded.set(Some(id)),
            );
            assert!(result.is_err());
            let operation_id = recorded
                .get()
                .expect("intent should be durable before transport");
            let operation = engine.inspect_guest_operation(operation_id).unwrap();
            assert_eq!(operation.failure, Some(expected_failure));
            assert_eq!(operation.phase.is_terminal(), terminal);
            if !terminal {
                assert!(matches!(
                    engine.exec_guest(
                        &guest,
                        &credentials,
                        GuestExecRequest {
                            cell_id: cell.id,
                            command: GuestCommand {
                                program: "powershell.exe".to_owned(),
                                args: vec!["-Command".to_owned(), "must-not-run".to_owned()],
                                timeout: StdDuration::from_secs(1),
                                max_output_bytes: 1024,
                            },
                            readiness: readiness_for_test(),
                        }
                    ),
                    Err(EngineError::LifecycleConflict(_))
                ));
            }
            assert_eq!(
                engine.state.load_cell(cell.id).unwrap().state,
                CellState::Running
            );
            let provider_state = engine.provider.state.lock().unwrap();
            assert!(!provider_state.calls.contains(&"stop_vm"));
            assert!(!provider_state.calls.contains(&"remove_vm"));
        }
    }

    #[test]
    fn provider_drift_between_proof_and_guest_action_fails_before_exec() {
        let (_directory, engine, cell, guest, credentials) = running_fixture();
        guest.state.lock().unwrap().drift_on_probe = true;
        assert!(matches!(
            engine.exec_guest(
                &guest,
                &credentials,
                GuestExecRequest {
                    cell_id: cell.id,
                    command: GuestCommand {
                        program: "cmd.exe".to_owned(),
                        args: Vec::new(),
                        timeout: StdDuration::from_secs(1),
                        max_output_bytes: 1024,
                    },
                    readiness: readiness_for_test(),
                }
            ),
            Err(EngineError::Guest(GuestIoError::OwnershipChanged))
        ));
        let state = guest.state.lock().unwrap();
        assert!(!state.calls.contains(&"exec"));
        drop(state);
        let operation = engine
            .list_guest_operations(Some(cell.id))
            .unwrap()
            .pop()
            .unwrap();
        assert_eq!(operation.phase, GuestOperationPhase::Failed);
        assert_eq!(operation.failure, Some(GuestFailureClass::OwnershipChanged));
    }

    #[test]
    fn installation_rotation_blocks_guest_authority_before_transport() {
        let (directory, engine, cell, guest, credentials) = running_fixture();
        let replacement = crate::state::InstallationRecord {
            schema_version: crate::state::INSTALL_SCHEMA_VERSION,
            install_id: Uuid::new_v4(),
        };
        fs::write(
            directory.path().join("state").join("installation.json"),
            serde_json::to_vec(&replacement).unwrap(),
        )
        .unwrap();
        assert!(matches!(
            engine.exec_guest(
                &guest,
                &credentials,
                GuestExecRequest {
                    cell_id: cell.id,
                    command: GuestCommand {
                        program: "cmd.exe".to_owned(),
                        args: Vec::new(),
                        timeout: StdDuration::from_secs(1),
                        max_output_bytes: 1024,
                    },
                    readiness: readiness_for_test(),
                }
            ),
            Err(EngineError::OwnershipNotProven(_))
        ));
        assert!(guest.state.lock().unwrap().calls.is_empty());
        assert!(
            engine
                .list_guest_operations(Some(cell.id))
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn copy_in_is_handle_bounded_and_partial_copy_is_nonreplayable() {
        let (directory, engine, cell, guest, credentials) = running_fixture();
        let source = directory.path().join("input.bin");
        fs::write(&source, b"copy-content").unwrap();
        let destination = GuestPath::parse("inputs/data.bin").unwrap();
        let report = engine
            .copy_into_guest(
                &guest,
                &credentials,
                GuestCopyInRequest {
                    cell_id: cell.id,
                    source: source.clone(),
                    destination: destination.clone(),
                    overwrite: OverwritePolicy::Deny,
                    timeout: StdDuration::from_secs(1),
                    max_bytes: 1024,
                    readiness: readiness_for_test(),
                },
            )
            .unwrap();
        assert_eq!(report.size, 12);
        assert_eq!(
            guest.state.lock().unwrap().copy_in,
            Some((
                destination.clone(),
                b"copy-content".to_vec(),
                OverwritePolicy::Deny
            ))
        );
        {
            let mut state = guest.state.lock().unwrap();
            state.readiness = VecDeque::from([GuestReadiness::Ready]);
            state.failure = Some(InjectedGuestFailure::PartialCopy);
        }
        assert!(matches!(
            engine.copy_into_guest(
                &guest,
                &credentials,
                GuestCopyInRequest {
                    cell_id: cell.id,
                    source,
                    destination,
                    overwrite: OverwritePolicy::Replace,
                    timeout: StdDuration::from_secs(1),
                    max_bytes: 1024,
                    readiness: readiness_for_test(),
                }
            ),
            Err(EngineError::Guest(GuestIoError::PartialCopy))
        ));
        assert!(
            engine
                .list_guest_operations(Some(cell.id))
                .unwrap()
                .iter()
                .any(|operation| {
                    operation.phase == GuestOperationPhase::TransportActive
                        && operation.failure == Some(GuestFailureClass::PartialCopy)
                })
        );
    }

    #[cfg(windows)]
    #[test]
    fn copy_in_rejects_reparse_source_before_guest_transport() {
        let (directory, engine, cell, guest, credentials) = running_fixture();
        let external = directory.path().join("external.bin");
        let source = directory.path().join("linked.bin");
        fs::write(&external, b"foreign-content").unwrap();
        if std::os::windows::fs::symlink_file(&external, &source).is_err() {
            return;
        }

        assert!(matches!(
            engine.copy_into_guest(
                &guest,
                &credentials,
                GuestCopyInRequest {
                    cell_id: cell.id,
                    source,
                    destination: GuestPath::parse("inputs/data.bin").unwrap(),
                    overwrite: OverwritePolicy::Deny,
                    timeout: StdDuration::from_secs(1),
                    max_bytes: 1024,
                    readiness: readiness_for_test(),
                },
            ),
            Err(EngineError::Guest(GuestIoError::PathViolation))
        ));
        assert!(guest.state.lock().unwrap().calls.is_empty());
        assert!(
            engine
                .list_guest_operations(Some(cell.id))
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn copy_out_and_artifact_collection_commit_hash_bound_files() {
        let (_directory, engine, cell, guest, credentials) = running_fixture();
        guest.state.lock().unwrap().copy_out =
            VecDeque::from([b"first".to_vec(), b"second".to_vec()]);
        let report = engine
            .collect_artifacts(
                &guest,
                &credentials,
                ArtifactCollectRequest {
                    cell_id: cell.id,
                    sources: vec![
                        GuestPath::parse("results/first.txt").unwrap(),
                        GuestPath::parse("results/second.txt").unwrap(),
                    ],
                    timeout: StdDuration::from_secs(1),
                    max_bytes_per_file: 1024,
                    readiness: readiness_for_test(),
                },
            )
            .unwrap();
        assert_eq!(report.artifact.entries.len(), 2);
        assert_eq!(report.artifact.entries[0].size, 5);
        assert_eq!(report.artifact.entries[1].size, 6);
        for (entry, expected) in report
            .artifact
            .entries
            .iter()
            .zip([b"first".as_slice(), b"second".as_slice()])
        {
            let path = engine.state.root().join(&entry.host_relative_path);
            assert_eq!(fs::read(path).unwrap(), expected);
            assert_eq!(entry.sha256, format!("{:x}", Sha256::digest(expected)));
        }
        assert_eq!(
            engine
                .inspect_artifact(cell.id, report.operation_id)
                .unwrap(),
            report.artifact
        );
        assert_eq!(
            engine
                .inspect_guest_operation(report.operation_id)
                .unwrap()
                .phase,
            GuestOperationPhase::Completed
        );
    }

    #[test]
    fn artifact_prune_is_bounded_dry_runnable_and_crash_retryable() {
        let (_directory, engine, cell, guest, credentials) = running_fixture();
        guest.state.lock().unwrap().copy_out = VecDeque::from([b"retained".to_vec()]);
        let artifact = engine
            .collect_artifacts(
                &guest,
                &credentials,
                ArtifactCollectRequest {
                    cell_id: cell.id,
                    sources: vec![GuestPath::parse("results/retained.txt").unwrap()],
                    timeout: StdDuration::from_secs(1),
                    max_bytes_per_file: 1024,
                    readiness: readiness_for_test(),
                },
            )
            .unwrap();
        let request = |dry_run| ArtifactPruneRequest {
            older_than: StdDuration::ZERO,
            max_artifacts: 1,
            dry_run,
        };

        let dry_run = engine.prune_artifacts(request(true)).unwrap();
        assert_eq!(dry_run.entries.len(), 1);
        assert_eq!(
            dry_run.entries[0].disposition,
            ArtifactPruneDisposition::Eligible
        );
        assert!(
            engine
                .inspect_artifact(cell.id, artifact.operation_id)
                .is_ok()
        );

        let pruned = engine.prune_artifacts(request(false)).unwrap();
        assert_eq!(pruned.entries[0].bytes, 8);
        assert_eq!(
            pruned.entries[0].disposition,
            ArtifactPruneDisposition::Pruned
        );
        assert!(matches!(
            engine.inspect_artifact(cell.id, artifact.operation_id),
            Err(EngineError::State(StateError::NotFound(_)))
        ));
        assert_eq!(
            engine
                .inspect_guest_operation(artifact.operation_id)
                .unwrap()
                .phase,
            GuestOperationPhase::Completed
        );
        assert!(
            engine
                .inspect_guest_operation(artifact.operation_id)
                .unwrap()
                .artifact_pruned_at
                .is_some()
        );

        let recovered = engine.prune_artifacts(request(false)).unwrap();
        assert_eq!(
            recovered.entries[0].disposition,
            ArtifactPruneDisposition::RecoveryCompleted
        );
        assert_eq!(recovered.entries[0].bytes, 0);
        assert!(matches!(
            engine.prune_artifacts(ArtifactPruneRequest {
                older_than: StdDuration::ZERO,
                max_artifacts: 0,
                dry_run: false,
            }),
            Err(EngineError::InvalidCellRequest(_))
        ));
    }

    #[test]
    fn durable_cell_errors_are_bounded_codes_without_provider_detail() {
        assert_eq!(
            durable_error_code(&EngineError::Provider(ProviderError::Command(
                "credential-sentinel provider stderr".to_owned()
            ))),
            "vmcell.provider.failed"
        );
    }

    #[test]
    fn ttl_gc_is_boundary_exact_idempotent_and_blocks_unknown_guest_work() {
        let (_directory, engine, image_id) = fixture();
        let mut ttl_spec = spec(image_id);
        ttl_spec.ttl_seconds = Some(1);
        let cell = engine.create_cell(ttl_spec).unwrap();
        let expiry = cell.expires_at.unwrap();
        let before = engine
            .gc_expired_at(expiry - Duration::milliseconds(1))
            .unwrap();
        assert_eq!(before.entries[0].disposition, GcDisposition::NotExpired);
        let at = engine.gc_expired_at(expiry).unwrap();
        assert_eq!(at.entries[0].disposition, GcDisposition::Destroyed);
        let again = engine.gc_expired_at(expiry).unwrap();
        assert_eq!(
            again.entries[0].disposition,
            GcDisposition::AlreadyDestroyed
        );

        let (_directory, engine, image_id) = fixture();
        let mut ttl_spec = spec(image_id);
        ttl_spec.ttl_seconds = Some(1);
        let cell = engine.create_cell(ttl_spec).unwrap();
        let operation = GuestOperationRecord::intent(cell.id, GuestOperationKind::Exec, Utc::now());
        engine.state.save_guest_operation(&operation).unwrap();
        let report = engine.gc_expired_at(cell.expires_at.unwrap()).unwrap();
        assert_eq!(
            report.entries[0].disposition,
            GcDisposition::InFlightGuestOperation
        );
        assert!(engine.provider.state.lock().unwrap().vm.is_some());
    }

    #[test]
    fn gc_reports_running_drift_and_rejects_manual_contention_without_mutation() {
        let (_directory, engine, image_id) = fixture();
        let mut ttl_spec = spec(image_id);
        ttl_spec.ttl_seconds = Some(1);
        let cell = engine.create_cell(ttl_spec).unwrap();
        engine.start_cell(cell.id).unwrap();
        engine.provider.alter_marker();
        let report = engine.gc_expired_at(cell.expires_at.unwrap()).unwrap();
        assert_eq!(
            report.entries[0].disposition,
            GcDisposition::OwnershipMismatch
        );
        assert!(engine.provider.state.lock().unwrap().vm.is_some());

        let (_directory, engine, image_id) = fixture();
        let mut ttl_spec = spec(image_id);
        ttl_spec.ttl_seconds = Some(1);
        let cell = engine.create_cell(ttl_spec).unwrap();
        let guard = engine.state.acquire_mutation_lock().unwrap();
        assert!(matches!(
            engine.gc_expired_at(cell.expires_at.unwrap()),
            Err(EngineError::State(StateError::MutationBusy))
        ));
        assert!(engine.provider.state.lock().unwrap().vm.is_some());
        drop(guard);
    }
}
