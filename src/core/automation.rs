use serde::{Deserialize, Serialize};

pub const AUTOMATION_SCHEMA_VERSION: u32 = 1;
pub const DOCTOR_CONTRACT: &str = "vmcell.doctor.v1";
pub const STATUS_CONTRACT: &str = "vmcell.status.v1";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OwnershipClassification {
    Proven,
    PhaseProven,
    Unproven,
    Mismatch,
    NotApplicable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RequiredAction {
    None,
    RetryLifecycle,
    RecoveryRequired,
    ManualReview,
}
