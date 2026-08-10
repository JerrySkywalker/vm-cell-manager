use std::error::Error;
use std::path::PathBuf;

use clap::{Args, Parser, Subcommand, ValueEnum};
use serde::Serialize;

use crate::config::ConfigError;
use crate::core::automation::{
    AUTOMATION_SCHEMA_VERSION, DOCTOR_CONTRACT, RequiredAction, STATUS_CONTRACT,
};
use crate::core::cell::{CellId, CellIdError, CellRecord};
use crate::core::guest::{GuestOperationId, GuestOperationIdError, GuestOperationRecord};
use crate::core::image::{Architecture, GuestOs, ImageId, ImageIdError, ImageRecord};
use crate::engine::{
    CellInspection, EngineError, ImageValidationReport, RunCellError, RunCleanupDisposition,
    RunFailureReport, RunStage,
};
use crate::guest::{
    DEFAULT_MAX_COPY_BYTES, DEFAULT_MAX_OUTPUT_BYTES, GuestIoError, GuestPath, OverwritePolicy,
};
use crate::providers::{
    ProviderError, ProviderProbe, ProviderProbeStatus, builtin_provider_probes,
};
use crate::state::{StateError, StateStore};

pub const CLI_JSON_SCHEMA_VERSION: u32 = AUTOMATION_SCHEMA_VERSION;

#[derive(Debug, thiserror::Error)]
#[error("invalid CLI input: {0}")]
pub struct CliInputError(pub String);

#[derive(Debug, Parser)]
#[command(
    name = "vmcell",
    version,
    about = "Local disposable VM execution cells",
    after_help = "Windows Human MVP examples (real execution remains release-gated):\n  vmcell doctor\n  vmcell status\n  vmcell image validate --path BASE.vhdx --guest-os windows --provider hyperv\n  vmcell image add --id windows-dev --path BASE.vhdx --guest-os windows --provider hyperv\n  vmcell run --image windows-dev --username Administrator --password-stdin -- cmd.exe /d /c ver\n\nSee docs/quickstart-windows.md. Never place a guest password on argv."
)]
pub struct Cli {
    /// Emit versioned machine-readable JSON.
    #[arg(long, global = true)]
    pub json: bool,

    /// Load versioned user defaults from this exact ordinary file.
    #[arg(long, global = true, value_name = "PATH")]
    pub config: Option<PathBuf>,

    /// Override the local state root.
    #[arg(long, global = true, value_name = "PATH")]
    pub state_root: Option<PathBuf>,

    /// Wait up to this bounded interval for the state mutation lock.
    #[arg(
        long,
        global = true,
        value_parser = parse_lock_timeout_ms
    )]
    pub lock_timeout_ms: Option<u64>,

    /// Override the configured human progress preference.
    #[arg(long, global = true, value_enum)]
    pub human_output: Option<CliHumanOutput>,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Probe the host and built-in providers without mutating host state.
    Doctor,

    /// Summarize providers, cells, images, operations, and safe next actions read-only.
    Status,

    /// Inspect built-in local providers.
    Provider {
        #[command(subcommand)]
        command: ProviderCommand,
    },

    /// Register and inspect immutable images.
    Image {
        #[command(subcommand)]
        command: ImageCommand,
    },

    /// Create one stopped, networkless Hyper-V or QEMU cell.
    Create {
        #[arg(long)]
        image: ImageId,

        #[arg(long)]
        cpu_count: Option<u16>,

        #[arg(long)]
        memory_mib: Option<u64>,

        #[arg(long)]
        ttl_seconds: Option<u64>,

        #[arg(long, value_enum)]
        provider: Option<CliProvider>,

        #[arg(long, value_enum)]
        accelerator: Option<CliAccelerator>,

        #[arg(long)]
        allow_tcg: bool,
    },

    /// Create a disposable cell, run one guest command, and apply the cleanup policy.
    Run {
        #[arg(long)]
        image: ImageId,

        #[arg(long = "cpu", visible_alias = "cpu-count")]
        cpu_count: Option<u16>,

        #[arg(long = "memory", visible_alias = "memory-mib")]
        memory_mib: Option<u64>,

        #[arg(long = "ttl", visible_alias = "ttl-seconds")]
        ttl_seconds: Option<u64>,

        #[arg(long, value_enum)]
        provider: Option<CliProvider>,

        #[arg(long, value_enum)]
        accelerator: Option<CliAccelerator>,

        #[arg(long)]
        allow_tcg: bool,

        #[arg(long, conflicts_with = "keep_on_failure")]
        keep: bool,

        #[arg(long)]
        keep_on_failure: bool,

        #[command(flatten)]
        credential: CredentialArgs,

        #[arg(long)]
        readiness_timeout_seconds: Option<u64>,

        #[arg(long = "action-timeout-seconds")]
        action_timeout_seconds: Option<u64>,

        #[arg(long, default_value_t = DEFAULT_MAX_OUTPUT_BYTES)]
        max_output_bytes: u64,

        #[arg(required = true, trailing_var_arg = true, allow_hyphen_values = true)]
        command: Vec<String>,
    },

    /// List locally recorded cells.
    List,

    /// Inspect one cell and reconcile provider identity without mutation.
    Inspect { cell_id: CellId },

    /// Start an exactly owned cell.
    Start { cell_id: CellId },

    /// Stop an exactly owned cell.
    Stop { cell_id: CellId },

    /// Destroy an exactly owned cell and its owned runtime directory.
    Destroy { cell_id: CellId },

    /// Reconcile local manifests with provider-observed state without mutation.
    Reconcile { cell_id: Option<CellId> },

    /// Execute a process through the exact-owned cell's supported guest transport.
    Exec {
        cell_id: CellId,

        #[command(flatten)]
        credential: CredentialArgs,

        #[arg(long)]
        readiness_timeout_seconds: Option<u64>,

        #[arg(long)]
        timeout_seconds: Option<u64>,

        #[arg(long, default_value_t = DEFAULT_MAX_OUTPUT_BYTES)]
        max_output_bytes: u64,

        #[arg(required = true, trailing_var_arg = true, allow_hyphen_values = true)]
        command: Vec<String>,
    },

    /// Run a line-oriented PowerShell Direct console on an exact-owned Windows cell.
    ///
    /// Each line is an independent bounded operation. This is not a PTY and does not preserve
    /// guest stdin, cwd, environment, or a remote process between lines. The password is read
    /// from bounded stdin; command lines are read separately from the attached Windows console.
    Shell {
        cell_id: CellId,

        #[command(flatten)]
        credential: CredentialArgs,

        #[arg(long)]
        readiness_timeout_seconds: Option<u64>,

        #[arg(long = "action-timeout-seconds")]
        action_timeout_seconds: Option<u64>,

        #[arg(long, default_value_t = DEFAULT_MAX_OUTPUT_BYTES)]
        max_output_bytes: u64,
    },

    /// Atomically copy one ordinary host file into the guest workspace.
    CopyIn {
        cell_id: CellId,

        #[arg(long)]
        source: PathBuf,

        #[arg(long)]
        destination: GuestPath,

        #[arg(long, value_enum, default_value_t = CliOverwritePolicy::Deny)]
        overwrite: CliOverwritePolicy,

        #[command(flatten)]
        credential: CredentialArgs,

        #[arg(long)]
        readiness_timeout_seconds: Option<u64>,

        #[arg(long)]
        timeout_seconds: Option<u64>,

        #[arg(long, default_value_t = DEFAULT_MAX_COPY_BYTES)]
        max_bytes: u64,
    },

    /// Copy one guest-workspace file into the deterministic artifact store.
    CopyOut {
        cell_id: CellId,

        #[arg(long)]
        source: GuestPath,

        #[command(flatten)]
        credential: CredentialArgs,

        #[arg(long)]
        readiness_timeout_seconds: Option<u64>,

        #[arg(long)]
        timeout_seconds: Option<u64>,

        #[arg(long, default_value_t = DEFAULT_MAX_COPY_BYTES)]
        max_bytes: u64,
    },

    /// Collect or inspect deterministic guest artifacts.
    Artifact {
        #[command(subcommand)]
        command: ArtifactCommand,
    },

    /// Inspect durable non-secret guest operation records.
    Operation {
        #[command(subcommand)]
        command: GuestOperationCommand,
    },

    /// Explicitly destroy expired exact-owned cells; no daemon is used.
    Gc,
}

