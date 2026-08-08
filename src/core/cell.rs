use std::fmt;
use std::str::FromStr;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

use super::image::{ImageBinding, ImageId};
use super::ownership::{CellOwnership, ProviderObjectIdentity};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct CellId(pub Uuid);

impl CellId {
    #[must_use]
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl Default for CellId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for CellId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

impl FromStr for CellId {
    type Err = CellIdError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Uuid::parse_str(value).map(Self).map_err(CellIdError)
    }
}

#[derive(Debug, Error)]
#[error("invalid cell id: {0}")]
pub struct CellIdError(#[source] uuid::Error);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CellState {
    Creating,
    Stopped,
    Running,
    Destroying,
    Destroyed,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CellPhase {
    IntentRecorded,
    OverlayCreated,
    ProviderObjectCreated,
    Ready,
    Destroying,
    Destroyed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CellSpec {
    pub image: ImageId,
    pub provider: Option<String>,
    pub cpu_count: u16,
    pub memory_mib: u64,
    pub ttl_seconds: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CellRecord {
    pub schema_version: u32,
    pub id: CellId,
    pub provider: String,
    pub spec: CellSpec,
    pub image: ImageBinding,
    pub ownership: CellOwnership,
    pub provider_object: Option<ProviderObjectIdentity>,
    pub state: CellState,
    pub phase: CellPhase,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub expires_at: Option<DateTime<Utc>>,
    pub last_error: Option<String>,
}
