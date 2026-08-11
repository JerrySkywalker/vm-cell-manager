//! Stable correlation identity for one declarative job invocation.
//!
//! This module intentionally contains no lifecycle state or authority.  A job
//! identity is attached to the existing run request and result so that a
//! versioned result can bind an execution to the exact parsed job-spec bytes.

use std::fmt;
use std::str::FromStr;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

/// The first public job-result metadata schema.
pub const JOB_RESULT_SCHEMA_VERSION: u32 = 1;
/// Stable identifier for metadata attached to a job-backed run result.
pub const JOB_RESULT_CONTRACT: &str = "vmcell.job-result.v1";

/// A fresh correlation identity for one invocation of a validated job spec.
///
/// It never confers ownership of a cell, provider process, or guest action.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct JobId(pub Uuid);

impl JobId {
    #[must_use]
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl Default for JobId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for JobId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

impl FromStr for JobId {
    type Err = JobIdError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Uuid::parse_str(value).map(Self).map_err(JobIdError)
    }
}

#[derive(Debug, Error)]
#[error("invalid job id: {0}")]
pub struct JobIdError(#[source] uuid::Error);

/// Non-serialized execution context established before the lifecycle call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JobRunContext {
    job_id: JobId,
    job_spec_sha256: String,
    started_at: DateTime<Utc>,
}

impl JobRunContext {
    /// Bind a fresh invocation to an exact lower-case SHA-256 digest of the
    /// validated job-spec source bytes.
    pub fn new(
        job_spec_sha256: impl Into<String>,
        started_at: DateTime<Utc>,
    ) -> Result<Self, JobRunContextError> {
        let job_spec_sha256 = job_spec_sha256.into();
        if !is_lowercase_sha256(&job_spec_sha256) {
            return Err(JobRunContextError::InvalidSpecDigest);
        }
        Ok(Self {
            job_id: JobId::new(),
            job_spec_sha256,
            started_at,
        })
    }

    #[must_use]
    pub fn job_id(&self) -> JobId {
        self.job_id
    }

    #[must_use]
    pub fn result_metadata(&self, completed_at: DateTime<Utc>) -> JobResultMetadata {
        let completed_at = completed_at.max(self.started_at);
        let elapsed_milliseconds = completed_at
            .signed_duration_since(self.started_at)
            .num_milliseconds()
            .max(0) as u64;
        JobResultMetadata {
            schema_version: JOB_RESULT_SCHEMA_VERSION,
            contract: JOB_RESULT_CONTRACT.to_owned(),
            job_id: self.job_id,
            job_spec_sha256: self.job_spec_sha256.clone(),
            started_at: self.started_at,
            completed_at,
            elapsed_milliseconds,
        }
    }
}

#[derive(Debug, Error)]
pub enum JobRunContextError {
    #[error("job execution requires an exact lower-case SHA-256 job-spec digest")]
    InvalidSpecDigest,
}

/// Safe, versioned identity and timing metadata emitted with a job-backed run
/// success or failure.  It deliberately excludes the job document, command,
/// credentials, host paths, provider paths, and raw probe evidence.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JobResultMetadata {
    pub schema_version: u32,
    pub contract: String,
    pub job_id: JobId,
    pub job_spec_sha256: String,
    pub started_at: DateTime<Utc>,
    pub completed_at: DateTime<Utc>,
    pub elapsed_milliseconds: u64,
}

fn is_lowercase_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
}

#[cfg(test)]
mod tests {
    use chrono::{Duration, TimeZone};

    use super::*;

    #[test]
    fn metadata_binds_exact_digest_and_fresh_identity() {
        let started = Utc.with_ymd_and_hms(2026, 8, 11, 0, 0, 0).unwrap();
        let first = JobRunContext::new("a".repeat(64), started).unwrap();
        let second = JobRunContext::new("a".repeat(64), started).unwrap();
        let result = first.result_metadata(started + Duration::milliseconds(17));

        assert_ne!(first.job_id(), second.job_id());
        assert_eq!(result.schema_version, JOB_RESULT_SCHEMA_VERSION);
        assert_eq!(result.contract, JOB_RESULT_CONTRACT);
        assert_eq!(result.job_spec_sha256, "a".repeat(64));
        assert_eq!(result.elapsed_milliseconds, 17);
        assert!(!serde_json::to_string(&result).unwrap().contains("command"));

        let normalized = first.result_metadata(started - Duration::milliseconds(1));
        assert_eq!(normalized.started_at, normalized.completed_at);
        assert_eq!(normalized.elapsed_milliseconds, 0);
    }

    #[test]
    fn malformed_digest_is_rejected() {
        let now = Utc::now();
        let uppercase = "A".repeat(64);
        let invalid_hex = "g".repeat(64);
        let short = "a".repeat(63);
        for value in ["", uppercase.as_str(), invalid_hex.as_str(), short.as_str()] {
            assert!(matches!(
                JobRunContext::new(value, now),
                Err(JobRunContextError::InvalidSpecDigest)
            ));
        }
    }
}
