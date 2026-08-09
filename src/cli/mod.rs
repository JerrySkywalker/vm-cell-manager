use std::path::PathBuf;

use clap::{Args, Parser, Subcommand, ValueEnum};
use serde::Serialize;

use crate::core::cell::CellId;
use crate::core::guest::GuestOperationId;
use crate::core::image::{Architecture, GuestOs, ImageId};
use crate::guest::{
    DEFAULT_ACTION_TIMEOUT_SECONDS, DEFAULT_MAX_COPY_BYTES, DEFAULT_MAX_OUTPUT_BYTES,
    DEFAULT_READINESS_TIMEOUT_SECONDS, GuestPath, OverwritePolicy,
};
use crate::providers::{ProviderProbe, builtin_provider_probes};
use crate::state::StateStore;

#[derive(Debug, Parser)]
#[command(
    name = "vmcell",
    version,
    about = "Local disposable VM execution cells"
)]
pub struct Cli {
    /// Emit versioned machine-readable JSON.
    #[arg(long, global = true)]
    pub json: bool,

    /// Override the local state root.
    #[arg(long, global = true, value_name = "PATH")]
    pub state_root: Option<PathBuf>,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Probe the host and built-in providers without mutating host state.
    Doctor,

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

    /// Create one stopped, networkless Hyper-V cell.
    Create {
        #[arg(long)]
        image: ImageId,

        #[arg(long, default_value_t = 2)]
        cpu_count: u16,

        #[arg(long, default_value_t = 4096)]
        memory_mib: u64,

        #[arg(long)]
        ttl_seconds: Option<u64>,

        #[arg(long, value_enum, default_value_t = CliProvider::Hyperv)]
        provider: CliProvider,

        #[arg(long, value_enum)]
        accelerator: Option<CliAccelerator>,

        #[arg(long)]
        allow_tcg: bool,
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

    /// Execute a process in an exact-owned running Windows guest.
    Exec {
        cell_id: CellId,

        #[command(flatten)]
        credential: CredentialArgs,

        #[arg(long, default_value_t = DEFAULT_READINESS_TIMEOUT_SECONDS)]
        readiness_timeout_seconds: u64,

        #[arg(long, default_value_t = DEFAULT_ACTION_TIMEOUT_SECONDS)]
        timeout_seconds: u64,

        #[arg(long, default_value_t = DEFAULT_MAX_OUTPUT_BYTES)]
        max_output_bytes: u64,

        #[arg(required = true, trailing_var_arg = true, allow_hyphen_values = true)]
        command: Vec<String>,
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

        #[arg(long, default_value_t = DEFAULT_READINESS_TIMEOUT_SECONDS)]
        readiness_timeout_seconds: u64,

        #[arg(long, default_value_t = DEFAULT_ACTION_TIMEOUT_SECONDS)]
        timeout_seconds: u64,

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

        #[arg(long, default_value_t = DEFAULT_READINESS_TIMEOUT_SECONDS)]
        readiness_timeout_seconds: u64,

        #[arg(long, default_value_t = DEFAULT_ACTION_TIMEOUT_SECONDS)]
        timeout_seconds: u64,

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

#[derive(Debug, Subcommand)]
pub enum ArtifactCommand {
    /// Collect one or more guest-workspace files into one atomic artifact record.
    Collect {
        cell_id: CellId,

        #[arg(long = "path", required = true)]
        paths: Vec<GuestPath>,

        #[command(flatten)]
        credential: CredentialArgs,

        #[arg(long, default_value_t = DEFAULT_READINESS_TIMEOUT_SECONDS)]
        readiness_timeout_seconds: u64,

        #[arg(long, default_value_t = DEFAULT_ACTION_TIMEOUT_SECONDS)]
        timeout_seconds: u64,

        #[arg(long, default_value_t = DEFAULT_MAX_COPY_BYTES)]
        max_bytes_per_file: u64,
    },

    /// Inspect one committed artifact manifest.
    Inspect {
        cell_id: CellId,
        operation_id: GuestOperationId,
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
    /// Register an immutable Hyper-V VHDX base.
    Add {
        #[arg(long)]
        id: ImageId,

        #[arg(long)]
        path: PathBuf,

        #[arg(long, value_enum)]
        guest_os: CliGuestOs,

        #[arg(long, value_enum, default_value_t = CliArchitecture::X86_64)]
        guest_arch: CliArchitecture,

        #[arg(long, value_enum, default_value_t = CliProvider::Hyperv)]
        provider: CliProvider,
    },

    /// List registered images.
    List,

    /// Inspect one registered image manifest.
    Inspect { id: ImageId },
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
    pub host_os: &'static str,
    pub host_arch: &'static str,
    pub state_root: PathBuf,
    pub providers: Vec<ProviderProbe>,
}

impl DoctorReport {
    #[must_use]
    pub fn collect(state_root: Option<PathBuf>) -> Self {
        Self {
            schema_version: 1,
            host_os: std::env::consts::OS,
            host_arch: std::env::consts::ARCH,
            state_root: state_root.unwrap_or_else(StateStore::default_root),
            providers: builtin_provider_probes(),
        }
    }
}

#[derive(Debug, Serialize)]
pub struct ErrorEnvelope<'a> {
    pub schema_version: u32,
    pub error: ErrorBody<'a>,
}

#[derive(Debug, Serialize)]
pub struct ErrorBody<'a> {
    pub category: &'static str,
    pub message: &'a str,
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
            schema_version: 1,
            items,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
                cpu_count: 4,
                memory_mib: 8192,
                ..
            }
        ));
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
                    provider: CliProvider::Qemu,
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
                provider: CliProvider::Qemu,
                accelerator: Some(CliAccelerator::Tcg),
                allow_tcg: true,
                ..
            }
        ));
    }
}
