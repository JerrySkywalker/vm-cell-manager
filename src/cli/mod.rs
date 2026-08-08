use std::path::PathBuf;

use clap::{Parser, Subcommand};
use serde::Serialize;

use crate::providers::{ProviderProbe, builtin_provider_probes};
use crate::state::StateStore;

#[derive(Debug, Parser)]
#[command(name = "vmcell", version, about = "Local disposable VM execution cells")]
pub struct Cli {
    /// Emit machine-readable JSON where supported.
    #[arg(long, global = true)]
    pub json: bool,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Probe the host and built-in providers without mutating host state.
    Doctor,

    /// List built-in provider probes.
    #[command(name = "provider-list")]
    ProviderList,
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
    pub fn collect() -> Self {
        Self {
            schema_version: 1,
            host_os: std::env::consts::OS,
            host_arch: std::env::consts::ARCH,
            state_root: StateStore::default_root(),
            providers: builtin_provider_probes(),
        }
    }
}