#[derive(Debug, Clone, Args)]
pub struct CredentialArgs {
    #[arg(long)]
    pub username: Option<String>,

    #[arg(long)]
    pub password_stdin: bool,

    #[arg(long, hide = true, value_parser = reject_password_argv)]
    _password: Option<String>,
}

fn reject_password_argv(_value: &str) -> Result<String, &'static str> {
    Err("guest passwords are forbidden on argv; use --password-stdin")
}

fn parse_lock_timeout_ms(value: &str) -> Result<u64, String> {
    parse_bounded_u64(value, 0, 30_000, "lock timeout")
}

fn parse_retention_seconds(value: &str) -> Result<u64, String> {
    parse_bounded_u64(value, 0, 31_536_000, "artifact retention")
}

fn parse_prune_batch(value: &str) -> Result<usize, String> {
    let value = parse_bounded_u64(value, 1, 256, "artifact prune batch")?;
    usize::try_from(value).map_err(|_| "artifact prune batch is not representable".to_owned())
}

fn parse_bounded_u64(value: &str, min: u64, max: u64, label: &str) -> Result<u64, String> {
    let parsed = value
        .parse::<u64>()
        .map_err(|_| format!("{label} must be an integer"))?;
    if (min..=max).contains(&parsed) {
        Ok(parsed)
    } else {
        Err(format!("{label} must be between {min} and {max}"))
    }
}

#[derive(Debug, Subcommand)]
pub enum ArtifactCommand {
    /// Collect one or more guest-workspace files into one atomic artifact record.
    Collect {
        cell_id: CellId,

        #[arg(long = "path", required = true)]
        paths: Vec<GuestPath>,

        #[command(flatten)]
        credential: CredentialArgs,

        #[arg(long)]
        readiness_timeout_seconds: Option<u64>,

        #[arg(long)]
        timeout_seconds: Option<u64>,

        #[arg(long, default_value_t = DEFAULT_MAX_COPY_BYTES)]
        max_bytes_per_file: u64,
    },

    /// Inspect one committed artifact manifest.
    Inspect {
        cell_id: CellId,
        operation_id: GuestOperationId,
    },

    /// Explicitly prune committed artifacts older than the retention interval.
    Prune {
        #[arg(long, default_value_t = 604_800, value_parser = parse_retention_seconds)]
        older_than_seconds: u64,

        #[arg(long, default_value_t = 64, value_parser = parse_prune_batch)]
        max_artifacts: usize,

        #[arg(long)]
        dry_run: bool,
    },
}

#[derive(Debug, Subcommand)]
pub enum GuestOperationCommand {
    /// List operation records, optionally filtered to one cell.
    List { cell_id: Option<CellId> },

    /// Inspect one operation record.
    Inspect { operation_id: GuestOperationId },

    /// Reconcile a durable operation without replaying guest side effects.
    Reconcile { operation_id: GuestOperationId },
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum CliOverwritePolicy {
    Deny,
    Replace,
}

impl From<CliOverwritePolicy> for OverwritePolicy {
    fn from(value: CliOverwritePolicy) -> Self {
        match value {
            CliOverwritePolicy::Deny => Self::Deny,
            CliOverwritePolicy::Replace => Self::Replace,
        }
    }
}

#[derive(Debug, Subcommand)]
pub enum ImageCommand {
    /// Register an immutable provider-compatible VHDX or QCOW2 base.
    Add {
        #[arg(long)]
        id: ImageId,

        #[arg(long)]
        path: PathBuf,

        #[arg(long, value_enum)]
        guest_os: CliGuestOs,

        #[arg(long, value_enum, default_value_t = CliArchitecture::X86_64)]
        guest_arch: CliArchitecture,

        #[arg(long, value_enum)]
        provider: Option<CliProvider>,
    },

    /// Validate a prepared base or revalidate one registered image without mutation.
    Validate {
        #[arg(long, conflicts_with_all = ["path", "guest_os"])]
        id: Option<ImageId>,

        #[arg(long, required_unless_present = "id")]
        path: Option<PathBuf>,

        #[arg(long, value_enum, required_unless_present = "id")]
        guest_os: Option<CliGuestOs>,

        #[arg(long, value_enum, default_value_t = CliArchitecture::X86_64)]
        guest_arch: CliArchitecture,

        #[arg(long, value_enum)]
        provider: Option<CliProvider>,
    },

    /// List registered images.
    List,

    /// Inspect one registered image manifest.
    Inspect { id: ImageId },

    /// List durable cell references that permit or block metadata removal.
    Dependencies { id: ImageId },

