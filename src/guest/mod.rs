pub mod powershell_direct;
pub mod qga;
pub mod ssh;

use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GuestCommand {
    pub program: String,
    pub args: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GuestCommandResult {
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
}

#[derive(Debug, Error)]
pub enum GuestIoError {
    #[error("guest transport is not implemented yet: {0}")]
    NotImplemented(&'static str),
}

pub trait GuestTransport: Send + Sync {
    fn name(&self) -> &'static str;

    fn exec(&self, _command: &GuestCommand) -> Result<GuestCommandResult, GuestIoError> {
        Err(GuestIoError::NotImplemented(self.name()))
    }
}
