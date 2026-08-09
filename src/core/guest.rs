use std::fmt;
use std::str::FromStr;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

use super::cell::CellId;

pub const GUEST_OPERATION_SCHEMA_VERSION: u32 = 1;
pub const ARTIFACT_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct GuestOperationId(pub Uuid);

impl GuestOperationId {
    #[must_use]
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl Default for GuestOperationId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for GuestOperationId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

impl FromStr for GuestOperationId {
    type Err = GuestOperationIdError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Uuid::parse_str(value)
            .map(Self)
            .map_err(GuestOperationIdError)
    }
}

#[derive(Debug, Error)]
#[error("invalid guest operation id: {0}")]
pub struct GuestOperationIdError(#[source] uuid::Error);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GuestOperationKind {
    Exec,
    CopyIn,
    CopyOut,
    ArtifactCollect,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GuestOperationPhase {
    IntentRecorded,
    TransportActive,
    ArtifactCommitted,
    Completed,
    ArtifactPruned,
    Failed,
}

impl GuestOperationPhase {
    #[must_use]
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Completed | Self::ArtifactPruned | Self::Failed)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GuestFailureClass {
    Interrupted,
    GuestNotReady,
    Authentication,
    Session,
    Timeout,
    OutputLimit,
    InvalidEncoding,
    PathViolation,
    PartialCopy,
    OwnershipChanged,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GuestOperationRecord {
    pub schema_version: u32,
    pub id: GuestOperationId,
    pub cell_id: CellId,
    pub kind: GuestOperationKind,
    pub phase: GuestOperationPhase,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
    pub failure: Option<GuestFailureClass>,
    pub exit_code: Option<i32>,
    pub stdout_bytes: Option<u64>,
    pub stderr_bytes: Option<u64>,
    pub artifact_id: Option<GuestOperationId>,
}

impl GuestOperationRecord {
    #[must_use]
    pub fn intent(cell_id: CellId, kind: GuestOperationKind, now: DateTime<Utc>) -> Self {
        Self {
            schema_version: GUEST_OPERATION_SCHEMA_VERSION,
            id: GuestOperationId::new(),
            cell_id,
            kind,
            phase: GuestOperationPhase::IntentRecorded,
            created_at: now,
            updated_at: now,
            completed_at: None,
            failure: None,
            exit_code: None,
            stdout_bytes: None,
            stderr_bytes: None,
            artifact_id: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArtifactEntry {
    pub guest_path: String,
    pub host_relative_path: String,
    pub sha256: String,
    pub size: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArtifactRecord {
    pub schema_version: u32,
    pub id: GuestOperationId,
    pub cell_id: CellId,
    pub created_at: DateTime<Utc>,
    pub entries: Vec<ArtifactEntry>,
}

pub const MAX_ARTIFACT_FILES: usize = 16;
pub const MAX_ARTIFACT_FILE_BYTES: u64 = 64 * 1024 * 1024;
pub const MAX_ARTIFACT_TOTAL_BYTES: u64 = 1024 * 1024 * 1024;