    /// Remove only registered image metadata when no non-destroyed cell depends on it.
    Unregister { id: ImageId },
}

#[derive(Debug, Subcommand)]
pub enum ProviderCommand {
    /// List built-in provider probes.
    List,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum CliGuestOs {
    Windows,
    Linux,
}

impl From<CliGuestOs> for GuestOs {
    fn from(value: CliGuestOs) -> Self {
        match value {
            CliGuestOs::Windows => Self::Windows,
            CliGuestOs::Linux => Self::Linux,
        }
    }
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum CliArchitecture {
    X86_64,
    Aarch64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum CliProvider {
    Hyperv,
    Qemu,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum CliHumanOutput {
    Normal,
    Quiet,
}

impl CliProvider {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Hyperv => "hyperv",
            Self::Qemu => "qemu",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum CliAccelerator {
    Auto,
    Whpx,
    Kvm,
    Hvf,
    Tcg,
}

impl CliAccelerator {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Whpx => "whpx",
            Self::Kvm => "kvm",
            Self::Hvf => "hvf",
            Self::Tcg => "tcg",
        }
    }
}

impl From<CliArchitecture> for Architecture {
    fn from(value: CliArchitecture) -> Self {
        match value {
            CliArchitecture::X86_64 => Self::X86_64,
            CliArchitecture::Aarch64 => Self::Aarch64,
        }
    }
}

#[derive(Debug, Serialize)]
pub struct DoctorReport {
    pub schema_version: u32,
    pub contract: &'static str,
    pub status: DoctorStatus,
    pub host_os: &'static str,
    pub host_arch: &'static str,
    pub state_root: PathBuf,
    pub providers: Vec<ProviderProbe>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DoctorStatus {
    Ready,
    Unavailable,
}

impl DoctorReport {
    #[must_use]
    pub fn collect(state_root: Option<PathBuf>) -> Self {
        Self::from_probes(
            state_root.unwrap_or_else(StateStore::default_root),
            builtin_provider_probes(),
        )
    }

    #[must_use]
    pub(crate) fn from_probes(state_root: PathBuf, providers: Vec<ProviderProbe>) -> Self {
        let providers = providers
            .into_iter()
            .map(ProviderProbe::normalized)
            .collect::<Vec<_>>();
        let status = if providers.iter().any(|provider| provider.available) {
            DoctorStatus::Ready
        } else {
            DoctorStatus::Unavailable
        };
        Self {
            schema_version: CLI_JSON_SCHEMA_VERSION,
            contract: DOCTOR_CONTRACT,
            status,
            host_os: std::env::consts::OS,
            host_arch: std::env::consts::ARCH,
            state_root,
            providers,
        }
    }
}

#[derive(Debug, Serialize)]
pub struct StatusReport {
    pub schema_version: u32,
    pub contract: &'static str,
    pub evaluated_at: chrono::DateTime<chrono::Utc>,
    pub providers: Vec<ProviderProbe>,
    pub cells: Vec<StatusCellEntry>,
    pub images: Vec<StatusImageEntry>,
    pub operations: Vec<StatusOperationEntry>,
}

impl StatusReport {
    #[must_use]
    pub fn new(
        evaluated_at: chrono::DateTime<chrono::Utc>,
        providers: Vec<ProviderProbe>,
        cells: Vec<StatusCellEntry>,
        images: Vec<StatusImageEntry>,
        operations: Vec<StatusOperationEntry>,
    ) -> Self {
        Self {
            schema_version: CLI_JSON_SCHEMA_VERSION,
            contract: STATUS_CONTRACT,
            evaluated_at,
            providers,
            cells,
            images,
            operations,
        }
    }
}

#[derive(Debug, Serialize)]
pub struct StatusCellEntry {
    pub cell: CellRecord,
    pub retention: StatusRetention,
    pub pending_operations: usize,
    pub uncertain_operations: usize,
    pub observation: StatusCellObservation,
    pub cleanup: StatusCleanupGuidance,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StatusRetention {
    Manual,
    ActiveUntilExpiry,
    Expired,
    None,
}

#[derive(Debug, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum StatusCellObservation {
    Observed {
        inspection: Box<CellInspection>,
    },
    ProviderUnavailable {
        provider_status: ProviderProbeStatus,
    },
    InspectionFailed {
        error_code: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StatusCleanupGuidance {
    ExactOwnedDestroy,
    PhaseRecovery,
    ReconcileThenRetry,
    NotNeeded,
    ManualReview,
}

#[derive(Debug, Serialize)]
pub struct StatusImageEntry {
    pub image: ImageRecord,
    pub observations: Vec<StatusImageVariantObservation>,
}

#[derive(Debug, Serialize)]
pub struct StatusImageVariantObservation {
    pub provider: String,
    pub observation: StatusImageObservation,
}

#[derive(Debug, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum StatusImageObservation {
    Validated {
        report: Box<ImageValidationReport>,
    },
    ProviderUnavailable {
        provider_status: ProviderProbeStatus,
    },
    ValidationFailed {
        error_code: String,
    },
}

#[derive(Debug, Serialize)]
pub struct StatusOperationEntry {
    pub operation: GuestOperationRecord,
    pub pending: bool,
    pub uncertain: bool,
    pub required_action: RequiredAction,
}

#[derive(Debug, Serialize)]
pub struct ErrorEnvelope {
    pub schema_version: u32,
    pub error: ErrorBody,
}

impl ErrorEnvelope {
    #[must_use]
    pub fn new(classification: CliErrorClassification, message: impl Into<String>) -> Self {
        Self {
            schema_version: CLI_JSON_SCHEMA_VERSION,
            error: ErrorBody {
                code: classification.code.to_owned(),
                category: classification.category,
                message: message.into(),
                retryable: classification.retryable,
                exit_code: classification.exit_code.as_u8(),
            },
        }
    }
}

#[derive(Debug, Serialize)]
pub struct RunErrorEnvelope {
    pub schema_version: u32,
    pub error: ErrorBody,
    pub run: RunErrorReport,
}

impl RunErrorEnvelope {
    #[must_use]
    pub fn new(
        classification: CliErrorClassification,
        message: impl Into<String>,
        run: &RunFailureReport,
    ) -> Self {
        Self {
            schema_version: CLI_JSON_SCHEMA_VERSION,
            error: ErrorBody {
                code: classification.code.to_owned(),
                category: classification.category,
                message: message.into(),
                retryable: classification.retryable,
                exit_code: classification.exit_code.as_u8(),
            },
            run: RunErrorReport::from(run),
        }
    }
}

#[derive(Debug, Serialize)]
pub struct RunErrorReport {
    pub schema_version: u32,
    pub cell_id: Option<CellId>,
    pub operation_id: Option<GuestOperationId>,
    pub stage: RunStage,
    pub cleanup: RunCleanupDisposition,
    pub error_code: String,
    pub cleanup_error_code: Option<String>,
    pub result: Option<RunErrorResult>,
}

impl From<&RunFailureReport> for RunErrorReport {
    fn from(report: &RunFailureReport) -> Self {
        Self {
            schema_version: report.schema_version,
            cell_id: report.cell_id,
            operation_id: report.operation_id,
            stage: report.stage,
            cleanup: report.cleanup,
            error_code: report.error_code.clone(),
            cleanup_error_code: report.cleanup_error_code.clone(),
            result: report.result.as_ref().map(RunErrorResult::from),
        }
    }
}

#[derive(Debug, Serialize)]
pub struct RunErrorResult {
    pub exit_code: i32,
    pub stdout_bytes: u64,
    pub stderr_bytes: u64,
    pub truncated: bool,
}

impl From<&crate::guest::GuestCommandResult> for RunErrorResult {
    fn from(result: &crate::guest::GuestCommandResult) -> Self {
        Self {
            exit_code: result.exit_code,
            stdout_bytes: result.stdout_bytes,
            stderr_bytes: result.stderr_bytes,
            truncated: result.truncated,
        }
    }
}

#[derive(Debug, Serialize)]
pub struct ErrorBody {
    pub code: String,
    pub category: CliErrorCategory,
    pub message: String,
    pub retryable: bool,
    pub exit_code: u8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CliErrorCategory {
    InvalidInput,
    NotFound,
    Conflict,
    Unavailable,
    Unsupported,
    Ownership,
    Contention,
    Timeout,
    Integrity,
    RecoveryRequired,
    ResourceLimit,
    Authentication,
    Internal,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum CliExitCode {
    Success = 0,
    InvalidInput = 2,
    NotFound = 3,
    Conflict = 4,
    Unavailable = 5,
    Ownership = 6,
    Contention = 7,
    Timeout = 8,
    Integrity = 9,
    Internal = 10,
    RecoveryRequired = 11,
    ResourceLimit = 12,
    Authentication = 13,
    Unsupported = 14,
}

impl CliExitCode {
    #[must_use]
    pub const fn as_u8(self) -> u8 {
        self as u8
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CliErrorClassification {
    pub code: &'static str,
    pub category: CliErrorCategory,
    pub exit_code: CliExitCode,
    pub retryable: bool,
}

#[must_use]
pub const fn public_error_message(classification: CliErrorClassification) -> &'static str {
    match classification.category {
        CliErrorCategory::InvalidInput => "request input is invalid",
        CliErrorCategory::NotFound => "requested state object was not found",
        CliErrorCategory::Conflict => "requested operation conflicts with current state",
        CliErrorCategory::Unavailable => "required provider capability is unavailable",
        CliErrorCategory::Ownership => "ownership proof failed",
        CliErrorCategory::Contention => "state mutation lock is busy",
        CliErrorCategory::Timeout => "bounded operation timed out",
        CliErrorCategory::Integrity => "integrity proof failed",
        CliErrorCategory::Internal => "internal operation failed",
        CliErrorCategory::RecoveryRequired => "manual recovery is required",
        CliErrorCategory::ResourceLimit => "configured resource bound was exceeded",
        CliErrorCategory::Authentication => "guest authentication failed",
        CliErrorCategory::Unsupported => "requested capability is unsupported",
    }
}

const fn classification(
    code: &'static str,
    category: CliErrorCategory,
    exit_code: CliExitCode,
    retryable: bool,
) -> CliErrorClassification {
    CliErrorClassification {
        code,
        category,
        exit_code,
        retryable,
    }
}

#[must_use]
pub fn classify_cli_error(error: &(dyn Error + 'static)) -> CliErrorClassification {
    if let Some(error) = error.downcast_ref::<RunCellError>() {
        return classify_engine_error(error.engine_error());
    }
    if let Some(error) = error.downcast_ref::<EngineError>() {
        return classify_engine_error(error);
    }
    if let Some(error) = error.downcast_ref::<StateError>() {
        return classify_state_error(error);
    }
    if let Some(error) = error.downcast_ref::<ProviderError>() {
        return classify_provider_error(error);
    }
    if let Some(error) = error.downcast_ref::<GuestIoError>() {
        return classify_guest_error(error);
    }
    if let Some(error) = error.downcast_ref::<ConfigError>() {
        return classify_config_error(error);
    }
    if error.downcast_ref::<CliInputError>().is_some() {
        return classification(
            "vmcell.invalid_input",
            CliErrorCategory::InvalidInput,
            CliExitCode::InvalidInput,
            false,
        );
    }
    if error.downcast_ref::<CellIdError>().is_some()
        || error.downcast_ref::<ImageIdError>().is_some()
        || error.downcast_ref::<GuestOperationIdError>().is_some()
    {
        return classification(
            "vmcell.invalid_input",
            CliErrorCategory::InvalidInput,
            CliExitCode::InvalidInput,
            false,
        );
    }
    classification(
        "vmcell.internal",
        CliErrorCategory::Internal,
        CliExitCode::Internal,
        false,
    )
}

fn classify_config_error(error: &ConfigError) -> CliErrorClassification {
    match error {
        ConfigError::NotFound => classification(
            "vmcell.config.not_found",
            CliErrorCategory::InvalidInput,
            CliExitCode::InvalidInput,
            false,
        ),
        ConfigError::UnsupportedSchema { .. } => classification(
            "vmcell.config.unsupported_schema",
            CliErrorCategory::Integrity,
            CliExitCode::Integrity,
            false,
        ),
        ConfigError::UnsafePath
        | ConfigError::TooLarge
        | ConfigError::Json(_)
        | ConfigError::InvalidValue(_) => classification(
            "vmcell.config.invalid",
            CliErrorCategory::InvalidInput,
            CliExitCode::InvalidInput,
            false,
        ),
        ConfigError::Io(_) => classification(
            "vmcell.config.io",
            CliErrorCategory::Internal,
            CliExitCode::Internal,
            false,
        ),
    }
}

fn classify_engine_error(error: &EngineError) -> CliErrorClassification {
    match error {
        EngineError::State(error) => classify_state_error(error),
        EngineError::Provider(error) => classify_provider_error(error),
        EngineError::Guest(error) => classify_guest_error(error),
        EngineError::UnsupportedProvider(_) => classification(
            "vmcell.provider.unsupported",
            CliErrorCategory::Unsupported,
            CliExitCode::Unsupported,
            false,
        ),
        EngineError::ProviderUnavailable(_) => classification(
            "vmcell.provider.unavailable",
            CliErrorCategory::Unavailable,
            CliExitCode::Unavailable,
            true,
        ),
        EngineError::InvalidCellRequest(_) => classification(
            "vmcell.invalid_input",
            CliErrorCategory::InvalidInput,
            CliExitCode::InvalidInput,
            false,
        ),
        EngineError::LifecycleConflict(_) => classification(
            "vmcell.lifecycle.conflict",
            CliErrorCategory::Conflict,
            CliExitCode::Conflict,
            false,
        ),
        EngineError::Integrity(_) => classification(
            "vmcell.state.integrity",
            CliErrorCategory::Integrity,
            CliExitCode::Integrity,
            false,
        ),
        EngineError::InvalidImage(_) => classification(
            "vmcell.invalid_input",
            CliErrorCategory::InvalidInput,
            CliExitCode::InvalidInput,
            false,
        ),
        EngineError::ImageIntegrity(_) => classification(
            "vmcell.image.integrity",
            CliErrorCategory::Integrity,
            CliExitCode::Integrity,
            false,
        ),
        EngineError::ImageConflict(_) => classification(
            "vmcell.image.conflict",
            CliErrorCategory::Conflict,
            CliExitCode::Conflict,
            false,
        ),
        EngineError::ImageInUse { .. } => classification(
            "vmcell.image.in_use",
            CliErrorCategory::Conflict,
            CliExitCode::Conflict,
            false,
        ),
        EngineError::OwnershipNotProven(_) => classification(
            "vmcell.ownership.not_proven",
            CliErrorCategory::Ownership,
            CliExitCode::Ownership,
            false,
        ),
        EngineError::ProviderDrift(_) => classification(
            "vmcell.ownership.drift",
            CliErrorCategory::Ownership,
            CliExitCode::Ownership,
            false,
        ),
        EngineError::UnexpectedPowerState(_) => classification(
            "vmcell.lifecycle.conflict",
            CliErrorCategory::Conflict,
            CliExitCode::Conflict,
            false,
        ),
    }
}

fn classify_state_error(error: &StateError) -> CliErrorClassification {
    match error {
        StateError::NotFound(_) => classification(
            "vmcell.state.not_found",
            CliErrorCategory::NotFound,
            CliExitCode::NotFound,
            false,
        ),
        StateError::AlreadyExists(_) => classification(
            "vmcell.state.conflict",
            CliErrorCategory::Conflict,
            CliExitCode::Conflict,
            false,
        ),
        StateError::MutationBusy => classification(
            "vmcell.state.contention",
            CliErrorCategory::Contention,
            CliExitCode::Contention,
            true,
        ),
        StateError::UnsafeRuntimePath(_)
        | StateError::IdentityMismatch { .. }
        | StateError::UnsupportedSchema { .. }
        | StateError::ArtifactIntegrity { .. }
        | StateError::GuestOperationIntegrity { .. } => classification(
            "vmcell.state.integrity",
            CliErrorCategory::Integrity,
            CliExitCode::Integrity,
            false,
        ),
        StateError::Json { .. } => classification(
            "vmcell.state.integrity",
            CliErrorCategory::Integrity,
            CliExitCode::Integrity,
            false,
        ),
        StateError::Io { .. } => classification(
            "vmcell.state.io",
            CliErrorCategory::Internal,
            CliExitCode::Internal,
            false,
        ),
    }
}

fn classify_provider_error(error: &ProviderError) -> CliErrorClassification {
    match error {
        ProviderError::Unsupported { .. } => classification(
            "vmcell.provider.unsupported",
            CliErrorCategory::Unsupported,
            CliExitCode::Unsupported,
            false,
        ),
        ProviderError::Command(_) => classification(
            "vmcell.provider.command_failed",
            CliErrorCategory::Unavailable,
            CliExitCode::Unavailable,
            false,
        ),
        ProviderError::Timeout(_) => classification(
            "vmcell.provider.timeout",
            CliErrorCategory::Timeout,
            CliExitCode::Timeout,
            false,
        ),
        ProviderError::OutputLimit(_) => classification(
            "vmcell.provider.output_limit",
            CliErrorCategory::ResourceLimit,
            CliExitCode::ResourceLimit,
            false,
        ),
        ProviderError::InvalidResponse(_) => classification(
            "vmcell.provider.invalid_response",
            CliErrorCategory::Integrity,
            CliExitCode::Integrity,
            false,
        ),
        ProviderError::NotFound(_) => classification(
            "vmcell.provider.not_found",
            CliErrorCategory::NotFound,
            CliExitCode::NotFound,
            false,
        ),
        ProviderError::Collision(_) => classification(
            "vmcell.provider.conflict",
            CliErrorCategory::Conflict,
            CliExitCode::Conflict,
            false,
        ),
        ProviderError::OwnershipChanged(_) | ProviderError::Authority(_) => classification(
            "vmcell.ownership.changed",
            CliErrorCategory::Ownership,
            CliExitCode::Ownership,
            false,
        ),
    }
}

fn classify_guest_error(error: &GuestIoError) -> CliErrorClassification {
    match error {
        GuestIoError::NotImplemented(_) => classification(
            "vmcell.guest.unsupported",
            CliErrorCategory::Unsupported,
            CliExitCode::Unsupported,
            false,
        ),
        GuestIoError::InvalidRequest(_) | GuestIoError::PathViolation => classification(
            "vmcell.guest.invalid_input",
            CliErrorCategory::InvalidInput,
            CliExitCode::InvalidInput,
            false,
        ),
        GuestIoError::OwnershipChanged => classification(
            "vmcell.ownership.changed",
            CliErrorCategory::Ownership,
            CliExitCode::Ownership,
            false,
        ),
        GuestIoError::GuestNotReady => classification(
            "vmcell.guest.not_ready",
            CliErrorCategory::Unavailable,
            CliExitCode::Unavailable,
            true,
        ),
        GuestIoError::AuthenticationFailed => classification(
            "vmcell.guest.authentication",
            CliErrorCategory::Authentication,
            CliExitCode::Authentication,
            false,
        ),
        GuestIoError::SessionFailed | GuestIoError::Transport => classification(
            "vmcell.guest.transport",
            CliErrorCategory::Unavailable,
            CliExitCode::Unavailable,
            false,
        ),
        GuestIoError::Timeout => classification(
            "vmcell.guest.timeout",
            CliErrorCategory::Timeout,
            CliExitCode::Timeout,
            false,
        ),
        GuestIoError::OutputLimit => classification(
            "vmcell.guest.output_limit",
            CliErrorCategory::ResourceLimit,
            CliExitCode::ResourceLimit,
            false,
        ),
        GuestIoError::InvalidResponse => classification(
            "vmcell.guest.invalid_response",
            CliErrorCategory::Integrity,
            CliExitCode::Integrity,
            false,
        ),
        GuestIoError::PartialCopy => classification(
            "vmcell.guest.recovery_required",
            CliErrorCategory::RecoveryRequired,
            CliExitCode::RecoveryRequired,
            false,
        ),
    }
}

#[derive(Debug, Serialize)]
pub struct ListEnvelope<T> {
    pub schema_version: u32,
    pub items: Vec<T>,
}

impl<T> ListEnvelope<T> {
    #[must_use]
    pub fn new(items: Vec<T>) -> Self {
        Self {
            schema_version: CLI_JSON_SCHEMA_VERSION,
            items,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::automation::{AUTOMATION_SCHEMA_VERSION, DOCTOR_CONTRACT};
    use crate::core::capability::ProviderCapabilities;
    use crate::providers::ProviderProbeStatus;
    use clap::CommandFactory;

    #[test]
    fn doctor_contract_is_versioned_and_machine_readable() {
        let report = DoctorReport::from_probes(
            PathBuf::from("state"),
            vec![ProviderProbe {
                name: "qemu",
                status: ProviderProbeStatus::Ready,
                available: true,
                detail: "diagnostic prose".to_owned(),
                capabilities: ProviderCapabilities {
                    schema_version: AUTOMATION_SCHEMA_VERSION,
                    full_system_vm: true,
                    cow_overlay: true,
                    hardware_acceleration: false,
                    accelerators: vec!["tcg".to_owned()],
                    guest_os: vec!["linux".to_owned()],
                    guest_arch: vec!["x86_64".to_owned()],
                    guest_transports: vec!["qga".to_owned()],
                    networkless_guest_exec: true,
                },
            }],
        );
        let value = serde_json::to_value(report).unwrap();
        assert_eq!(value["schema_version"], AUTOMATION_SCHEMA_VERSION);
        assert_eq!(value["contract"], DOCTOR_CONTRACT);
        assert_eq!(value["status"], "ready");
        assert_eq!(value["providers"][0]["status"], "ready");
        assert_eq!(
            value["providers"][0]["capabilities"]["schema_version"],
            AUTOMATION_SCHEMA_VERSION
        );
        assert_eq!(
            value["providers"][0]["capabilities"]["accelerators"][0],
            "tcg"
        );
    }

    #[test]
    fn doctor_is_unavailable_when_no_provider_is_ready() {
        let report = DoctorReport::from_probes(
            PathBuf::from("state"),
            vec![ProviderProbe {
                name: "hyperv",
                status: ProviderProbeStatus::UnsupportedHost,
                available: false,
                detail: "diagnostic prose".to_owned(),
                capabilities: ProviderCapabilities::unavailable(),
            }],
        );
        assert_eq!(report.status, DoctorStatus::Unavailable);
    }

    #[test]
    fn doctor_normalizes_inconsistent_provider_availability() {
        let report = DoctorReport::from_probes(
            PathBuf::from("state"),
            vec![ProviderProbe {
                name: "qemu",
                status: ProviderProbeStatus::Unavailable,
                available: true,
                detail: "scripted inconsistent probe".to_owned(),
                capabilities: ProviderCapabilities {
                    schema_version: AUTOMATION_SCHEMA_VERSION,
                    full_system_vm: true,
                    cow_overlay: true,
                    hardware_acceleration: true,
                    accelerators: vec!["tcg".to_owned()],
                    guest_os: vec!["linux".to_owned()],
                    guest_arch: vec!["x86_64".to_owned()],
                    guest_transports: vec!["qga".to_owned()],
                    networkless_guest_exec: true,
                },
            }],
        );

        assert_eq!(report.status, DoctorStatus::Unavailable);
        assert!(!report.providers[0].available);
        assert!(!report.providers[0].capabilities.cow_overlay);
    }

    #[test]
    fn json_v1_envelopes_remain_compatibly_shaped() {
        let list = serde_json::to_value(ListEnvelope::<String>::new(Vec::new())).unwrap();
        assert_eq!(
            list,
            serde_json::json!({
                "schema_version": 1,
                "items": []
            })
        );

        let classification = classify_cli_error(&StateError::MutationBusy);
        let error = serde_json::to_value(ErrorEnvelope::new(
            classification,
            "another vmcell mutation is active",
        ))
        .unwrap();
        assert_eq!(
            error,
            serde_json::json!({
                "schema_version": 1,
                "error": {
                    "code": "vmcell.state.contention",
                    "category": "contention",
                    "message": "another vmcell mutation is active",
                    "retryable": true,
                    "exit_code": 7
                }
            })
        );
    }

    #[test]
    fn run_error_envelope_omits_guest_stream_contents() {
        let report = RunFailureReport {
            schema_version: AUTOMATION_SCHEMA_VERSION,
            cell_id: Some(CellId::new()),
            operation_id: Some(GuestOperationId::new()),
            stage: RunStage::Cleanup,
            cleanup: RunCleanupDisposition::Failed,
            error_code: "vmcell.provider.failed".to_owned(),
            cleanup_error_code: None,
            result: Some(crate::guest::GuestCommandResult {
                exit_code: 23,
                stdout: "stdout-secret-sentinel".to_owned(),
                stderr: "stderr-secret-sentinel".to_owned(),
                encoding: "utf-8".to_owned(),
                stdout_bytes: 22,
                stderr_bytes: 22,
                truncated: false,
            }),
        };
        let classification = classify_cli_error(&ProviderError::Command("redacted".to_owned()));
        let value = serde_json::to_value(RunErrorEnvelope::new(
            classification,
            public_error_message(classification),
            &report,
        ))
        .unwrap();
        let serialized = serde_json::to_string(&value).unwrap();

        assert!(!serialized.contains("stdout-secret-sentinel"));
        assert!(!serialized.contains("stderr-secret-sentinel"));
        assert!(value["run"]["result"].get("stdout").is_none());
        assert!(value["run"]["result"].get("stderr").is_none());
        assert_eq!(value["run"]["result"]["exit_code"], 23);
        assert_eq!(value["run"]["result"]["stdout_bytes"], 22);
        assert_eq!(value["run"]["result"]["stderr_bytes"], 22);
    }

    #[test]
    fn deterministic_error_taxonomy_covers_state_provider_guest_and_engine() {
        let cases: Vec<(Box<dyn Error>, &str, CliExitCode, bool)> = vec![
            (
                Box::new(StateError::NotFound(PathBuf::from("missing"))),
                "vmcell.state.not_found",
                CliExitCode::NotFound,
                false,
            ),
            (
                Box::new(StateError::MutationBusy),
                "vmcell.state.contention",
                CliExitCode::Contention,
                true,
            ),
            (
                Box::new(ProviderError::OwnershipChanged("drift".to_owned())),
                "vmcell.ownership.changed",
                CliExitCode::Ownership,
                false,
            ),
            (
                Box::new(ProviderError::Timeout("deadline".to_owned())),
                "vmcell.provider.timeout",
                CliExitCode::Timeout,
                false,
            ),
            (
                Box::new(ProviderError::OutputLimit("bounded output".to_owned())),
                "vmcell.provider.output_limit",
                CliExitCode::ResourceLimit,
                false,
            ),
            (
                Box::new(GuestIoError::Timeout),
                "vmcell.guest.timeout",
                CliExitCode::Timeout,
                false,
            ),
            (
                Box::new(EngineError::ProviderUnavailable("offline".to_owned())),
                "vmcell.provider.unavailable",
                CliExitCode::Unavailable,
                true,
            ),
            (
                Box::new(EngineError::ImageIntegrity("hash drift".to_owned())),
                "vmcell.image.integrity",
                CliExitCode::Integrity,
                false,
            ),
            (
                Box::new(EngineError::ImageInUse {
                    image_id: ImageId::parse("windows-dev").unwrap(),
                    blocking_cells: 1,
                }),
                "vmcell.image.in_use",
                CliExitCode::Conflict,
                false,
            ),
            (
                Box::new(EngineError::InvalidImage("missing image path".to_owned())),
                "vmcell.invalid_input",
                CliExitCode::InvalidInput,
                false,
            ),
            (
                Box::new(EngineError::LifecycleConflict("not ready".to_owned())),
                "vmcell.lifecycle.conflict",
                CliExitCode::Conflict,
                false,
            ),
            (
                Box::new(EngineError::Integrity("manifest drift".to_owned())),
                "vmcell.state.integrity",
                CliExitCode::Integrity,
                false,
            ),
            (
                Box::new(CliInputError("missing credential flag".to_owned())),
                "vmcell.invalid_input",
                CliExitCode::InvalidInput,
                false,
            ),
            (
                Box::new(StateError::Json {
                    path: PathBuf::from("cells/cell.json"),
                    source: serde_json::from_str::<serde_json::Value>("{").unwrap_err(),
                }),
                "vmcell.state.integrity",
                CliExitCode::Integrity,
                false,
            ),
            (
                Box::new(ConfigError::InvalidValue("invalid default")),
                "vmcell.config.invalid",
                CliExitCode::InvalidInput,
                false,
            ),
            (
                Box::new(ConfigError::UnsupportedSchema {
                    expected: 1,
                    actual: 2,
                }),
                "vmcell.config.unsupported_schema",
                CliExitCode::Integrity,
                false,
            ),
            (
                Box::new(ConfigError::NotFound),
                "vmcell.config.not_found",
                CliExitCode::InvalidInput,
                false,
            ),
        ];

        for (error, code, exit_code, retryable) in cases {
            let actual = classify_cli_error(error.as_ref());
            assert_eq!(actual.code, code);
            assert_eq!(actual.exit_code, exit_code);
            assert_eq!(actual.retryable, retryable);
        }
    }

    #[test]
    fn provider_fault_taxonomy_is_exhaustive_and_backend_independent() {
        let common = [
            (
                ProviderError::Command("redacted".to_owned()),
                "vmcell.provider.command_failed",
                CliExitCode::Unavailable,
            ),
            (
                ProviderError::Timeout("redacted".to_owned()),
                "vmcell.provider.timeout",
                CliExitCode::Timeout,
            ),
            (
                ProviderError::OutputLimit("redacted".to_owned()),
                "vmcell.provider.output_limit",
                CliExitCode::ResourceLimit,
            ),
            (
                ProviderError::InvalidResponse("redacted".to_owned()),
                "vmcell.provider.invalid_response",
                CliExitCode::Integrity,
            ),
            (
                ProviderError::NotFound("redacted".to_owned()),
                "vmcell.provider.not_found",
                CliExitCode::NotFound,
            ),
            (
                ProviderError::Collision("redacted".to_owned()),
                "vmcell.provider.conflict",
                CliExitCode::Conflict,
            ),
            (
                ProviderError::OwnershipChanged("redacted".to_owned()),
                "vmcell.ownership.changed",
                CliExitCode::Ownership,
            ),
            (
                ProviderError::Authority("redacted".to_owned()),
                "vmcell.ownership.changed",
                CliExitCode::Ownership,
            ),
        ];
        for (error, code, exit_code) in common {
            let classification = classify_provider_error(&error);
            assert_eq!(classification.code, code);
            assert_eq!(classification.exit_code, exit_code);
            assert!(!classification.retryable);
        }

        for provider in ["hyperv", "qemu"] {
            let classification = classify_provider_error(&ProviderError::Unsupported {
                provider,
                operation: "fixture",
            });
            assert_eq!(classification.code, "vmcell.provider.unsupported");
            assert_eq!(classification.exit_code, CliExitCode::Unsupported);
        }
    }

    #[test]
    fn automation_exit_codes_are_stable_and_nonzero_for_errors() {
        let values = [
            CliExitCode::InvalidInput,
            CliExitCode::NotFound,
            CliExitCode::Conflict,
            CliExitCode::Unavailable,
            CliExitCode::Ownership,
            CliExitCode::Contention,
            CliExitCode::Timeout,
            CliExitCode::Integrity,
            CliExitCode::Internal,
            CliExitCode::RecoveryRequired,
            CliExitCode::ResourceLimit,
            CliExitCode::Authentication,
            CliExitCode::Unsupported,
        ];
        let numeric = values.map(CliExitCode::as_u8);
        for (index, value) in numeric.iter().enumerate() {
            assert_ne!(*value, 0);
            assert!(!numeric[..index].contains(value));
        }
        assert_eq!(CliExitCode::Success.as_u8(), 0);
    }

    #[test]
    fn parses_m1_create_surface() {
        let cli = Cli::try_parse_from([
            "vmcell",
            "--json",
            "--state-root",
            "state",
            "create",
            "--image",
            "windows-dev",
            "--cpu-count",
            "4",
            "--memory-mib",
            "8192",
        ])
        .unwrap();

        assert!(cli.json);
        assert_eq!(cli.state_root, Some(PathBuf::from("state")));
        assert!(matches!(
            cli.command,
            Command::Create {
                cpu_count: Some(4),
                memory_mib: Some(8192),
                ..
            }
        ));
    }

    #[test]
    fn config_backed_fields_remain_absent_until_explicitly_overridden() {
        let cli = Cli::try_parse_from([
            "vmcell",
            "--config",
            "user-config.json",
            "--human-output",
            "quiet",
            "create",
            "--image",
            "windows-dev",
        ])
        .unwrap();

        assert_eq!(cli.config, Some(PathBuf::from("user-config.json")));
        assert_eq!(cli.human_output, Some(CliHumanOutput::Quiet));
        assert!(matches!(
            cli.command,
            Command::Create {
                cpu_count: None,
                memory_mib: None,
                provider: None,
                ..
            }
        ));
    }

    #[test]
    fn parses_human_run_surface_and_rejects_conflicting_retention() {
        let cli = Cli::try_parse_from([
            "vmcell",
            "run",
            "--image",
            "windows-dev",
            "--cpu",
            "4",
            "--memory",
            "8192",
            "--ttl",
            "3600",
            "--keep-on-failure",
            "--username",
            "Administrator",
            "--password-stdin",
            "--",
            "cmd.exe",
            "/c",
            "echo ok",
        ])
        .unwrap();

        assert!(matches!(
            cli.command,
            Command::Run {
                cpu_count: Some(4),
                memory_mib: Some(8192),
                ttl_seconds: Some(3600),
                keep_on_failure: true,
                ..
            }
        ));
        assert!(
            Cli::try_parse_from([
                "vmcell",
                "run",
                "--image",
                "windows-dev",
                "--keep",
                "--keep-on-failure",
                "--",
                "cmd.exe",
            ])
            .is_err()
        );
    }

    #[test]
    fn rejects_path_like_image_ids() {
        assert!(
            Cli::try_parse_from([
                "vmcell",
                "image",
                "add",
                "--id",
                "../foreign",
                "--path",
                "base.vhdx",
                "--guest-os",
                "windows",
            ])
            .is_err()
        );
    }

    #[test]
    fn parses_image_dependency_and_unregister_surfaces() {
        let dependencies =
            Cli::try_parse_from(["vmcell", "image", "dependencies", "windows-dev"]).unwrap();
        assert!(matches!(
            dependencies.command,
            Command::Image {
                command: ImageCommand::Dependencies { .. }
            }
        ));
        let unregister =
            Cli::try_parse_from(["vmcell", "image", "unregister", "windows-dev"]).unwrap();
        assert!(matches!(
            unregister.command,
            Command::Image {
                command: ImageCommand::Unregister { .. }
            }
        ));
    }

    #[test]
    fn parses_nested_provider_list_surface() {
        let cli = Cli::try_parse_from(["vmcell", "provider", "list"]).unwrap();
        assert!(matches!(
            cli.command,
            Command::Provider {
                command: ProviderCommand::List
            }
        ));
        assert!(Cli::try_parse_from(["vmcell", "provider-list"]).is_err());
    }

    #[test]
    fn parses_m2_exec_copy_artifact_and_gc_surfaces_without_password_argv() {
        let cell_id = CellId::new();
        let exec = Cli::try_parse_from([
            "vmcell",
            "exec",
            &cell_id.to_string(),
            "--username",
            "Administrator",
            "--password-stdin",
            "--",
            "cmd.exe",
            "/c",
            "echo ok",
        ])
        .unwrap();
        assert!(matches!(exec.command, Command::Exec { .. }));
        let prune = Cli::try_parse_from([
            "vmcell",
            "--lock-timeout-ms",
            "250",
            "artifact",
            "prune",
            "--older-than-seconds",
            "3600",
            "--max-artifacts",
            "8",
            "--dry-run",
        ])
        .unwrap();
        assert_eq!(prune.lock_timeout_ms, Some(250));
        assert!(matches!(
            prune.command,
            Command::Artifact {
                command: ArtifactCommand::Prune { dry_run: true, .. }
            }
        ));
        assert!(Cli::try_parse_from(["vmcell", "--lock-timeout-ms", "30001", "list"]).is_err());
        assert!(
            Cli::try_parse_from([
                "vmcell",
                "exec",
                &cell_id.to_string(),
                "--username",
                "Administrator",
                "--password",
                "secret",
                "--",
                "cmd.exe",
            ])
            .is_err()
        );
        assert!(
            Cli::try_parse_from([
                "vmcell",
                "copy-in",
                &cell_id.to_string(),
                "--source",
                "input.bin",
                "--destination",
                "input\\file.bin",
                "--username",
                "Administrator",
                "--password-stdin",
            ])
            .is_ok()
        );
        assert!(
            Cli::try_parse_from([
                "vmcell",
                "exec",
                &cell_id.to_string(),
                "--",
                "/usr/bin/true",
            ])
            .is_ok()
        );
        assert!(Cli::try_parse_from(["vmcell", "gc"]).is_ok());
        let operation_id = GuestOperationId::new();
        assert!(
            Cli::try_parse_from([
                "vmcell",
                "operation",
                "reconcile",
                &operation_id.to_string(),
            ])
            .is_ok()
        );
    }

    #[test]
    fn parses_qemu_image_create_and_explicit_tcg_policy() {
        let image = Cli::try_parse_from([
            "vmcell",
            "image",
            "add",
            "--id",
            "linux-qemu",
            "--path",
            "base.qcow2",
            "--guest-os",
            "linux",
            "--provider",
            "qemu",
        ])
        .unwrap();
        assert!(matches!(
            image.command,
            Command::Image {
                command: ImageCommand::Add {
                    provider: Some(CliProvider::Qemu),
                    ..
                }
            }
        ));
        let create = Cli::try_parse_from([
            "vmcell",
            "create",
            "--image",
            "linux-qemu",
            "--provider",
            "qemu",
            "--accelerator",
            "tcg",
            "--allow-tcg",
        ])
        .unwrap();
        assert!(matches!(
            create.command,
            Command::Create {
                provider: Some(CliProvider::Qemu),
                accelerator: Some(CliAccelerator::Tcg),
                allow_tcg: true,
                ..
            }
        ));
    }

    #[test]
    fn image_validate_accepts_exactly_one_registered_or_candidate_target() {
        assert!(
            Cli::try_parse_from([
                "vmcell",
                "image",
                "validate",
                "--path",
                "base.vhdx",
                "--guest-os",
                "windows",
            ])
            .is_ok()
        );
        assert!(
            Cli::try_parse_from([
                "vmcell",
                "image",
                "validate",
                "--id",
                "windows-dev",
                "--provider",
                "hyperv",
            ])
            .is_ok()
        );
        assert!(
            Cli::try_parse_from([
                "vmcell",
                "image",
                "validate",
                "--id",
                "windows-dev",
                "--path",
                "base.vhdx",
                "--guest-os",
                "windows",
            ])
            .is_err()
        );
        assert!(Cli::try_parse_from(["vmcell", "image", "validate"]).is_err());
    }

    #[test]
    fn root_help_carries_the_release_gated_windows_workflow_without_secret_argv() {
        let help = Cli::command().render_long_help().to_string();

        assert!(help.contains("Windows Human MVP examples"));
        assert!(help.contains("vmcell doctor"));
        assert!(help.contains("vmcell image validate --path BASE.vhdx"));
        assert!(help.contains("vmcell run --image windows-dev"));
        assert!(help.contains("--password-stdin"));
        assert!(help.contains("real execution remains release-gated"));
        assert!(!help.contains("--password secret"));
    }

    #[test]
    fn shell_parser_and_help_are_explicit_about_the_non_pty_password_protocol() {
        let cli = Cli::try_parse_from([
            "vmcell",
            "shell",
            "00000000-0000-0000-0000-000000000001",
            "--username",
            "Administrator",
            "--password-stdin",
        ])
        .unwrap();
        assert!(matches!(
            cli.command,
            Command::Shell {
                credential: CredentialArgs {
                    password_stdin: true,
                    ..
                },
                ..
            }
        ));
        assert!(
            Cli::try_parse_from([
                "vmcell",
                "shell",
                "00000000-0000-0000-0000-000000000001",
                "--username",
                "Administrator",
                "--password",
                "credential-sentinel",
            ])
            .is_err()
        );

        let mut command = Cli::command();
        let shell = command
            .find_subcommand_mut("shell")
            .expect("shell subcommand should exist");
        let help = shell.render_long_help().to_string();
        assert!(help.contains("not a PTY"));
        assert!(help.contains("does not preserve guest stdin"));
        assert!(help.contains("password is read from bounded stdin"));
        assert!(help.contains("attached Windows console"));
        assert!(!help.contains("credential-sentinel"));
    }

    #[test]
    fn status_command_and_empty_json_contract_are_stable() {
        let cli = Cli::try_parse_from(["vmcell", "status"]).unwrap();
        assert!(matches!(cli.command, Command::Status));

        let report = StatusReport::new(
            chrono::Utc::now(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
        );
        let json = serde_json::to_value(report).unwrap();
        assert_eq!(json["schema_version"], CLI_JSON_SCHEMA_VERSION);
        assert_eq!(json["contract"], STATUS_CONTRACT);
        assert_eq!(json["providers"], serde_json::json!([]));
        assert_eq!(json["cells"], serde_json::json!([]));
        assert_eq!(json["images"], serde_json::json!([]));
        assert_eq!(json["operations"], serde_json::json!([]));
    }

    #[test]
    fn status_image_contract_keeps_each_provider_variant_visible() {
        let image = ImageRecord {
            schema_version: 1,
            id: "portable-dev".parse().unwrap(),
            guest_os: GuestOs::Linux,
            guest_arch: Architecture::X86_64,
            variants: vec![
                crate::core::image::ImageVariant {
                    provider: "hyperv".to_owned(),
                    disk_format: "vhdx".to_owned(),
                    path: PathBuf::from("base.vhdx"),
                    sha256: "a".repeat(64),
                    file_size: 1,
                },
                crate::core::image::ImageVariant {
                    provider: "qemu".to_owned(),
                    disk_format: "qcow2".to_owned(),
                    path: PathBuf::from("base.qcow2"),
                    sha256: "b".repeat(64),
                    file_size: 2,
                },
            ],
            registered_at: chrono::Utc::now(),
        };
        let entry = StatusImageEntry {
            image,
            observations: vec![
                StatusImageVariantObservation {
                    provider: "hyperv".to_owned(),
                    observation: StatusImageObservation::ProviderUnavailable {
                        provider_status: ProviderProbeStatus::Unavailable,
                    },
                },
                StatusImageVariantObservation {
                    provider: "qemu".to_owned(),
                    observation: StatusImageObservation::ProviderUnavailable {
                        provider_status: ProviderProbeStatus::Unavailable,
                    },
                },
            ],
        };

        let json = serde_json::to_value(entry).unwrap();
        assert_eq!(json["observations"].as_array().unwrap().len(), 2);
        assert_eq!(json["observations"][0]["provider"], "hyperv");
        assert_eq!(json["observations"][1]["provider"], "qemu");
    }
}
