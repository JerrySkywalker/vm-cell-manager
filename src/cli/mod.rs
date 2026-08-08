use std::path::PathBuf;

use clap::{Parser, Subcommand, ValueEnum};
use serde::Serialize;

use crate::core::cell::CellId;
use crate::core::image::{Architecture, GuestOs, ImageId};
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
}
