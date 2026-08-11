use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use directories::ProjectDirs;
use fs2::FileExt;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;
use uuid::Uuid;

use crate::core::cell::{
    CELL_SCHEMA_VERSION, CellId, CellRecord, JOB_CORRELATED_CELL_SCHEMA_VERSION,
    MAX_CELL_SCHEMA_VERSION,
};
use crate::core::guest::{
    ARTIFACT_SCHEMA_VERSION, ArtifactRecord, GUEST_OPERATION_SCHEMA_VERSION, GuestOperationId,
    GuestOperationRecord, JOB_CORRELATED_ARTIFACT_SCHEMA_VERSION,
    JOB_CORRELATED_GUEST_OPERATION_SCHEMA_VERSION, MAX_ARTIFACT_FILE_BYTES, MAX_ARTIFACT_FILES,
    MAX_ARTIFACT_SCHEMA_VERSION, MAX_ARTIFACT_TOTAL_BYTES, MAX_GUEST_OPERATION_SCHEMA_VERSION,
};
use crate::core::image::{IMAGE_SCHEMA_VERSION, ImageId, ImageRecord};
use crate::core::job::JobCorrelation;
use crate::core::ownership::OWNERSHIP_MARKER_SCHEMA;

pub const INSTALL_SCHEMA_VERSION: u32 = 1;
pub const STATE_COMPATIBILITY_SCHEMA_VERSION: u32 = 1;
/// Highest durable record format understood by this binary.
pub const DURABLE_STATE_FORMAT_VERSION: u32 = 2;
/// Legacy/direct roots remain format 1 until they contain job-correlated v2
/// records. `state check` reports the active maximum, not merely this ceiling.
pub const LEGACY_DURABLE_STATE_FORMAT_VERSION: u32 = 1;
pub const STATE_COMPATIBILITY_CONTRACT: &str = "vmcell.state-compatibility.v1";
pub const MAX_MUTATION_LOCK_TIMEOUT: Duration = Duration::from_secs(30);
const MUTATION_LOCK_POLL_INTERVAL: Duration = Duration::from_millis(25);
const REDACTED_LEGACY_ERROR_CODE: &str = "vmcell.legacy.redacted";

#[derive(Debug, Clone)]
pub struct StateStore {
    root: PathBuf,
    mutation_lock_timeout: Duration,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstallationRecord {
    pub schema_version: u32,
    pub install_id: Uuid,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StateCompatibilityStatus {
    Empty,
    Compatible,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct StateCompatibilityCounts {
    pub installations: u64,
    pub images: u64,
    pub cells: u64,
    pub guest_operations: u64,
    pub artifacts: u64,
}

impl StateCompatibilityCounts {
    fn is_empty(&self) -> bool {
        self.installations == 0
            && self.images == 0
            && self.cells == 0
            && self.guest_operations == 0
            && self.artifacts == 0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct StateCompatibilityReport {
    pub schema_version: u32,
    pub contract: &'static str,
    pub durable_state_format_version: u32,
    pub status: StateCompatibilityStatus,
    pub checked_at: DateTime<Utc>,
    pub counts: StateCompatibilityCounts,
}

#[derive(Debug, Error)]
pub enum StateError {
    #[error("state I/O failed for {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("invalid state JSON in {path}: {source}")]
    Json {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },

    #[error("state object not found: {0}")]
    NotFound(PathBuf),

    #[error("state object already exists: {0}")]
    AlreadyExists(PathBuf),

    #[error("another vmcell mutation is active")]
    MutationBusy,

    #[error("refusing unsafe runtime path: {0}")]
    UnsafeRuntimePath(PathBuf),

    #[error("persisted {kind} identity in {path} does not match requested id {expected}")]
    IdentityMismatch {
        kind: &'static str,
        path: PathBuf,
        expected: String,
    },

    #[error("unsupported {kind} schema version in {path}: expected {expected}, found {actual}")]
    UnsupportedSchema {
        kind: &'static str,
        path: PathBuf,
        expected: u32,
        actual: u32,
    },

    #[error(
        "durable {kind} schema requires a different vmcell version: supported {supported}, found {actual}"
    )]
    UpgradeRequired {
        kind: &'static str,
        supported: u32,
        actual: u32,
    },

    #[error("artifact integrity check failed for {path}: {reason}")]
    ArtifactIntegrity { path: PathBuf, reason: &'static str },

    #[error("guest operation integrity check failed for {path}: {reason}")]
    GuestOperationIntegrity { path: PathBuf, reason: &'static str },

    #[error("job correlation integrity check failed for {path}: {reason}")]
    JobCorrelationIntegrity { path: PathBuf, reason: &'static str },
}

pub struct MutationGuard {
    file: File,
    lock_path: PathBuf,
    state_root: (PathBuf, File),
    state_directories: Vec<(PathBuf, File)>,
    process_key: PathBuf,
}

pub(crate) struct InstallationAuthority {
    _file: File,
    record: InstallationRecord,
}

impl InstallationAuthority {
    pub(crate) fn record(&self) -> &InstallationRecord {
        &self.record
    }
}

pub(crate) struct CellRuntimeGuard {
    cell_id: CellId,
    state_root: PathBuf,
    runtime_root: PathBuf,
    cell_root: PathBuf,
    configuration_path: PathBuf,
    overlay_path: PathBuf,
    state_handle: File,
    runtime_handle: File,
    cell_handle: Option<File>,
    configuration_handle: Option<File>,
}

pub(crate) struct ArtifactGuard {
    cell_id: CellId,
    operation_id: GuestOperationId,
    root: PathBuf,
    files_root: PathBuf,
    state_handle: File,
    artifacts_handle: File,
    cell_handle: File,
    operation_handle: File,
    files_handle: File,
}

impl CellRuntimeGuard {
    pub(crate) fn cell_id(&self) -> CellId {
        self.cell_id
    }

    pub(crate) fn configuration_path(&self) -> &Path {
        &self.configuration_path
    }

    pub(crate) fn overlay_path(&self) -> &Path {
        &self.overlay_path
    }

    pub(crate) fn validate_filesystem_identity(&self) -> Result<(), StateError> {
        validate_open_path_identity(&self.state_root, &self.state_handle)?;
        validate_open_path_identity(&self.runtime_root, &self.runtime_handle)?;
        let cell_handle = self
            .cell_handle
            .as_ref()
            .ok_or_else(|| StateError::UnsafeRuntimePath(self.cell_root.clone()))?;
        validate_open_path_identity(&self.cell_root, cell_handle)?;
        let configuration_handle = self
            .configuration_handle
            .as_ref()
            .ok_or_else(|| StateError::UnsafeRuntimePath(self.configuration_path.clone()))?;
        validate_open_path_identity(&self.configuration_path, configuration_handle)?;
        validate_runtime_chain(&self.state_root, &self.cell_root)?;
        if self.configuration_path.parent() != Some(self.cell_root.as_path())
            || self.overlay_path.parent() != Some(self.cell_root.as_path())
        {
            return Err(StateError::UnsafeRuntimePath(self.cell_root.clone()));
        }
        Ok(())
    }
}

impl Drop for MutationGuard {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.file);
        if let Ok(mut roots) = process_mutation_roots().lock() {
            roots.remove(&self.process_key);
        }
    }
}

impl MutationGuard {
    pub(crate) fn validate_filesystem_identity(&self) -> Result<(), StateError> {
        validate_open_path_identity(&self.lock_path, &self.file)?;
        validate_open_path_identity(&self.state_root.0, &self.state_root.1)?;
        for (path, file) in &self.state_directories {
            validate_open_path_identity(path, file)?;
        }
        Ok(())
    }
}

impl StateStore {
    #[must_use]
    pub fn new(root: PathBuf) -> Self {
        let root = if root.is_absolute() {
            root
        } else {
            std::env::current_dir()
                .map(|directory| directory.join(&root))
                .unwrap_or(root)
        };
        Self {
            root,
            mutation_lock_timeout: Duration::ZERO,
        }
    }

    #[must_use]
    pub fn with_mutation_lock_timeout(mut self, timeout: Duration) -> Self {
        self.mutation_lock_timeout = timeout.min(MAX_MUTATION_LOCK_TIMEOUT);
        self
    }

    #[must_use]
    pub fn default_root() -> PathBuf {
        ProjectDirs::from("dev", "vmcell", "VM Cell Manager")
            .map(|dirs| dirs.data_local_dir().to_path_buf())
            .unwrap_or_else(|| PathBuf::from(".vmcell"))
    }

    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Validate every active core durable record without creating state,
    /// contacting a provider, replaying guest work, or rewriting older
    /// compatible JSON. Tombstoned or quarantined artifact subtrees are not
    /// compatibility evidence and remain subject to their recovery paths.
    pub fn check_compatibility(&self) -> Result<StateCompatibilityReport, StateError> {
        let _state_root = match fs::symlink_metadata(&self.root) {
            Ok(_) => Some(open_ordinary_directory(&self.root)?),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(source) => return Err(io_error(&self.root, source)),
        };
        let installation_path = self.root.join("installation.json");
        let installations = match fs::symlink_metadata(&installation_path) {
            Ok(_) => {
                self.load_installation().map_err(as_upgrade_required)?;
                1
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => 0,
            Err(source) => return Err(io_error(&installation_path, source)),
        };
        let images = self.list_images().map_err(as_upgrade_required)?;
        let cells = self.list_cells().map_err(as_upgrade_required)?;
        validate_unique_cell_job_ids(&self.root, &cells).map_err(as_upgrade_required)?;
        let operations = self.list_guest_operations().map_err(as_upgrade_required)?;
        validate_job_operation_bindings(&self.root, &cells, &operations)
            .map_err(as_upgrade_required)?;
        let mut artifacts = 0_u64;
        let mut durable_state_format_version = cells
            .iter()
            .map(|cell| cell.schema_version)
            .chain(operations.iter().map(|operation| operation.schema_version))
            .max()
            .unwrap_or(LEGACY_DURABLE_STATE_FORMAT_VERSION);
        for operation in &operations {
            if operation.artifact_id == Some(operation.id) && operation.artifact_pruned_at.is_none()
            {
                let artifact = self
                    .load_artifact(operation.cell_id, operation.id)
                    .map_err(as_upgrade_required)?;
                validate_artifact_job_binding(
                    &self
                        .artifact_root(operation.cell_id, operation.id)
                        .join("manifest.json"),
                    operation,
                    &artifact,
                )
                .map_err(as_upgrade_required)?;
                durable_state_format_version =
                    durable_state_format_version.max(artifact.schema_version);
                artifacts = artifacts.saturating_add(1);
            }
        }
        let counts = StateCompatibilityCounts {
            installations,
            images: images.len() as u64,
            cells: cells.len() as u64,
            guest_operations: operations.len() as u64,
            artifacts,
        };
        let status = if counts.is_empty() {
            StateCompatibilityStatus::Empty
        } else {
            StateCompatibilityStatus::Compatible
        };
        Ok(StateCompatibilityReport {
            schema_version: STATE_COMPATIBILITY_SCHEMA_VERSION,
            contract: STATE_COMPATIBILITY_CONTRACT,
            durable_state_format_version,
            status,
            checked_at: Utc::now(),
            counts,
        })
    }

    pub(crate) fn acquire_mutation_lock(&self) -> Result<MutationGuard, StateError> {
        let deadline = Instant::now()
            .checked_add(self.mutation_lock_timeout)
            .unwrap_or_else(Instant::now);
        loop {
            match self.try_acquire_mutation_lock() {
                Err(StateError::MutationBusy) if Instant::now() < deadline => {
                    let remaining = deadline.saturating_duration_since(Instant::now());
                    thread::sleep(remaining.min(MUTATION_LOCK_POLL_INTERVAL));
                }
                result => return result,
            }
        }
    }

    fn try_acquire_mutation_lock(&self) -> Result<MutationGuard, StateError> {
        ensure_directory(&self.root)?;
        let state_root_handle = open_ordinary_directory(&self.root)?;
        let mut state_directories = Vec::new();
        for name in [
            "locks",
            "images",
            "cells",
            "runtime",
            "operations",
            "artifacts",
        ] {
            let directory = self.root.join(name);
            create_direct_child_directory(&directory)?;
            state_directories.push((directory.clone(), open_ordinary_directory(&directory)?));
        }
        let lock_dir = self.root.join("locks");
        let process_key = self
            .root
            .canonicalize()
            .map_err(|source| io_error(&self.root, source))?;
        {
            let mut roots = process_mutation_roots()
                .lock()
                .map_err(|_| StateError::MutationBusy)?;
            if !roots.insert(process_key.clone()) {
                return Err(StateError::MutationBusy);
            }
        }

        let lock_path = lock_dir.join("mutation.lock");
        let file = match open_mutation_lock_file(&lock_path) {
            Ok(file) => file,
            Err(error) => {
                release_process_mutation_root(&process_key);
                return Err(error);
            }
        };

        if let Err(source) = file.try_lock_exclusive() {
            release_process_mutation_root(&process_key);
            if is_lock_contention(&source) {
                return Err(StateError::MutationBusy);
            } else {
                return Err(io_error(&lock_path, source));
            }
        }

        if let Err(error) = validate_open_path_identity(&lock_path, &file) {
            let _ = FileExt::unlock(&file);
            release_process_mutation_root(&process_key);
            return Err(error);
        }

        Ok(MutationGuard {
            file,
            lock_path,
            state_root: (self.root.clone(), state_root_handle),
            state_directories,
            process_key,
        })
    }

    pub(crate) fn installation(&self) -> Result<InstallationRecord, StateError> {
        let path = self.root.join("installation.json");
        if path.exists() {
            return self.load_installation();
        }

        ensure_directory(&self.root)?;
        let record = InstallationRecord {
            schema_version: INSTALL_SCHEMA_VERSION,
            install_id: Uuid::new_v4(),
        };
        write_json_new(&path, &record)?;
        Ok(record)
    }

    /// Load the existing installation identity without creating a replacement.
    pub fn load_installation(&self) -> Result<InstallationRecord, StateError> {
        let path = self.root.join("installation.json");
        let record: InstallationRecord = read_json(&path)?;
        ensure_schema(
            &path,
            "installation record",
            record.schema_version,
            INSTALL_SCHEMA_VERSION,
        )?;
        Ok(record)
    }

    pub(crate) fn acquire_installation_authority(
        &self,
    ) -> Result<InstallationAuthority, StateError> {
        let path = self.root.join("installation.json");
        let mut file = open_state_file_for_authority(&path)?;
        let record: InstallationRecord = read_json_from_file(&path, &mut file)?;
        ensure_schema(
            &path,
            "installation record",
            record.schema_version,
            INSTALL_SCHEMA_VERSION,
        )?;
        Ok(InstallationAuthority {
            _file: file,
            record,
        })
    }

    pub(crate) fn save_image_new(&self, record: &ImageRecord) -> Result<(), StateError> {
        let path = self.image_path(&record.id);
        validate_image_schema(&path, record)?;
        write_json_new(&path, record)
    }

    pub fn load_image(&self, image_id: &ImageId) -> Result<ImageRecord, StateError> {
        let path = self.image_path(image_id);
        let record = read_json(&path)?;
        validate_image_schema(&path, &record)?;
        if &record.id != image_id {
            return Err(StateError::IdentityMismatch {
                kind: "image record",
                path,
                expected: image_id.to_string(),
            });
        }
        Ok(record)
    }

    pub fn list_images(&self) -> Result<Vec<ImageRecord>, StateError> {
        read_json_directory(&self.root.join("images"), validate_image_schema)
    }

    pub(crate) fn validate_image_removal_candidate(
        &self,
        image_id: &ImageId,
    ) -> Result<(), StateError> {
        let path = self.image_path(image_id);
        let mut file = open_state_file_for_authority(&path)?;
        let record: ImageRecord = read_json_from_file(&path, &mut file)?;
        validate_image_schema(&path, &record)?;
        if &record.id != image_id {
            return Err(StateError::IdentityMismatch {
                kind: "image record",
                path,
                expected: image_id.to_string(),
            });
        }
        validate_image_record_for_metadata_removal(&path, &record, &file)
    }

    pub(crate) fn remove_image_record(
        &self,
        mutation: &MutationGuard,
        image_id: &ImageId,
    ) -> Result<bool, StateError> {
        mutation.validate_filesystem_identity()?;
        let images_root = self.root.join("images");
        let images_handle = open_ordinary_directory(&images_root)?;
        let path = self.image_path(image_id);
        let mut file = match open_state_file_for_authority(&path) {
            Ok(file) => file,
            Err(StateError::NotFound(_)) => return Ok(false),
            Err(error) => return Err(error),
        };
        let record: ImageRecord = read_json_from_file(&path, &mut file)?;
        validate_image_schema(&path, &record)?;
        if &record.id != image_id {
            return Err(StateError::IdentityMismatch {
                kind: "image record",
                path,
                expected: image_id.to_string(),
            });
        }
        validate_image_record_for_metadata_removal(&path, &record, &file)?;
        validate_open_path_identity(&path, &file)?;
        let physical_state_root = self
            .root
            .canonicalize()
            .map_err(|source| io_error(&self.root, source))?;
        let physical_images_root = images_root
            .canonicalize()
            .map_err(|source| io_error(&images_root, source))?;
        let physical_path = path
            .canonicalize()
            .map_err(|source| io_error(&path, source))?;
        if physical_images_root.parent() != Some(physical_state_root.as_path())
            || physical_path.parent() != Some(physical_images_root.as_path())
        {
            return Err(StateError::UnsafeRuntimePath(path));
        }

        #[cfg(test)]
        abort_at_test_checkpoint("before_image_remove");
        drop(file);
        let retired_path = physical_images_root.join(format!(
            "{}.json.unregistered-{}",
            image_id.as_str(),
            Uuid::new_v4()
        ));
        fs::rename(&physical_path, &retired_path)
            .map_err(|source| io_error(&physical_path, source))?;
        #[cfg(not(windows))]
        images_handle
            .sync_all()
            .map_err(|source| io_error(&physical_images_root, source))?;
        #[cfg(test)]
        abort_at_test_checkpoint("after_image_remove");
        mutation.validate_filesystem_identity()?;
        drop(images_handle);
        Ok(true)
    }

    pub(crate) fn save_cell(&self, record: &CellRecord) -> Result<(), StateError> {
        let path = self.cell_path(record.id);
        let mut record = record.clone();
        redact_cell_diagnostic(&mut record);
        validate_cell_schema(&path, &record)?;
        match self.load_cell(record.id) {
            Ok(existing) if existing.job != record.job => {
                return Err(StateError::JobCorrelationIntegrity {
                    path,
                    reason: "cell job correlation is immutable",
                });
            }
            Ok(_) | Err(StateError::NotFound(_)) => {}
            Err(error) => return Err(error),
        }
        self.validate_unique_cell_job_id(&path, &record)?;
        write_json_atomic(&path, &record)
    }

    pub fn load_cell(&self, cell_id: CellId) -> Result<CellRecord, StateError> {
        let path = self.cell_path(cell_id);
        let mut record = read_json(&path)?;
        validate_cell_schema(&path, &record)?;
        if record.id != cell_id {
            return Err(StateError::IdentityMismatch {
                kind: "cell record",
                path,
                expected: cell_id.to_string(),
            });
        }
        redact_cell_diagnostic(&mut record);
        self.validate_unique_cell_job_id(&path, &record)?;
        Ok(record)
    }

    pub fn list_cells(&self) -> Result<Vec<CellRecord>, StateError> {
        let mut records = read_json_directory(&self.root.join("cells"), validate_cell_schema)?;
        validate_unique_cell_job_ids(&self.root, &records)?;
        records.iter_mut().for_each(redact_cell_diagnostic);
        Ok(records)
    }

    fn validate_unique_cell_job_id(
        &self,
        path: &Path,
        record: &CellRecord,
    ) -> Result<(), StateError> {
        let cells = read_json_directory(&self.root.join("cells"), validate_cell_schema)?;
        let mut cells = cells
            .into_iter()
            .filter(|cell| cell.id != record.id)
            .collect::<Vec<_>>();
        cells.push(record.clone());
        validate_unique_cell_job_ids_for_path(path, &cells)
    }

    pub(crate) fn save_guest_operation(
        &self,
        record: &GuestOperationRecord,
    ) -> Result<(), StateError> {
        let path = self.guest_operation_path(record.id);
        validate_guest_operation_schema(&path, record)?;
        match self.load_guest_operation(record.id) {
            Ok(existing) if existing.job_id != record.job_id => {
                return Err(StateError::JobCorrelationIntegrity {
                    path,
                    reason: "guest operation job correlation is immutable",
                });
            }
            Ok(_) | Err(StateError::NotFound(_)) => {}
            Err(error) => return Err(error),
        }
        self.validate_operation_parent_binding_for_write(&path, record)?;
        write_json_atomic(&path, record)
    }

    pub fn load_guest_operation(
        &self,
        operation_id: GuestOperationId,
    ) -> Result<GuestOperationRecord, StateError> {
        let path = self.guest_operation_path(operation_id);
        let record = read_json(&path)?;
        validate_guest_operation_schema(&path, &record)?;
        if record.id != operation_id {
            return Err(StateError::IdentityMismatch {
                kind: "guest operation record",
                path,
                expected: operation_id.to_string(),
            });
        }
        self.validate_operation_parent_binding(&path, &record)?;
        Ok(record)
    }

    pub fn list_guest_operations(&self) -> Result<Vec<GuestOperationRecord>, StateError> {
        let records = read_json_directory(
            &self.root.join("operations"),
            validate_guest_operation_schema,
        )?;
        for record in &records {
            self.validate_operation_parent_binding(&self.guest_operation_path(record.id), record)?;
        }
        Ok(records)
    }

    /// Require the exact parent-cell binding before a caller changes durable
    /// operation or artifact state. Read compatibility for a historical v1
    /// orphan intentionally does not extend to recovery or pruning authority.
    pub(crate) fn require_guest_operation_parent_for_mutation(
        &self,
        operation: &GuestOperationRecord,
    ) -> Result<(), StateError> {
        self.validate_operation_parent_binding_for_write(
            &self.guest_operation_path(operation.id),
            operation,
        )
    }

    fn validate_operation_parent_binding(
        &self,
        path: &Path,
        operation: &GuestOperationRecord,
    ) -> Result<(), StateError> {
        match self.load_cell(operation.cell_id) {
            Ok(cell) => validate_operation_job_binding(path, &cell, operation),
            // v0.3 format-1 state permitted an uncorrelated operation whose
            // former cell record was unavailable. Preserve that read
            // compatibility: the record confers no lifecycle authority, and
            // any engine recovery path still requires the cell separately.
            Err(StateError::NotFound(_)) if operation.job_id.is_none() => Ok(()),
            Err(StateError::NotFound(_)) => Err(StateError::JobCorrelationIntegrity {
                path: path.to_path_buf(),
                reason: "guest operation references a missing cell",
            }),
            Err(error) => Err(error),
        }
    }

    /// Managed writes always require their parent cell. This is deliberately
    /// stricter than read compatibility for an old uncorrelated operation:
    /// historical evidence remains inspectable, but cannot be reconciled,
    /// pruned, or rewritten without the exact cell authority.
    fn validate_operation_parent_binding_for_write(
        &self,
        path: &Path,
        operation: &GuestOperationRecord,
    ) -> Result<(), StateError> {
        let cell = self
            .load_cell(operation.cell_id)
            .map_err(|error| match error {
                StateError::NotFound(_) => StateError::JobCorrelationIntegrity {
                    path: path.to_path_buf(),
                    reason: "guest operation references a missing cell",
                },
                error => error,
            })?;
        validate_operation_job_binding(path, &cell, operation)
    }

    pub(crate) fn prepare_artifact_root(
        &self,
        cell_id: CellId,
        operation_id: GuestOperationId,
    ) -> Result<ArtifactGuard, StateError> {
        let state_handle = open_ordinary_directory(&self.root)?;
        let artifacts = self.root.join("artifacts");
        create_direct_child_directory(&artifacts)?;
        let artifacts_handle = open_ordinary_directory(&artifacts)?;
        let cell_root = artifacts.join(cell_id.to_string());
        create_direct_child_directory(&cell_root)?;
        let cell_handle = open_ordinary_directory(&cell_root)?;
        let root = cell_root.join(operation_id.to_string());
        create_direct_child_directory(&root)?;
        let operation_handle = open_ordinary_directory(&root)?;
        let files_root = root.join("files");
        create_direct_child_directory(&files_root)?;
        let files_handle = open_ordinary_directory(&files_root)?;
        ensure_existing_ancestors_are_ordinary(&files_root)?;
        ensure_no_reparse_tree(&root)?;
        Ok(ArtifactGuard {
            cell_id,
            operation_id,
            root,
            files_root,
            state_handle,
            artifacts_handle,
            cell_handle,
            operation_handle,
            files_handle,
        })
    }

    pub(crate) fn write_artifact_file(
        &self,
        guard: &ArtifactGuard,
        index: usize,
        bytes: &[u8],
    ) -> Result<String, StateError> {
        self.validate_artifact_guard(guard)?;
        let name = format!("{index:04}.bin");
        let path = guard.files_root.join(&name);
        write_bytes_new_atomic(&path, bytes)?;
        self.validate_artifact_guard(guard)?;
        Ok(format!(
            "artifacts/{}/{}/files/{name}",
            guard.cell_id, guard.operation_id
        ))
    }

    pub(crate) fn save_artifact_new(
        &self,
        guard: &ArtifactGuard,
        record: &ArtifactRecord,
    ) -> Result<(), StateError> {
        self.validate_artifact_guard(guard)?;
        let path = guard.root.join("manifest.json");
        validate_artifact_schema(&path, record)?;
        if record.id != guard.operation_id || record.cell_id != guard.cell_id {
            return Err(StateError::IdentityMismatch {
                kind: "artifact record",
                path,
                expected: format!("{}/{}", guard.cell_id, guard.operation_id),
            });
        }
        let operation = self.load_guest_operation(record.id)?;
        self.require_guest_operation_parent_for_mutation(&operation)?;
        validate_artifact_job_binding(&path, &operation, record)?;
        let expected_prefix = format!("artifacts/{}/{}/files/", guard.cell_id, guard.operation_id);
        if record.entries.iter().any(|entry| {
            !entry.host_relative_path.starts_with(&expected_prefix)
                || entry.host_relative_path[expected_prefix.len()..].contains('/')
        }) {
            return Err(StateError::UnsafeRuntimePath(path));
        }
        validate_artifact_files(&guard.root, record)?;
        write_json_new(&path, record)?;
        self.validate_artifact_guard(guard)?;
        validate_artifact_files(&guard.root, record)
    }

    pub fn load_artifact(
        &self,
        cell_id: CellId,
        operation_id: GuestOperationId,
    ) -> Result<ArtifactRecord, StateError> {
        let root = self.artifact_root(cell_id, operation_id);
        ensure_existing_ancestors_are_ordinary(&root)?;
        let _root_handle = open_ordinary_directory(&root)?;
        let path = root.join("manifest.json");
        let record = read_json(&path)?;
        validate_artifact_schema(&path, &record)?;
        if record.id != operation_id || record.cell_id != cell_id {
            return Err(StateError::IdentityMismatch {
                kind: "artifact record",
                path,
                expected: format!("{cell_id}/{operation_id}"),
            });
        }
        let operation = self.load_guest_operation(operation_id)?;
        validate_artifact_job_binding(&path, &operation, &record)?;
        validate_artifact_files(&root, &record)?;
        Ok(record)
    }

    pub(crate) fn remove_artifact_root(
        &self,
        mutation: &MutationGuard,
        cell_id: CellId,
        operation_id: GuestOperationId,
    ) -> Result<bool, StateError> {
        mutation.validate_filesystem_identity()?;
        let artifacts_root = self.root.join("artifacts");
        let artifacts_handle = open_ordinary_directory(&artifacts_root)?;
        let cell_root = artifacts_root.join(cell_id.to_string());
        let cell_handle = match open_ordinary_directory(&cell_root) {
            Ok(handle) => handle,
            Err(StateError::Io { source, .. }) if source.kind() == std::io::ErrorKind::NotFound => {
                return Ok(false);
            }
            Err(error) => return Err(error),
        };
        let operation_root = cell_root.join(operation_id.to_string());
        let operation_handle = match open_ordinary_directory(&operation_root) {
            Ok(handle) => handle,
            Err(StateError::Io { source, .. }) if source.kind() == std::io::ErrorKind::NotFound => {
                return Ok(false);
            }
            Err(error) => return Err(error),
        };
        validate_open_path_identity(&self.root, &mutation.state_root.1)?;
        validate_open_path_identity(&artifacts_root, &artifacts_handle)?;
        validate_open_path_identity(&cell_root, &cell_handle)?;
        validate_open_path_identity(&operation_root, &operation_handle)?;
        ensure_existing_ancestors_are_ordinary(&operation_root)?;
        ensure_no_reparse_tree(&operation_root)?;

        let physical_state_root = self
            .root
            .canonicalize()
            .map_err(|source| io_error(&self.root, source))?;
        let physical_artifacts_root = artifacts_root
            .canonicalize()
            .map_err(|source| io_error(&artifacts_root, source))?;
        let physical_cell_root = cell_root
            .canonicalize()
            .map_err(|source| io_error(&cell_root, source))?;
        let physical_operation_root = operation_root
            .canonicalize()
            .map_err(|source| io_error(&operation_root, source))?;
        if physical_artifacts_root.parent() != Some(physical_state_root.as_path())
            || physical_cell_root.parent() != Some(physical_artifacts_root.as_path())
            || physical_operation_root.parent() != Some(physical_cell_root.as_path())
        {
            return Err(StateError::UnsafeRuntimePath(operation_root));
        }

        #[cfg(test)]
        abort_at_test_checkpoint("before_artifact_remove");
        mutation.validate_filesystem_identity()?;
        validate_open_path_identity(&artifacts_root, &artifacts_handle)?;
        validate_open_path_identity(&cell_root, &cell_handle)?;
        validate_open_path_identity(&operation_root, &operation_handle)?;
        #[cfg(windows)]
        drop(operation_handle);
        fs::remove_dir_all(&physical_operation_root)
            .map_err(|source| io_error(&physical_operation_root, source))?;
        #[cfg(not(windows))]
        drop(operation_handle);
        mutation.validate_filesystem_identity()?;
        drop(cell_handle);
        drop(artifacts_handle);
        Ok(true)
    }

    pub(crate) fn artifact_root_exists(
        &self,
        cell_id: CellId,
        operation_id: GuestOperationId,
    ) -> Result<bool, StateError> {
        let root = self.artifact_root(cell_id, operation_id);
        match fs::symlink_metadata(&root) {
            Ok(metadata) if metadata.is_dir() && !is_reparse_point(&root)? => Ok(true),
            Ok(_) => Err(StateError::UnsafeRuntimePath(root)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(source) => Err(io_error(&root, source)),
        }
    }

    #[must_use]
    pub fn cell_runtime_root(&self, cell_id: CellId) -> PathBuf {
        self.root.join("runtime").join(cell_id.0.to_string())
    }

    #[must_use]
    pub fn cell_overlay_path(&self, cell_id: CellId) -> PathBuf {
        self.cell_overlay_path_for(cell_id, "vhdx")
    }

    #[must_use]
    pub fn cell_configuration_path(&self, cell_id: CellId) -> PathBuf {
        self.cell_configuration_path_for(cell_id, "hyperv")
    }

    #[must_use]
    pub fn cell_overlay_path_for(&self, cell_id: CellId, disk_format: &str) -> PathBuf {
        self.cell_runtime_root(cell_id)
            .join(format!("cell.{disk_format}"))
    }

    #[must_use]
    pub fn cell_configuration_path_for(&self, cell_id: CellId, provider: &str) -> PathBuf {
        self.cell_runtime_root(cell_id).join(provider)
    }

    #[cfg(test)]
    pub(crate) fn ensure_cell_runtime(&self, cell_id: CellId) -> Result<PathBuf, StateError> {
        let guard = self.prepare_cell_runtime(cell_id)?;
        Ok(guard.cell_root.clone())
    }

    #[cfg(test)]
    pub(crate) fn prepare_cell_runtime(
        &self,
        cell_id: CellId,
    ) -> Result<CellRuntimeGuard, StateError> {
        self.prepare_cell_runtime_for(
            cell_id,
            self.cell_configuration_path(cell_id),
            self.cell_overlay_path(cell_id),
        )
    }

    pub(crate) fn prepare_cell_runtime_for(
        &self,
        cell_id: CellId,
        configuration_path: PathBuf,
        overlay_path: PathBuf,
    ) -> Result<CellRuntimeGuard, StateError> {
        ensure_directory(&self.root)?;
        let state_handle = open_ordinary_directory(&self.root)?;
        let runtime_root = self.root.join("runtime");
        create_direct_child_directory(&runtime_root)?;
        let runtime_handle = open_ordinary_directory(&runtime_root)?;
        let cell_root = self.cell_runtime_root(cell_id);
        if configuration_path.parent() != Some(cell_root.as_path())
            || overlay_path.parent() != Some(cell_root.as_path())
        {
            return Err(StateError::UnsafeRuntimePath(cell_root));
        }
        create_direct_child_directory(&cell_root)?;
        let cell_handle = open_ordinary_directory(&cell_root)?;
        create_direct_child_directory(&configuration_path)?;
        let configuration_handle = open_ordinary_directory(&configuration_path)?;
        validate_runtime_chain(&self.root, &cell_root)?;
        ensure_no_reparse_tree(&cell_root)?;
        Ok(CellRuntimeGuard {
            cell_id,
            state_root: self.root.clone(),
            runtime_root,
            configuration_path,
            overlay_path,
            cell_root,
            state_handle,
            runtime_handle,
            cell_handle: Some(cell_handle),
            configuration_handle: Some(configuration_handle),
        })
    }

    #[cfg(test)]
    pub(crate) fn pin_cell_runtime(&self, cell_id: CellId) -> Result<CellRuntimeGuard, StateError> {
        self.pin_cell_runtime_for(
            cell_id,
            self.cell_configuration_path(cell_id),
            self.cell_overlay_path(cell_id),
        )
    }

    pub(crate) fn pin_cell_runtime_for(
        &self,
        cell_id: CellId,
        configuration_path: PathBuf,
        overlay_path: PathBuf,
    ) -> Result<CellRuntimeGuard, StateError> {
        let state_handle = open_ordinary_directory(&self.root)?;
        let runtime_root = self.root.join("runtime");
        let runtime_handle = open_ordinary_directory(&runtime_root)?;
        let cell_root = self.cell_runtime_root(cell_id);
        if configuration_path.parent() != Some(cell_root.as_path())
            || overlay_path.parent() != Some(cell_root.as_path())
        {
            return Err(StateError::UnsafeRuntimePath(cell_root));
        }
        let cell_handle = open_ordinary_directory(&cell_root)?;
        let configuration_handle = open_ordinary_directory(&configuration_path)?;
        validate_runtime_chain(&self.root, &cell_root)?;
        ensure_no_reparse_tree(&cell_root)?;
        Ok(CellRuntimeGuard {
            cell_id,
            state_root: self.root.clone(),
            runtime_root,
            configuration_path,
            overlay_path,
            cell_root,
            state_handle,
            runtime_handle,
            cell_handle: Some(cell_handle),
            configuration_handle: Some(configuration_handle),
        })
    }

    pub(crate) fn runtime_entry_exists(&self, cell_id: CellId) -> Result<bool, StateError> {
        let path = self.cell_runtime_root(cell_id);
        match fs::symlink_metadata(&path) {
            Ok(_) => Ok(true),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(source) => Err(io_error(&path, source)),
        }
    }

    pub(crate) fn remove_cell_runtime(
        &self,
        cell_id: CellId,
        mut guard: CellRuntimeGuard,
    ) -> Result<(), StateError> {
        let runtime_root = self.root.join("runtime");
        let cell_root = self.cell_runtime_root(cell_id);
        if guard.cell_id != cell_id
            || guard.state_root != self.root
            || guard.runtime_root != runtime_root
            || guard.cell_root != cell_root
        {
            return Err(StateError::UnsafeRuntimePath(cell_root));
        }
        guard.validate_filesystem_identity()?;

        validate_runtime_chain(&self.root, &cell_root)?;

        let physical_state_root = self
            .root
            .canonicalize()
            .map_err(|source| io_error(&self.root, source))?;
        let physical_runtime_root = runtime_root
            .canonicalize()
            .map_err(|source| io_error(&runtime_root, source))?;
        let physical_cell_root = cell_root
            .canonicalize()
            .map_err(|source| io_error(&cell_root, source))?;

        if physical_runtime_root.parent() != Some(physical_state_root.as_path())
            || physical_cell_root.parent() != Some(physical_runtime_root.as_path())
        {
            return Err(StateError::UnsafeRuntimePath(cell_root));
        }

        ensure_no_reparse_tree(&physical_cell_root)?;
        validate_runtime_chain(&self.root, &cell_root)?;
        guard.validate_filesystem_identity()?;
        #[cfg(windows)]
        {
            drop(guard.configuration_handle.take());
            drop(guard.cell_handle.take());
        }

        // Rust's Windows implementation performs handle-relative recursive
        // removal and does not follow a child swapped to a reparse point. The
        // pinned runtime-root handle prevents an ancestor swap while it runs.
        fs::remove_dir_all(&physical_cell_root)
            .map_err(|source| io_error(&physical_cell_root, source))?;
        #[cfg(not(windows))]
        drop(guard.cell_handle.take());
        Ok(())
    }

    fn image_path(&self, image_id: &ImageId) -> PathBuf {
        self.root
            .join("images")
            .join(format!("{}.json", image_id.as_str()))
    }

    fn cell_path(&self, cell_id: CellId) -> PathBuf {
        self.root.join("cells").join(format!("{}.json", cell_id.0))
    }

    fn guest_operation_path(&self, operation_id: GuestOperationId) -> PathBuf {
        self.root
            .join("operations")
            .join(format!("{}.json", operation_id.0))
    }

    fn artifact_root(&self, cell_id: CellId, operation_id: GuestOperationId) -> PathBuf {
        self.root
            .join("artifacts")
            .join(cell_id.to_string())
            .join(operation_id.to_string())
    }

    fn validate_artifact_guard(&self, guard: &ArtifactGuard) -> Result<(), StateError> {
        let expected_root = self.artifact_root(guard.cell_id, guard.operation_id);
        if guard.root != expected_root || guard.files_root != expected_root.join("files") {
            return Err(StateError::UnsafeRuntimePath(guard.root.clone()));
        }
        validate_open_path_identity(&self.root, &guard.state_handle)?;
        validate_open_path_identity(&self.root.join("artifacts"), &guard.artifacts_handle)?;
        validate_open_path_identity(
            &self.root.join("artifacts").join(guard.cell_id.to_string()),
            &guard.cell_handle,
        )?;
        validate_open_path_identity(&guard.root, &guard.operation_handle)?;
        validate_open_path_identity(&guard.files_root, &guard.files_handle)?;
        ensure_existing_ancestors_are_ordinary(&guard.files_root)?;
        ensure_no_reparse_tree(&guard.root)
    }
}

fn process_mutation_roots() -> &'static Mutex<std::collections::HashSet<PathBuf>> {
    static ROOTS: OnceLock<Mutex<std::collections::HashSet<PathBuf>>> = OnceLock::new();
    ROOTS.get_or_init(|| Mutex::new(std::collections::HashSet::new()))
}

fn release_process_mutation_root(root: &Path) {
    if let Ok(mut roots) = process_mutation_roots().lock() {
        roots.remove(root);
    }
}

fn is_lock_contention(error: &std::io::Error) -> bool {
    if error.kind() == std::io::ErrorKind::WouldBlock {
        return true;
    }
    #[cfg(windows)]
    {
        const ERROR_LOCK_VIOLATION: i32 = 33;
        error.raw_os_error() == Some(ERROR_LOCK_VIOLATION)
    }
    #[cfg(not(windows))]
    {
        false
    }
}

fn redact_cell_diagnostic(record: &mut CellRecord) {
    let is_safe = record.last_error.as_deref().is_none_or(|value| {
        value.len() <= 128
            && value.starts_with("vmcell.")
            && value.bytes().all(|byte| {
                byte.is_ascii_lowercase() || byte.is_ascii_digit() || b"._-".contains(&byte)
            })
    });
    if !is_safe {
        record.last_error = Some(REDACTED_LEGACY_ERROR_CODE.to_owned());
    }
}

fn read_json_directory<T: DeserializeOwned>(
    directory: &Path,
    validate: fn(&Path, &T) -> Result<(), StateError>,
) -> Result<Vec<T>, StateError> {
    match fs::symlink_metadata(directory) {
        Ok(metadata) if metadata.is_dir() && !is_reparse_point(directory)? => {}
        Ok(_) => return Err(StateError::UnsafeRuntimePath(directory.to_path_buf())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(source) => return Err(io_error(directory, source)),
    }
    let _directory_handle = open_ordinary_directory(directory)?;

    let mut paths = fs::read_dir(directory)
        .map_err(|source| io_error(directory, source))?
        .map(|entry| entry.map(|value| value.path()))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|source| io_error(directory, source))?;
    paths.retain(|path| {
        path.extension()
            .is_some_and(|extension| extension == "json")
    });
    paths.sort();
    paths
        .into_iter()
        .map(|path| {
            let value = read_json(&path)?;
            validate(&path, &value)?;
            Ok(value)
        })
        .collect()
}

fn read_json<T: DeserializeOwned>(path: &Path) -> Result<T, StateError> {
    let mut file = open_state_file_read(path)?;
    read_json_from_file(path, &mut file)
}

fn read_json_from_file<T: DeserializeOwned>(path: &Path, file: &mut File) -> Result<T, StateError> {
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)
        .map_err(|source| io_error(path, source))?;
    serde_json::from_slice(&bytes).map_err(|source| StateError::Json {
        path: path.to_path_buf(),
        source,
    })
}

fn open_state_file_read(path: &Path) -> Result<File, StateError> {
    open_state_file(path, false)
}

fn open_state_file_for_authority(path: &Path) -> Result<File, StateError> {
    open_state_file(path, true)
}

fn open_state_file(path: &Path, pin_identity: bool) -> Result<File, StateError> {
    ensure_existing_ancestors_are_ordinary(path)?;
    #[cfg(not(windows))]
    let _ = pin_identity;
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;

        options.custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;

        const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
        const FILE_SHARE_READ: u32 = 0x0000_0001;
        const FILE_SHARE_WRITE: u32 = 0x0000_0002;
        const FILE_SHARE_DELETE: u32 = 0x0000_0004;
        options.custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
        options.share_mode(if pin_identity {
            FILE_SHARE_READ
        } else {
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE
        });
    }
    let file = match options.open(path) {
        Ok(file) => file,
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => {
            return Err(StateError::NotFound(path.to_path_buf()));
        }
        Err(source) => return Err(io_error(path, source)),
    };
    if file_metadata_is_reparse(&file).map_err(|source| io_error(path, source))? {
        return Err(StateError::UnsafeRuntimePath(path.to_path_buf()));
    }
    ensure_private_open_file(path, &file)?;
    ensure_existing_ancestors_are_ordinary(path)?;
    validate_open_path_identity(path, &file)?;
    Ok(file)
}

fn write_json_new<T: Serialize>(path: &Path, value: &T) -> Result<(), StateError> {
    if path.exists() {
        return Err(StateError::AlreadyExists(path.to_path_buf()));
    }
    write_json_atomic(path, value)
}

fn write_json_atomic<T: Serialize>(path: &Path, value: &T) -> Result<(), StateError> {
    let parent = path
        .parent()
        .ok_or_else(|| StateError::UnsafeRuntimePath(path.to_path_buf()))?;
    ensure_directory(parent)?;

    let mut bytes = serde_json::to_vec_pretty(value).map_err(|source| StateError::Json {
        path: path.to_path_buf(),
        source,
    })?;
    bytes.push(b'\n');

    let temporary = parent.join(format!(
        ".{}.{}.tmp",
        path.file_name()
            .and_then(|value| value.to_str())
            .unwrap_or("state"),
        Uuid::new_v4()
    ));

    let result = (|| {
        let mut options = OpenOptions::new();
        options.create_new(true).write(true);
        configure_private_created_file(&mut options);
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;

            const FILE_FLAG_WRITE_THROUGH: u32 = 0x8000_0000;
            options.custom_flags(FILE_FLAG_WRITE_THROUGH);
        }
        let mut file = options
            .open(&temporary)
            .map_err(|source| io_error(&temporary, source))?;
        file.write_all(&bytes)
            .map_err(|source| io_error(&temporary, source))?;
        file.sync_all()
            .map_err(|source| io_error(&temporary, source))?;
        drop(file);
        #[cfg(test)]
        abort_at_test_checkpoint("before_manifest_rename");
        fs::rename(&temporary, path).map_err(|source| io_error(path, source))?;
        #[cfg(test)]
        abort_at_test_checkpoint("after_manifest_rename");
        #[cfg(not(windows))]
        {
            let committed = open_state_file_read(path)?;
            committed
                .sync_all()
                .map_err(|source| io_error(path, source))?;
            let directory = File::open(parent).map_err(|source| io_error(parent, source))?;
            directory
                .sync_all()
                .map_err(|source| io_error(parent, source))?;
        }
        Ok(())
    })();

    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

fn write_bytes_new_atomic(path: &Path, bytes: &[u8]) -> Result<(), StateError> {
    if path.exists() {
        return Err(StateError::AlreadyExists(path.to_path_buf()));
    }
    let parent = path
        .parent()
        .ok_or_else(|| StateError::UnsafeRuntimePath(path.to_path_buf()))?;
    ensure_existing_ancestors_are_ordinary(parent)?;
    let temporary = parent.join(format!(
        ".{}.{}.tmp",
        path.file_name()
            .and_then(|value| value.to_str())
            .unwrap_or("artifact"),
        Uuid::new_v4()
    ));
    let result = (|| {
        let mut options = OpenOptions::new();
        options.create_new(true).write(true);
        configure_private_created_file(&mut options);
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;

            const FILE_FLAG_WRITE_THROUGH: u32 = 0x8000_0000;
            options.custom_flags(FILE_FLAG_WRITE_THROUGH);
        }
        let mut file = options
            .open(&temporary)
            .map_err(|source| io_error(&temporary, source))?;
        file.write_all(bytes)
            .map_err(|source| io_error(&temporary, source))?;
        file.sync_all()
            .map_err(|source| io_error(&temporary, source))?;
        drop(file);
        #[cfg(test)]
        abort_at_test_checkpoint("before_artifact_rename");
        fs::rename(&temporary, path).map_err(|source| io_error(path, source))?;
        #[cfg(test)]
        abort_at_test_checkpoint("after_artifact_rename");
        ensure_existing_ancestors_are_ordinary(path)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

#[cfg(test)]
fn abort_at_test_checkpoint(checkpoint: &str) {
    let is_child =
        std::env::var_os("VMCELL_TEST_ATOMIC_CRASH_CHILD").is_some_and(|value| value == "1");
    let selected =
        std::env::var_os("VMCELL_TEST_ABORT_AT").is_some_and(|value| value == checkpoint);
    if is_child && selected {
        std::process::abort();
    }
}

fn ensure_directory(path: &Path) -> Result<(), StateError> {
    ensure_existing_ancestors_are_ordinary(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;

        let mut builder = fs::DirBuilder::new();
        builder.recursive(true).mode(0o700);
        builder
            .create(path)
            .map_err(|source| io_error(path, source))?;
    }
    #[cfg(not(unix))]
    fs::create_dir_all(path).map_err(|source| io_error(path, source))?;
    ensure_existing_ancestors_are_ordinary(path)?;
    ensure_private_directory(path)
}

fn create_direct_child_directory(path: &Path) -> Result<(), StateError> {
    #[cfg(unix)]
    let result = {
        use std::os::unix::fs::DirBuilderExt;

        fs::DirBuilder::new().mode(0o700).create(path)
    };
    #[cfg(not(unix))]
    let result = fs::create_dir(path);
    match result {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            if is_reparse_point(path)?
                || !fs::symlink_metadata(path)
                    .map_err(|source| io_error(path, source))?
                    .is_dir()
            {
                Err(StateError::UnsafeRuntimePath(path.to_path_buf()))
            } else {
                Ok(())
            }
        }
        Err(source) => Err(io_error(path, source)),
    }?;
    ensure_private_directory(path)
}

fn open_ordinary_directory(path: &Path) -> Result<File, StateError> {
    ensure_existing_ancestors_are_ordinary(path)?;
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;

        options.custom_flags(libc::O_CLOEXEC | libc::O_DIRECTORY | libc::O_NOFOLLOW);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;

        const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
        const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
        const FILE_SHARE_READ: u32 = 0x0000_0001;
        const FILE_SHARE_WRITE: u32 = 0x0000_0002;
        options.custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT);
        // Deliberately deny FILE_SHARE_DELETE while the guard is live so an
        // ancestor cannot be renamed/replaced between proof and provider use.
        options.share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE);
    }
    let file = options
        .open(path)
        .map_err(|source| io_error(path, source))?;
    let metadata = file.metadata().map_err(|source| io_error(path, source))?;
    if !metadata.is_dir()
        || file_metadata_is_reparse(&file).map_err(|source| io_error(path, source))?
    {
        return Err(StateError::UnsafeRuntimePath(path.to_path_buf()));
    }
    ensure_private_directory(path)?;
    ensure_existing_ancestors_are_ordinary(path)?;
    validate_open_path_identity(path, &file)?;
    Ok(file)
}

fn configure_private_created_file(options: &mut OpenOptions) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;

        options
            .mode(0o600)
            .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW);
    }
    #[cfg(not(unix))]
    let _ = options;
}

fn open_mutation_lock_file(path: &Path) -> Result<File, StateError> {
    let mut options = OpenOptions::new();
    options.create(true).read(true).write(true).truncate(false);
    configure_private_created_file(&mut options);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;

        const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
        const FILE_SHARE_READ: u32 = 0x0000_0001;
        const FILE_SHARE_WRITE: u32 = 0x0000_0002;
        options.custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
        options.share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE);
    }
    let file = options
        .open(path)
        .map_err(|source| io_error(path, source))?;
    let metadata = file.metadata().map_err(|source| io_error(path, source))?;
    if !metadata.is_file()
        || file_metadata_is_reparse(&file).map_err(|source| io_error(path, source))?
    {
        return Err(StateError::UnsafeRuntimePath(path.to_path_buf()));
    }
    ensure_private_open_file(path, &file)?;
    validate_open_path_identity(path, &file)?;
    Ok(file)
}

#[cfg(unix)]
fn ensure_private_directory(path: &Path) -> Result<(), StateError> {
    use std::os::unix::fs::MetadataExt;

    let metadata = fs::symlink_metadata(path).map_err(|source| io_error(path, source))?;
    if metadata.uid() != unsafe { libc::geteuid() } || metadata.mode() & 0o077 != 0 {
        return Err(StateError::UnsafeRuntimePath(path.to_path_buf()));
    }
    Ok(())
}

#[cfg(not(unix))]
fn ensure_private_directory(_path: &Path) -> Result<(), StateError> {
    Ok(())
}

#[cfg(unix)]
fn ensure_private_open_file(path: &Path, file: &File) -> Result<(), StateError> {
    use std::os::unix::fs::MetadataExt;

    let metadata = file.metadata().map_err(|source| io_error(path, source))?;
    if metadata.uid() != unsafe { libc::geteuid() } || metadata.mode() & 0o077 != 0 {
        return Err(StateError::UnsafeRuntimePath(path.to_path_buf()));
    }
    Ok(())
}

#[cfg(not(unix))]
fn ensure_private_open_file(_path: &Path, _file: &File) -> Result<(), StateError> {
    Ok(())
}

#[cfg(unix)]
fn validate_open_path_identity(path: &Path, file: &File) -> Result<(), StateError> {
    use std::os::unix::fs::MetadataExt;

    let open = file.metadata().map_err(|source| io_error(path, source))?;
    let current = fs::symlink_metadata(path).map_err(|source| io_error(path, source))?;
    if open.dev() != current.dev()
        || open.ino() != current.ino()
        || current.file_type().is_symlink()
    {
        return Err(StateError::UnsafeRuntimePath(path.to_path_buf()));
    }
    Ok(())
}

#[cfg(not(unix))]
fn validate_open_path_identity(path: &Path, _file: &File) -> Result<(), StateError> {
    ensure_existing_ancestors_are_ordinary(path)
}

#[cfg(windows)]
fn file_metadata_is_reparse(file: &File) -> std::io::Result<bool> {
    use std::os::windows::fs::MetadataExt;

    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
    Ok(file.metadata()?.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0)
}

#[cfg(not(windows))]
fn file_metadata_is_reparse(file: &File) -> std::io::Result<bool> {
    Ok(file.metadata()?.file_type().is_symlink())
}

fn validate_image_schema(path: &Path, record: &ImageRecord) -> Result<(), StateError> {
    ensure_schema(
        path,
        "image record",
        record.schema_version,
        IMAGE_SCHEMA_VERSION,
    )?;
    let expected = path.file_stem().and_then(|value| value.to_str());
    if expected != Some(record.id.as_str()) {
        return Err(StateError::IdentityMismatch {
            kind: "image record",
            path: path.to_path_buf(),
            expected: expected.unwrap_or("<non-utf8>").to_owned(),
        });
    }
    Ok(())
}

fn validate_image_record_for_metadata_removal(
    manifest_path: &Path,
    record: &ImageRecord,
    manifest_file: &File,
) -> Result<(), StateError> {
    if record.variants.is_empty() {
        return Err(StateError::UnsafeRuntimePath(manifest_path.to_path_buf()));
    }

    for variant in &record.variants {
        let expected_format = match variant.provider.as_str() {
            "hyperv" => "vhdx",
            "qemu" => "qcow2",
            _ => return Err(StateError::UnsafeRuntimePath(manifest_path.to_path_buf())),
        };
        let extension_matches = variant
            .path
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| extension.eq_ignore_ascii_case(expected_format));
        if !variant.path.is_absolute()
            || !variant.disk_format.eq_ignore_ascii_case(expected_format)
            || !extension_matches
            || removable_paths_equal(&variant.path, manifest_path)
            || windows_path_has_stream_or_device_ambiguity(&variant.path)
        {
            return Err(StateError::UnsafeRuntimePath(manifest_path.to_path_buf()));
        }
        validate_variant_file_identity(manifest_path, manifest_file, &variant.path)?;
    }
    Ok(())
}

fn validate_variant_file_identity(
    manifest_path: &Path,
    manifest_file: &File,
    variant_path: &Path,
) -> Result<(), StateError> {
    ensure_existing_ancestors_are_ordinary(variant_path)?;
    let metadata = match fs::symlink_metadata(variant_path) {
        Ok(metadata) => metadata,
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(source) => return Err(io_error(variant_path, source)),
    };
    if !metadata.is_file() || is_reparse_point(variant_path)? {
        return Err(StateError::UnsafeRuntimePath(manifest_path.to_path_buf()));
    }

    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;

        options.custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;

        const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
        const FILE_SHARE_READ: u32 = 0x0000_0001;
        const FILE_SHARE_WRITE: u32 = 0x0000_0002;
        const FILE_SHARE_DELETE: u32 = 0x0000_0004;
        options.custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
        options.share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE);
    }
    let variant_file = options
        .open(variant_path)
        .map_err(|source| io_error(variant_path, source))?;
    if file_metadata_is_reparse(&variant_file).map_err(|source| io_error(variant_path, source))?
        || file_identity_equal(manifest_path, manifest_file, variant_path, &variant_file)?
    {
        return Err(StateError::UnsafeRuntimePath(manifest_path.to_path_buf()));
    }
    Ok(())
}

#[cfg(unix)]
fn file_identity_equal(
    manifest_path: &Path,
    manifest_file: &File,
    variant_path: &Path,
    variant_file: &File,
) -> Result<bool, StateError> {
    use std::os::unix::fs::MetadataExt;

    let manifest = manifest_file
        .metadata()
        .map_err(|source| io_error(manifest_path, source))?;
    let variant = variant_file
        .metadata()
        .map_err(|source| io_error(variant_path, source))?;
    Ok(manifest.dev() == variant.dev() && manifest.ino() == variant.ino())
}

#[cfg(windows)]
fn file_identity_equal(
    manifest_path: &Path,
    manifest_file: &File,
    variant_path: &Path,
    variant_file: &File,
) -> Result<bool, StateError> {
    fn identity(path: &Path, file: &File) -> Result<(u32, u64), StateError> {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::Storage::FileSystem::{
            BY_HANDLE_FILE_INFORMATION, GetFileInformationByHandle,
        };

        let mut information = BY_HANDLE_FILE_INFORMATION::default();
        if unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut information) } == 0 {
            return Err(io_error(path, std::io::Error::last_os_error()));
        }
        let index =
            (u64::from(information.nFileIndexHigh) << 32) | u64::from(information.nFileIndexLow);
        Ok((information.dwVolumeSerialNumber, index))
    }

    Ok(identity(manifest_path, manifest_file)? == identity(variant_path, variant_file)?)
}

fn lexically_normalized_absolute_path(path: &Path) -> Option<PathBuf> {
    use std::path::Component;

    if !path.is_absolute() {
        return None;
    }
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Prefix(prefix) => normalized.push(prefix.as_os_str()),
            Component::RootDir => normalized.push(component.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => {
                if !normalized.pop() {
                    return None;
                }
            }
            Component::Normal(value) => normalized.push(value),
        }
    }
    Some(normalized)
}

#[cfg(windows)]
fn removable_paths_equal(left: &Path, right: &Path) -> bool {
    fn identity(path: &Path) -> Option<String> {
        let normalized = lexically_normalized_absolute_path(path)?;
        let value = normalized.to_string_lossy().replace('/', "\\");
        let value = if value
            .get(..8)
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case(r"\\?\UNC\"))
        {
            format!(r"\\{}", &value[8..])
        } else if value
            .get(..4)
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case(r"\\?\"))
        {
            value[4..].to_owned()
        } else {
            value
        };
        Some(value.to_ascii_lowercase())
    }

    identity(left).is_some_and(|left| identity(right).is_some_and(|right| left == right))
}

#[cfg(not(windows))]
fn removable_paths_equal(left: &Path, right: &Path) -> bool {
    lexically_normalized_absolute_path(left)
        .zip(lexically_normalized_absolute_path(right))
        .is_some_and(|(left, right)| left == right)
}

#[cfg(windows)]
fn windows_path_has_stream_or_device_ambiguity(path: &Path) -> bool {
    let value = path.to_string_lossy().replace('/', "\\");
    if value
        .get(..4)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case(r"\\.\"))
        || value
            .get(..15)
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case(r"\\?\GLOBALROOT\"))
    {
        return true;
    }
    let value = value.strip_prefix(r"\\?\").unwrap_or(&value);
    if value.starts_with(r"UNC\") {
        return value.contains(':');
    }
    let without_drive = value
        .as_bytes()
        .get(1)
        .filter(|byte| **byte == b':')
        .map_or(value, |_| &value[2..]);
    if without_drive.contains(':') {
        return true;
    }
    let Some(file_name) = Path::new(value).file_name().and_then(|name| name.to_str()) else {
        return true;
    };
    let device_stem = file_name
        .split('.')
        .next()
        .unwrap_or(file_name)
        .trim_end_matches([' ', '.'])
        .to_ascii_uppercase();
    matches!(
        device_stem.as_str(),
        "CON"
            | "PRN"
            | "AUX"
            | "NUL"
            | "COM1"
            | "COM2"
            | "COM3"
            | "COM4"
            | "COM5"
            | "COM6"
            | "COM7"
            | "COM8"
            | "COM9"
            | "LPT1"
            | "LPT2"
            | "LPT3"
            | "LPT4"
            | "LPT5"
            | "LPT6"
            | "LPT7"
            | "LPT8"
            | "LPT9"
            | "COM¹"
            | "COM²"
            | "COM³"
            | "LPT¹"
            | "LPT²"
            | "LPT³"
    )
}

#[cfg(not(windows))]
fn windows_path_has_stream_or_device_ambiguity(_path: &Path) -> bool {
    false
}

fn validate_cell_schema(path: &Path, record: &CellRecord) -> Result<(), StateError> {
    ensure_schema_one_of(
        path,
        "cell record",
        record.schema_version,
        &[CELL_SCHEMA_VERSION, JOB_CORRELATED_CELL_SCHEMA_VERSION],
        MAX_CELL_SCHEMA_VERSION,
    )?;
    ensure_schema(
        path,
        "cell ownership",
        record.ownership.schema_version,
        OWNERSHIP_MARKER_SCHEMA,
    )?;
    let expected = path.file_stem().and_then(|value| value.to_str());
    let record_id = record.id.to_string();
    if expected != Some(record_id.as_str()) {
        return Err(StateError::IdentityMismatch {
            kind: "cell record",
            path: path.to_path_buf(),
            expected: expected.unwrap_or("<non-utf8>").to_owned(),
        });
    }
    match (record.schema_version, &record.job) {
        (CELL_SCHEMA_VERSION, None) => {}
        (CELL_SCHEMA_VERSION, Some(_)) => {
            return Err(StateError::JobCorrelationIntegrity {
                path: path.to_path_buf(),
                reason: "cell schema v1 must not carry job correlation",
            });
        }
        (JOB_CORRELATED_CELL_SCHEMA_VERSION, Some(job)) => validate_job_correlation(path, job)?,
        (JOB_CORRELATED_CELL_SCHEMA_VERSION, None) => {
            return Err(StateError::JobCorrelationIntegrity {
                path: path.to_path_buf(),
                reason: "cell schema v2 requires job correlation",
            });
        }
        _ => unreachable!("schema version was checked above"),
    }
    Ok(())
}

fn validate_job_correlation(path: &Path, correlation: &JobCorrelation) -> Result<(), StateError> {
    if correlation.job_id.0.is_nil() {
        return Err(StateError::JobCorrelationIntegrity {
            path: path.to_path_buf(),
            reason: "job id must not be nil",
        });
    }
    if !correlation.has_valid_spec_digest() {
        return Err(StateError::JobCorrelationIntegrity {
            path: path.to_path_buf(),
            reason: "job specification digest is not canonical SHA-256",
        });
    }
    Ok(())
}

fn validate_unique_cell_job_ids(root: &Path, cells: &[CellRecord]) -> Result<(), StateError> {
    validate_unique_cell_job_ids_for_path(&root.join("cells"), cells)
}

fn validate_unique_cell_job_ids_for_path(
    path: &Path,
    cells: &[CellRecord],
) -> Result<(), StateError> {
    let mut seen = HashMap::new();
    for cell in cells {
        let Some(job) = &cell.job else {
            continue;
        };
        if let Some(other) = seen.insert(job.job_id, cell.id) {
            return Err(StateError::JobCorrelationIntegrity {
                path: path.to_path_buf(),
                reason: if other == cell.id {
                    "cell job correlation is not unique"
                } else {
                    "job id is already bound to a different cell"
                },
            });
        }
    }
    Ok(())
}

fn validate_operation_job_binding(
    path: &Path,
    cell: &CellRecord,
    operation: &GuestOperationRecord,
) -> Result<(), StateError> {
    if let Some(job_id) = operation.job_id {
        if cell.job.as_ref().map(|job| job.job_id) != Some(job_id) {
            return Err(StateError::JobCorrelationIntegrity {
                path: path.to_path_buf(),
                reason: "guest operation job id does not match its cell",
            });
        }
    }
    Ok(())
}

fn validate_job_operation_bindings(
    root: &Path,
    cells: &[CellRecord],
    operations: &[GuestOperationRecord],
) -> Result<(), StateError> {
    let cells_by_id = cells
        .iter()
        .map(|cell| (cell.id, cell))
        .collect::<HashMap<_, _>>();
    for operation in operations {
        let path = root
            .join("operations")
            .join(format!("{}.json", operation.id));
        match cells_by_id.get(&operation.cell_id) {
            Some(cell) => validate_operation_job_binding(&path, cell, operation)?,
            // Legacy/direct operations without a job correlation remain
            // readable under durable format 1 even if their old cell record
            // is no longer present. They are observational only; no engine
            // action may use them as a substitute for cell authority.
            None if operation.job_id.is_none() => {}
            None => {
                return Err(StateError::JobCorrelationIntegrity {
                    path,
                    reason: "guest operation references a missing cell",
                });
            }
        }
    }
    Ok(())
}

fn validate_artifact_job_binding(
    path: &Path,
    operation: &GuestOperationRecord,
    artifact: &ArtifactRecord,
) -> Result<(), StateError> {
    if artifact.cell_id != operation.cell_id {
        return Err(StateError::JobCorrelationIntegrity {
            path: path.to_path_buf(),
            reason: "artifact cell id does not match its guest operation",
        });
    }
    if artifact.job_id != operation.job_id {
        return Err(StateError::JobCorrelationIntegrity {
            path: path.to_path_buf(),
            reason: "artifact job id does not match its guest operation",
        });
    }
    Ok(())
}

fn validate_guest_operation_schema(
    path: &Path,
    record: &GuestOperationRecord,
) -> Result<(), StateError> {
    ensure_schema_one_of(
        path,
        "guest operation record",
        record.schema_version,
        &[
            GUEST_OPERATION_SCHEMA_VERSION,
            JOB_CORRELATED_GUEST_OPERATION_SCHEMA_VERSION,
        ],
        MAX_GUEST_OPERATION_SCHEMA_VERSION,
    )?;
    let expected = path.file_stem().and_then(|value| value.to_str());
    let record_id = record.id.to_string();
    if expected != Some(record_id.as_str()) {
        return Err(StateError::IdentityMismatch {
            kind: "guest operation record",
            path: path.to_path_buf(),
            expected: expected.unwrap_or("<non-utf8>").to_owned(),
        });
    }
    match (record.schema_version, record.job_id) {
        (GUEST_OPERATION_SCHEMA_VERSION, None)
        | (JOB_CORRELATED_GUEST_OPERATION_SCHEMA_VERSION, Some(_)) => {}
        (GUEST_OPERATION_SCHEMA_VERSION, Some(_)) => {
            return Err(StateError::JobCorrelationIntegrity {
                path: path.to_path_buf(),
                reason: "guest operation schema v1 must not carry job correlation",
            });
        }
        (JOB_CORRELATED_GUEST_OPERATION_SCHEMA_VERSION, None) => {
            return Err(StateError::JobCorrelationIntegrity {
                path: path.to_path_buf(),
                reason: "guest operation schema v2 requires job correlation",
            });
        }
        _ => unreachable!("schema version was checked above"),
    }
    let fields_are_valid = match record.phase {
        crate::core::guest::GuestOperationPhase::IntentRecorded => {
            record.completed_at.is_none()
                && record.failure.is_none()
                && record.artifact_id.is_none()
                && record.artifact_pruned_at.is_none()
                && record.exit_code.is_none()
                && record.stdout_bytes.is_none()
                && record.stderr_bytes.is_none()
        }
        crate::core::guest::GuestOperationPhase::TransportActive => {
            record.completed_at.is_none()
                && record.artifact_id.is_none()
                && record.artifact_pruned_at.is_none()
                && record.exit_code.is_none()
                && record.stdout_bytes.is_none()
                && record.stderr_bytes.is_none()
        }
        crate::core::guest::GuestOperationPhase::ArtifactCommitted => {
            record.completed_at.is_none()
                && record.failure.is_none()
                && record.artifact_id == Some(record.id)
                && record.artifact_pruned_at.is_none()
                && record.exit_code.is_none()
                && record.stdout_bytes.is_none()
                && record.stderr_bytes.is_none()
        }
        crate::core::guest::GuestOperationPhase::Completed => {
            record.completed_at.is_some()
                && record.failure.is_none()
                && record
                    .artifact_id
                    .is_none_or(|artifact_id| artifact_id == record.id)
                && record.artifact_id.is_none_or(|_| {
                    matches!(
                        record.kind,
                        crate::core::guest::GuestOperationKind::CopyOut
                            | crate::core::guest::GuestOperationKind::ArtifactCollect
                    )
                })
                && record.artifact_pruned_at.is_none_or(|_| {
                    record.artifact_id == Some(record.id)
                        && matches!(
                            record.kind,
                            crate::core::guest::GuestOperationKind::CopyOut
                                | crate::core::guest::GuestOperationKind::ArtifactCollect
                        )
                        && record.exit_code.is_none()
                        && record.stdout_bytes.is_none()
                        && record.stderr_bytes.is_none()
                })
        }
        crate::core::guest::GuestOperationPhase::Failed => {
            record.completed_at.is_some()
                && record.failure.is_some_and(|failure| {
                    failure != crate::core::guest::GuestFailureClass::Unknown
                })
                && record.artifact_id.is_none()
                && record.artifact_pruned_at.is_none()
                && record.exit_code.is_none()
                && record.stdout_bytes.is_none()
                && record.stderr_bytes.is_none()
        }
    };
    if !fields_are_valid
        || record.updated_at < record.created_at
        || record
            .completed_at
            .is_some_and(|completed_at| completed_at < record.created_at)
        || match record.artifact_pruned_at {
            Some(pruned_at) => {
                record
                    .completed_at
                    .is_none_or(|completed_at| completed_at > pruned_at)
                    || pruned_at != record.updated_at
            }
            None => record
                .completed_at
                .is_some_and(|completed_at| completed_at != record.updated_at),
        }
    {
        return Err(StateError::GuestOperationIntegrity {
            path: path.to_path_buf(),
            reason: "phase and durable fields are inconsistent",
        });
    }
    Ok(())
}

fn validate_artifact_schema(path: &Path, record: &ArtifactRecord) -> Result<(), StateError> {
    ensure_schema_one_of(
        path,
        "artifact record",
        record.schema_version,
        &[
            ARTIFACT_SCHEMA_VERSION,
            JOB_CORRELATED_ARTIFACT_SCHEMA_VERSION,
        ],
        MAX_ARTIFACT_SCHEMA_VERSION,
    )?;
    match (record.schema_version, record.job_id) {
        (ARTIFACT_SCHEMA_VERSION, None) | (JOB_CORRELATED_ARTIFACT_SCHEMA_VERSION, Some(_)) => {}
        (ARTIFACT_SCHEMA_VERSION, Some(_)) => {
            return Err(StateError::JobCorrelationIntegrity {
                path: path.to_path_buf(),
                reason: "artifact schema v1 must not carry job correlation",
            });
        }
        (JOB_CORRELATED_ARTIFACT_SCHEMA_VERSION, None) => {
            return Err(StateError::JobCorrelationIntegrity {
                path: path.to_path_buf(),
                reason: "artifact schema v2 requires job correlation",
            });
        }
        _ => unreachable!("schema version was checked above"),
    }
    if record.entries.is_empty() || record.entries.len() > MAX_ARTIFACT_FILES {
        return Err(StateError::ArtifactIntegrity {
            path: path.to_path_buf(),
            reason: "artifact file count is outside the bounded policy",
        });
    }
    if record
        .entries
        .iter()
        .any(|entry| entry.size > MAX_ARTIFACT_FILE_BYTES)
    {
        return Err(StateError::ArtifactIntegrity {
            path: path.to_path_buf(),
            reason: "artifact file size exceeds the bounded policy",
        });
    }
    let total = record.entries.iter().try_fold(0_u64, |total, entry| {
        total.checked_add(entry.size).ok_or(())
    });
    if total.is_err() || total.is_ok_and(|total| total > MAX_ARTIFACT_TOTAL_BYTES) {
        return Err(StateError::ArtifactIntegrity {
            path: path.to_path_buf(),
            reason: "artifact total size exceeds the bounded policy",
        });
    }
    Ok(())
}

fn validate_artifact_files(root: &Path, record: &ArtifactRecord) -> Result<(), StateError> {
    let files_root = root.join("files");
    let _files_handle = open_ordinary_directory(&files_root)?;
    ensure_no_reparse_tree(root)?;
    let expected_names = (0..record.entries.len())
        .map(|index| format!("{index:04}.bin"))
        .collect::<std::collections::BTreeSet<_>>();
    let actual_names = fs::read_dir(&files_root)
        .map_err(|source| io_error(&files_root, source))?
        .map(|entry| {
            entry
                .map_err(|source| io_error(&files_root, source))
                .and_then(|entry| {
                    entry
                        .file_name()
                        .into_string()
                        .map_err(|_| StateError::ArtifactIntegrity {
                            path: entry.path(),
                            reason: "artifact filename is not UTF-8",
                        })
                })
        })
        .collect::<Result<std::collections::BTreeSet<_>, _>>()?;
    if actual_names != expected_names {
        return Err(StateError::ArtifactIntegrity {
            path: files_root,
            reason: "artifact file set does not exactly match manifest",
        });
    }
    for (index, entry) in record.entries.iter().enumerate() {
        let file_name = format!("{index:04}.bin");
        let expected_relative = format!(
            "artifacts/{}/{}/files/{file_name}",
            record.cell_id, record.id
        );
        if entry.host_relative_path != expected_relative {
            return Err(StateError::ArtifactIntegrity {
                path: root.join("manifest.json"),
                reason: "manifest path is not operation-bound",
            });
        }
        let path = files_root.join(file_name);
        let mut file = open_state_file_for_authority(&path)?;
        let metadata = file.metadata().map_err(|source| io_error(&path, source))?;
        if metadata.len() != entry.size {
            return Err(StateError::ArtifactIntegrity {
                path,
                reason: "file size does not match manifest",
            });
        }
        let mut hasher = Sha256::new();
        let mut buffer = [0_u8; 64 * 1024];
        loop {
            let read = file
                .read(&mut buffer)
                .map_err(|source| io_error(&path, source))?;
            if read == 0 {
                break;
            }
            hasher.update(&buffer[..read]);
        }
        if format!("{:x}", hasher.finalize()) != entry.sha256 {
            return Err(StateError::ArtifactIntegrity {
                path,
                reason: "file hash does not match manifest",
            });
        }
    }
    ensure_no_reparse_tree(root)
}

fn ensure_schema(
    path: &Path,
    kind: &'static str,
    actual: u32,
    expected: u32,
) -> Result<(), StateError> {
    if actual == expected {
        Ok(())
    } else {
        Err(StateError::UnsupportedSchema {
            kind,
            path: path.to_path_buf(),
            expected,
            actual,
        })
    }
}

fn ensure_schema_one_of(
    path: &Path,
    kind: &'static str,
    actual: u32,
    supported: &[u32],
    highest_supported: u32,
) -> Result<(), StateError> {
    if supported.contains(&actual) {
        Ok(())
    } else {
        Err(StateError::UnsupportedSchema {
            kind,
            path: path.to_path_buf(),
            expected: highest_supported,
            actual,
        })
    }
}

fn as_upgrade_required(error: StateError) -> StateError {
    match error {
        StateError::UnsupportedSchema {
            kind,
            expected,
            actual,
            ..
        } => StateError::UpgradeRequired {
            kind,
            supported: expected,
            actual,
        },
        error => error,
    }
}

fn validate_runtime_chain(state_root: &Path, cell_root: &Path) -> Result<(), StateError> {
    let runtime_root = state_root.join("runtime");
    if cell_root.parent() != Some(runtime_root.as_path()) {
        return Err(StateError::UnsafeRuntimePath(cell_root.to_path_buf()));
    }
    ensure_existing_ancestors_are_ordinary(cell_root)
}

fn ensure_existing_ancestors_are_ordinary(path: &Path) -> Result<(), StateError> {
    let mut ancestors: Vec<&Path> = path.ancestors().collect();
    ancestors.reverse();
    for ancestor in ancestors {
        if ancestor.as_os_str().is_empty() || !ancestor.exists() {
            continue;
        }
        if is_reparse_point(ancestor)? {
            return Err(StateError::UnsafeRuntimePath(ancestor.to_path_buf()));
        }
    }
    Ok(())
}

fn ensure_no_reparse_tree(path: &Path) -> Result<(), StateError> {
    for entry in fs::read_dir(path).map_err(|source| io_error(path, source))? {
        let entry = entry.map_err(|source| io_error(path, source))?;
        let entry_path = entry.path();
        if is_reparse_point(&entry_path)? {
            return Err(StateError::UnsafeRuntimePath(entry_path));
        }
        if entry
            .file_type()
            .map_err(|source| io_error(&entry_path, source))?
            .is_dir()
        {
            ensure_no_reparse_tree(&entry_path)?;
        }
    }
    Ok(())
}

fn io_error(path: &Path, source: std::io::Error) -> StateError {
    StateError::Io {
        path: path.to_path_buf(),
        source,
    }
}

#[cfg(windows)]
fn is_reparse_point(path: &Path) -> Result<bool, StateError> {
    use std::os::windows::fs::MetadataExt;

    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
    let metadata = fs::symlink_metadata(path).map_err(|source| io_error(path, source))?;
    Ok(metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0)
}

#[cfg(not(windows))]
fn is_reparse_point(path: &Path) -> Result<bool, StateError> {
    let metadata = fs::symlink_metadata(path).map_err(|source| io_error(path, source))?;
    Ok(metadata.file_type().is_symlink())
}

#[cfg(test)]
mod tests {
    use std::process::{Command, Stdio};
    use std::str::FromStr;

    use chrono::Utc;
    use tempfile::tempdir;

    use super::*;
    use crate::core::cell::{CellPhase, CellSpec, CellState};
    use crate::core::guest::{
        ArtifactEntry, GuestFailureClass, GuestOperationKind, GuestOperationPhase,
    };
    use crate::core::image::{Architecture, GuestOs, ImageBinding, ImageVariant};
    use crate::core::ownership::CellOwnership;

    #[cfg(unix)]
    #[test]
    fn unix_state_root_and_records_are_private_and_identity_pinned() {
        use std::os::unix::fs::PermissionsExt;

        let directory = tempdir().unwrap();
        let store = StateStore::new(directory.path().join("state"));
        let guard = store.acquire_mutation_lock().unwrap();
        store.installation().unwrap();
        assert_eq!(
            fs::metadata(store.root()).unwrap().permissions().mode() & 0o077,
            0
        );
        assert_eq!(
            fs::metadata(store.root().join("installation.json"))
                .unwrap()
                .permissions()
                .mode()
                & 0o077,
            0
        );
        assert!(guard.validate_filesystem_identity().is_ok());
        drop(guard);

        let lock_path = store.root().join("locks").join("mutation.lock");
        fs::set_permissions(&lock_path, fs::Permissions::from_mode(0o644)).unwrap();
        assert!(matches!(
            store.acquire_mutation_lock(),
            Err(StateError::UnsafeRuntimePath(_))
        ));
        fs::set_permissions(&lock_path, fs::Permissions::from_mode(0o600)).unwrap();

        fs::set_permissions(store.root(), fs::Permissions::from_mode(0o755)).unwrap();
        assert!(matches!(
            store.acquire_mutation_lock(),
            Err(StateError::UnsafeRuntimePath(_))
        ));
    }

    #[cfg(unix)]
    #[test]
    fn unix_nested_runtime_and_artifact_guards_reject_path_replacement() {
        use std::os::unix::fs::PermissionsExt;

        let directory = tempdir().unwrap();
        let store = StateStore::new(directory.path().join("state"));
        let mutation = store.acquire_mutation_lock().unwrap();
        store.installation().unwrap();
        let cell_id = CellId::new();
        let runtime = store.prepare_cell_runtime(cell_id).unwrap();
        let cell_root = store.cell_runtime_root(cell_id);
        let retired_cell = store.root().join("runtime-retired");
        fs::rename(&cell_root, &retired_cell).unwrap();
        fs::create_dir(&cell_root).unwrap();
        fs::set_permissions(&cell_root, fs::Permissions::from_mode(0o700)).unwrap();
        fs::write(cell_root.join("foreign"), b"retain").unwrap();
        assert!(matches!(
            runtime.validate_filesystem_identity(),
            Err(StateError::UnsafeRuntimePath(_))
        ));
        assert!(matches!(
            store.remove_cell_runtime(cell_id, runtime),
            Err(StateError::UnsafeRuntimePath(_))
        ));
        assert_eq!(fs::read(cell_root.join("foreign")).unwrap(), b"retain");

        let artifact_cell = CellId::new();
        let operation_id = GuestOperationId::new();
        let artifact = store
            .prepare_artifact_root(artifact_cell, operation_id)
            .unwrap();
        let artifact_root = store.artifact_root(artifact_cell, operation_id);
        let retired_artifact = store.root().join("artifact-retired");
        fs::rename(&artifact_root, &retired_artifact).unwrap();
        fs::create_dir(&artifact_root).unwrap();
        fs::set_permissions(&artifact_root, fs::Permissions::from_mode(0o700)).unwrap();
        fs::create_dir(artifact_root.join("files")).unwrap();
        fs::set_permissions(
            artifact_root.join("files"),
            fs::Permissions::from_mode(0o700),
        )
        .unwrap();
        assert!(matches!(
            store.validate_artifact_guard(&artifact),
            Err(StateError::UnsafeRuntimePath(_))
        ));
        drop(artifact);
        drop(mutation);
    }

    fn test_cell_record(store: &StateStore, cell_id: CellId) -> CellRecord {
        let image_id = ImageId::parse("crash-base").unwrap();
        let variant = ImageVariant {
            provider: "hyperv".to_owned(),
            disk_format: "vhdx".to_owned(),
            path: store.root().join("base.vhdx"),
            sha256: "abc".to_owned(),
            file_size: 42,
        };
        let now = Utc::now();
        CellRecord {
            schema_version: CELL_SCHEMA_VERSION,
            id: cell_id,
            provider: "hyperv".to_owned(),
            spec: CellSpec {
                image: image_id.clone(),
                provider: Some("hyperv".to_owned()),
                cpu_count: 2,
                memory_mib: 4096,
                ttl_seconds: None,
                accelerator: None,
                allow_tcg: false,
            },
            image: ImageBinding::from_variant(image_id, GuestOs::Windows, &variant),
            ownership: CellOwnership::new(
                Uuid::new_v4(),
                cell_id,
                Uuid::new_v4(),
                store.cell_configuration_path(cell_id),
                store.cell_overlay_path(cell_id),
            ),
            provider_object: None,
            state: CellState::Creating,
            phase: CellPhase::IntentRecorded,
            created_at: now,
            updated_at: now,
            expires_at: None,
            last_error: Some("vmcell.test.baseline".to_owned()),
            job: None,
        }
    }

    fn test_job_cell_record(
        store: &StateStore,
        cell_id: CellId,
        job: JobCorrelation,
    ) -> CellRecord {
        let mut cell = test_cell_record(store, cell_id);
        cell.schema_version = CellRecord::schema_version_for_job(Some(&job));
        cell.job = Some(job);
        cell
    }

    fn phase_and_state(label: &str) -> (CellPhase, CellState) {
        match label {
            "intent" => (CellPhase::IntentRecorded, CellState::Creating),
            "overlay" => (CellPhase::OverlayCreated, CellState::Creating),
            "provider_created" => (CellPhase::ProviderObjectCreated, CellState::Creating),
            "provider_claimed" => (CellPhase::ProviderObjectClaimed, CellState::Creating),
            "ready" => (CellPhase::Ready, CellState::Stopped),
            "destroying_provisioning" => (CellPhase::DestroyingProvisioning, CellState::Destroying),
            "destroying" => (CellPhase::Destroying, CellState::Destroying),
            "destroyed" => (CellPhase::Destroyed, CellState::Destroyed),
            _ => panic!("unknown test phase {label}"),
        }
    }

    fn subprocess_for(test_name: &str) -> Command {
        let mut command = Command::new(std::env::current_exe().unwrap());
        command.arg("--exact").arg(test_name).arg("--nocapture");
        command
    }

    #[test]
    fn installation_identity_is_durable() {
        let directory = tempdir().unwrap();
        let store = StateStore::new(directory.path().join("state"));

        let first = store.installation().unwrap();
        let second = store.installation().unwrap();

        assert_eq!(first, second);
    }

    #[test]
    fn loading_installation_never_creates_a_replacement() {
        let directory = tempdir().unwrap();
        let store = StateStore::new(directory.path().join("state"));

        assert!(matches!(
            store.load_installation(),
            Err(StateError::NotFound(_))
        ));
        assert!(!store.root().join("installation.json").exists());
    }

    #[test]
    fn compatibility_check_is_read_only_for_empty_and_field_omitting_legacy_state() {
        use crate::core::job::{JobCorrelation, JobId};

        let directory = tempdir().unwrap();
        let empty_root = directory.path().join("empty-state");
        let empty = StateStore::new(empty_root.clone())
            .check_compatibility()
            .unwrap();
        assert_eq!(empty.status, StateCompatibilityStatus::Empty);
        assert_eq!(empty.counts, StateCompatibilityCounts::default());
        assert!(!empty_root.exists());

        let store = StateStore::new(directory.path().join("legacy-state"));
        let mutation = store.acquire_mutation_lock().unwrap();
        store.installation().unwrap();
        let image_id = ImageId::parse("crash-base").unwrap();
        let image = ImageRecord {
            schema_version: IMAGE_SCHEMA_VERSION,
            id: image_id,
            guest_os: GuestOs::Windows,
            guest_arch: Architecture::X86_64,
            variants: vec![ImageVariant {
                provider: "hyperv".to_owned(),
                disk_format: "vhdx".to_owned(),
                path: directory.path().join("legacy-base.vhdx"),
                sha256: "legacy-fixture-hash".to_owned(),
                file_size: 42,
            }],
            registered_at: Utc::now(),
        };
        store.save_image_new(&image).unwrap();
        let cell_id = CellId::new();
        let now = Utc::now();
        let correlation =
            JobCorrelation::new(JobId::new(), "a".repeat(64), now).expect("valid correlation");
        let cell = test_job_cell_record(&store, cell_id, correlation.clone());
        store.save_cell(&cell).unwrap();
        let mut operation = GuestOperationRecord::intent_with_job(
            cell_id,
            GuestOperationKind::ArtifactCollect,
            now,
            Some(correlation.job_id),
        );
        operation.phase = GuestOperationPhase::TransportActive;
        store.save_guest_operation(&operation).unwrap();
        let guard = store.prepare_artifact_root(cell_id, operation.id).unwrap();
        let relative = store
            .write_artifact_file(&guard, 0, b"legacy-correlation-artifact")
            .unwrap();
        let artifact = ArtifactRecord {
            schema_version: ArtifactRecord::schema_version_for_job(Some(correlation.job_id)),
            id: operation.id,
            cell_id,
            created_at: now,
            entries: vec![ArtifactEntry {
                guest_path: "results/output.bin".to_owned(),
                host_relative_path: relative,
                sha256: format!("{:x}", Sha256::digest(b"legacy-correlation-artifact")),
                size: 27,
            }],
            job_id: Some(correlation.job_id),
        };
        store.save_artifact_new(&guard, &artifact).unwrap();
        operation.phase = GuestOperationPhase::ArtifactCommitted;
        operation.artifact_id = Some(operation.id);
        store.save_guest_operation(&operation).unwrap();

        let cell_path = store.cell_path(cell_id);
        let operation_path = store.guest_operation_path(operation.id);
        let artifact_path = store
            .artifact_root(cell_id, operation.id)
            .join("manifest.json");
        let mut legacy_cell = serde_json::to_value(&cell).unwrap();
        assert!(legacy_cell.as_object_mut().unwrap().remove("job").is_some());
        legacy_cell["schema_version"] = serde_json::json!(CELL_SCHEMA_VERSION);
        let mut legacy_operation = serde_json::to_value(&operation).unwrap();
        assert!(
            legacy_operation
                .as_object_mut()
                .unwrap()
                .remove("artifact_pruned_at")
                .is_some()
        );
        assert!(
            legacy_operation
                .as_object_mut()
                .unwrap()
                .remove("job_id")
                .is_some()
        );
        legacy_operation["schema_version"] = serde_json::json!(GUEST_OPERATION_SCHEMA_VERSION);
        let mut legacy_artifact = serde_json::to_value(&artifact).unwrap();
        assert!(
            legacy_artifact
                .as_object_mut()
                .unwrap()
                .remove("job_id")
                .is_some()
        );
        legacy_artifact["schema_version"] = serde_json::json!(ARTIFACT_SCHEMA_VERSION);
        fs::write(
            &cell_path,
            format!("{}\n", serde_json::to_string_pretty(&legacy_cell).unwrap()),
        )
        .unwrap();
        fs::write(
            &operation_path,
            format!(
                "{}\n",
                serde_json::to_string_pretty(&legacy_operation).unwrap()
            ),
        )
        .unwrap();
        fs::write(
            &artifact_path,
            format!(
                "{}\n",
                serde_json::to_string_pretty(&legacy_artifact).unwrap()
            ),
        )
        .unwrap();
        drop(guard);
        drop(mutation);

        let tracked_paths = [
            store.root().join("installation.json"),
            store.image_path(&image.id),
            cell_path,
            operation_path,
            artifact_path,
        ];
        let before = tracked_paths
            .iter()
            .map(|path| fs::read(path).unwrap())
            .collect::<Vec<_>>();
        let report = store.check_compatibility().unwrap();
        let after = tracked_paths
            .iter()
            .map(|path| fs::read(path).unwrap())
            .collect::<Vec<_>>();

        assert_eq!(report.contract, STATE_COMPATIBILITY_CONTRACT);
        assert_eq!(report.durable_state_format_version, 1);
        assert_eq!(report.status, StateCompatibilityStatus::Compatible);
        assert_eq!(report.counts.installations, 1);
        assert_eq!(report.counts.images, 1);
        assert_eq!(report.counts.cells, 1);
        assert_eq!(report.counts.guest_operations, 1);
        assert_eq!(report.counts.artifacts, 1);
        assert_eq!(after, before);
    }

    #[test]
    fn field_omitting_legacy_operation_without_a_parent_cell_remains_readable() {
        let directory = tempdir().unwrap();
        let store = StateStore::new(directory.path().join("legacy-operation-state"));
        let mutation = store.acquire_mutation_lock().unwrap();
        let now = Utc::now();
        let mut operation =
            GuestOperationRecord::intent(CellId::new(), GuestOperationKind::Exec, now);
        operation.phase = GuestOperationPhase::Failed;
        operation.failure = Some(GuestFailureClass::Authentication);
        operation.completed_at = Some(now);
        let path = store.guest_operation_path(operation.id);
        let mut legacy = serde_json::to_value(&operation).unwrap();
        assert!(
            legacy
                .as_object_mut()
                .unwrap()
                .remove("artifact_pruned_at")
                .is_some()
        );
        assert!(legacy.get("job_id").is_none());
        write_json_atomic(&path, &legacy).unwrap();
        drop(mutation);

        let before = fs::read(&path).unwrap();
        assert_eq!(store.load_guest_operation(operation.id).unwrap(), operation);
        assert_eq!(
            store.list_guest_operations().unwrap(),
            vec![operation.clone()]
        );
        let report = store.check_compatibility().unwrap();
        let after = fs::read(&path).unwrap();

        assert_eq!(report.status, StateCompatibilityStatus::Compatible);
        assert_eq!(report.counts.cells, 0);
        assert_eq!(report.counts.guest_operations, 1);
        assert_eq!(after, before);
        assert!(matches!(
            store.save_guest_operation(&operation),
            Err(StateError::JobCorrelationIntegrity {
                reason: "guest operation references a missing cell",
                ..
            })
        ));
        assert_eq!(fs::read(&path).unwrap(), before);
        let guard = store
            .prepare_artifact_root(operation.cell_id, operation.id)
            .unwrap();
        let relative = store
            .write_artifact_file(&guard, 0, b"legacy-orphan-artifact")
            .unwrap();
        let artifact = ArtifactRecord {
            schema_version: ARTIFACT_SCHEMA_VERSION,
            id: operation.id,
            cell_id: operation.cell_id,
            created_at: now,
            entries: vec![ArtifactEntry {
                guest_path: "results/orphan.bin".to_owned(),
                host_relative_path: relative,
                sha256: format!("{:x}", Sha256::digest(b"legacy-orphan-artifact")),
                size: 22,
            }],
            job_id: None,
        };
        assert!(matches!(
            store.save_artifact_new(&guard, &artifact),
            Err(StateError::JobCorrelationIntegrity {
                reason: "guest operation references a missing cell",
                ..
            })
        ));
        assert!(!guard.root.join("manifest.json").exists());
    }

    #[test]
    fn compatibility_check_rejects_future_schema_without_rewriting_it() {
        let directory = tempdir().unwrap();
        let store = StateStore::new(directory.path().join("state"));
        let mutation = store.acquire_mutation_lock().unwrap();
        let image_id = ImageId::parse("future-base").unwrap();
        let image = ImageRecord {
            schema_version: IMAGE_SCHEMA_VERSION,
            id: image_id.clone(),
            guest_os: GuestOs::Windows,
            guest_arch: Architecture::X86_64,
            variants: vec![ImageVariant {
                provider: "hyperv".to_owned(),
                disk_format: "vhdx".to_owned(),
                path: directory.path().join("future.vhdx"),
                sha256: "future-fixture-hash".to_owned(),
                file_size: 42,
            }],
            registered_at: Utc::now(),
        };
        store.save_image_new(&image).unwrap();
        let path = store.image_path(&image_id);
        let mut future = serde_json::to_value(&image).unwrap();
        future["schema_version"] = serde_json::json!(IMAGE_SCHEMA_VERSION + 1);
        fs::write(
            &path,
            format!("{}\n", serde_json::to_string_pretty(&future).unwrap()),
        )
        .unwrap();
        drop(mutation);
        let before = fs::read(&path).unwrap();

        assert!(matches!(
            store.check_compatibility(),
            Err(StateError::UpgradeRequired {
                kind: "image record",
                supported: IMAGE_SCHEMA_VERSION,
                actual,
            }) if actual == IMAGE_SCHEMA_VERSION + 1
        ));
        assert_eq!(fs::read(path).unwrap(), before);
    }

    #[test]
    fn image_and_cell_manifests_round_trip() {
        let directory = tempdir().unwrap();
        let store = StateStore::new(directory.path().join("state"));
        let image_id = ImageId::parse("windows-dev").unwrap();
        let image = ImageRecord {
            schema_version: 1,
            id: image_id.clone(),
            guest_os: GuestOs::Windows,
            guest_arch: Architecture::X86_64,
            variants: vec![ImageVariant {
                provider: "hyperv".to_owned(),
                disk_format: "vhdx".to_owned(),
                path: directory.path().join("base.vhdx"),
                sha256: "abc".to_owned(),
                file_size: 42,
            }],
            registered_at: Utc::now(),
        };
        store.save_image_new(&image).unwrap();
        assert_eq!(store.load_image(&image_id).unwrap(), image);

        let cell_id = CellId::new();
        let now = Utc::now();
        let ownership = CellOwnership::new(
            Uuid::new_v4(),
            cell_id,
            Uuid::new_v4(),
            store.cell_configuration_path(cell_id),
            store.cell_overlay_path(cell_id),
        );
        let mut cell = CellRecord {
            schema_version: 1,
            id: cell_id,
            provider: "hyperv".to_owned(),
            spec: CellSpec {
                image: image_id.clone(),
                provider: Some("hyperv".to_owned()),
                cpu_count: 2,
                memory_mib: 4096,
                ttl_seconds: None,
                accelerator: None,
                allow_tcg: false,
            },
            image: ImageBinding::from_variant(image_id, image.guest_os, &image.variants[0]),
            ownership,
            provider_object: None,
            state: CellState::Creating,
            phase: CellPhase::IntentRecorded,
            created_at: now,
            updated_at: now,
            expires_at: None,
            last_error: None,
            job: None,
        };
        store.save_cell(&cell).unwrap();
        assert_eq!(store.load_cell(cell_id).unwrap().id, cell_id);

        cell.last_error = Some("vmcell.test.updated_atomically".to_owned());
        store.save_cell(&cell).unwrap();
        assert_eq!(
            store.load_cell(cell_id).unwrap().last_error.as_deref(),
            Some("vmcell.test.updated_atomically")
        );

        let mut legacy = cell.clone();
        legacy.last_error = Some("credential-sentinel raw provider stderr".to_owned());
        fs::write(
            store.cell_path(cell_id),
            serde_json::to_vec(&legacy).unwrap(),
        )
        .unwrap();
        assert_eq!(
            store.load_cell(cell_id).unwrap().last_error.as_deref(),
            Some(REDACTED_LEGACY_ERROR_CODE)
        );
        assert_eq!(
            store.list_cells().unwrap()[0].last_error.as_deref(),
            Some(REDACTED_LEGACY_ERROR_CODE)
        );
    }

    #[test]
    fn image_record_removal_is_atomic_idempotent_and_never_touches_base_bytes() {
        if std::env::var_os("VMCELL_TEST_IMAGE_REMOVE_CHILD").is_some() {
            let root = PathBuf::from(std::env::var_os("VMCELL_TEST_STATE_ROOT").unwrap());
            let image_id = ImageId::parse(std::env::var("VMCELL_TEST_IMAGE_ID").unwrap()).unwrap();
            let store = StateStore::new(root);
            let mutation = store.acquire_mutation_lock().unwrap();
            assert!(store.remove_image_record(&mutation, &image_id).unwrap());
            std::process::exit(77);
        }

        let directory = tempdir().unwrap();
        let store = StateStore::new(directory.path().join("state"));
        let image_id = ImageId::parse("removable-image").unwrap();
        let base_path = directory.path().join("base.vhdx");
        fs::write(&base_path, b"immutable-base-sentinel").unwrap();
        store
            .save_image_new(&ImageRecord {
                schema_version: IMAGE_SCHEMA_VERSION,
                id: image_id.clone(),
                guest_os: GuestOs::Windows,
                guest_arch: Architecture::X86_64,
                variants: vec![ImageVariant {
                    provider: "hyperv".to_owned(),
                    disk_format: "vhdx".to_owned(),
                    path: base_path.clone(),
                    sha256: "sentinel-hash".to_owned(),
                    file_size: 23,
                }],
                registered_at: Utc::now(),
            })
            .unwrap();
        let manifest_path = store.image_path(&image_id);
        let manifest_bytes = fs::read(&manifest_path).unwrap();

        let before = subprocess_for(
            "state::tests::image_record_removal_is_atomic_idempotent_and_never_touches_base_bytes",
        )
        .env("VMCELL_TEST_IMAGE_REMOVE_CHILD", "1")
        .env("VMCELL_TEST_ATOMIC_CRASH_CHILD", "1")
        .env("VMCELL_TEST_ABORT_AT", "before_image_remove")
        .env("VMCELL_TEST_STATE_ROOT", store.root())
        .env("VMCELL_TEST_IMAGE_ID", image_id.as_str())
        .output()
        .unwrap();
        assert!(!before.status.success());
        assert_ne!(before.status.code(), Some(77));
        assert!(store.load_image(&image_id).is_ok());
        assert_eq!(fs::read(&base_path).unwrap(), b"immutable-base-sentinel");

        let after = subprocess_for(
            "state::tests::image_record_removal_is_atomic_idempotent_and_never_touches_base_bytes",
        )
        .env("VMCELL_TEST_IMAGE_REMOVE_CHILD", "1")
        .env("VMCELL_TEST_ATOMIC_CRASH_CHILD", "1")
        .env("VMCELL_TEST_ABORT_AT", "after_image_remove")
        .env("VMCELL_TEST_STATE_ROOT", store.root())
        .env("VMCELL_TEST_IMAGE_ID", image_id.as_str())
        .output()
        .unwrap();
        assert!(!after.status.success());
        assert_ne!(after.status.code(), Some(77));
        assert!(matches!(
            store.load_image(&image_id),
            Err(StateError::NotFound(_))
        ));
        assert_eq!(fs::read(&base_path).unwrap(), b"immutable-base-sentinel");
        let retired = fs::read_dir(store.root().join("images"))
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .filter(|path| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.starts_with("removable-image.json.unregistered-"))
            })
            .collect::<Vec<_>>();
        assert_eq!(retired.len(), 1);
        assert_eq!(fs::read(&retired[0]).unwrap(), manifest_bytes);

        let mutation = store.acquire_mutation_lock().unwrap();
        assert!(!store.remove_image_record(&mutation, &image_id).unwrap());
    }

    #[test]
    fn image_record_removal_rejects_manifest_alias_without_touching_it() {
        let directory = tempdir().unwrap();
        let store = StateStore::new(directory.path().join("state"));
        let mutation = store.acquire_mutation_lock().unwrap();
        let image_id = ImageId::parse("aliased-image").unwrap();
        let manifest_path = store.image_path(&image_id);
        let alias_path = directory.path().join("base.vhdx");
        let record = ImageRecord {
            schema_version: IMAGE_SCHEMA_VERSION,
            id: image_id.clone(),
            guest_os: GuestOs::Windows,
            guest_arch: Architecture::X86_64,
            variants: vec![ImageVariant {
                provider: "hyperv".to_owned(),
                disk_format: "vhdx".to_owned(),
                path: alias_path.clone(),
                sha256: "forged-manifest-alias".to_owned(),
                file_size: 1,
            }],
            registered_at: Utc::now(),
        };
        store.save_image_new(&record).unwrap();
        fs::hard_link(&manifest_path, &alias_path).unwrap();
        let before = fs::read(&manifest_path).unwrap();

        assert!(matches!(
            store.remove_image_record(&mutation, &image_id),
            Err(StateError::UnsafeRuntimePath(_))
        ));
        assert_eq!(fs::read(&manifest_path).unwrap(), before);
        assert_eq!(fs::read(&alias_path).unwrap(), before);
        assert_eq!(store.load_image(&image_id).unwrap(), record);
    }

    #[test]
    fn image_record_removal_rejects_valid_format_reparse_alias() {
        let directory = tempdir().unwrap();
        let store = StateStore::new(directory.path().join("state"));
        let mutation = store.acquire_mutation_lock().unwrap();
        let image_id = ImageId::parse("reparse-aliased-image").unwrap();
        let manifest_path = store.image_path(&image_id);
        let alias_path = directory.path().join("base.vhdx");
        let record = ImageRecord {
            schema_version: IMAGE_SCHEMA_VERSION,
            id: image_id.clone(),
            guest_os: GuestOs::Windows,
            guest_arch: Architecture::X86_64,
            variants: vec![ImageVariant {
                provider: "hyperv".to_owned(),
                disk_format: "vhdx".to_owned(),
                path: alias_path.clone(),
                sha256: "forged-reparse-alias".to_owned(),
                file_size: 1,
            }],
            registered_at: Utc::now(),
        };
        store.save_image_new(&record).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(&manifest_path, &alias_path).unwrap();
        #[cfg(windows)]
        if std::os::windows::fs::symlink_file(&manifest_path, &alias_path).is_err() {
            return;
        }
        let before = fs::read(&manifest_path).unwrap();

        assert!(matches!(
            store.remove_image_record(&mutation, &image_id),
            Err(StateError::UnsafeRuntimePath(_))
        ));
        assert_eq!(fs::read(&manifest_path).unwrap(), before);
    }

    #[cfg(windows)]
    #[test]
    fn image_record_removal_rejects_manifest_alternate_data_stream_alias() {
        let directory = tempdir().unwrap();
        let store = StateStore::new(directory.path().join("state"));
        let mutation = store.acquire_mutation_lock().unwrap();
        let image_id = ImageId::parse("stream-aliased-image").unwrap();
        let manifest_path = store.image_path(&image_id);
        let stream_path = PathBuf::from(format!("{}:base.vhdx", manifest_path.display()));
        let record = ImageRecord {
            schema_version: IMAGE_SCHEMA_VERSION,
            id: image_id.clone(),
            guest_os: GuestOs::Windows,
            guest_arch: Architecture::X86_64,
            variants: vec![ImageVariant {
                provider: "hyperv".to_owned(),
                disk_format: "vhdx".to_owned(),
                path: stream_path,
                sha256: "forged-stream-alias".to_owned(),
                file_size: 1,
            }],
            registered_at: Utc::now(),
        };
        store.save_image_new(&record).unwrap();
        let before = fs::read(&manifest_path).unwrap();

        assert!(matches!(
            store.remove_image_record(&mutation, &image_id),
            Err(StateError::UnsafeRuntimePath(_))
        ));
        assert_eq!(fs::read(&manifest_path).unwrap(), before);
    }

    #[cfg(windows)]
    #[test]
    fn image_removal_path_policy_rejects_windows_device_namespaces_and_names() {
        for path in [
            r"\\.\PhysicalDrive0.vhdx",
            r"\\?\GLOBALROOT\Device\HarddiskVolumeShadowCopy1\base.vhdx",
            r"C:\images\NUL.vhdx",
            r"C:\images\COM1.vhdx",
            r"C:\images\LPT².vhdx",
        ] {
            assert!(
                windows_path_has_stream_or_device_ambiguity(Path::new(path)),
                "{path}"
            );
        }
        assert!(!windows_path_has_stream_or_device_ambiguity(Path::new(
            r"C:\images\base.vhdx"
        )));
    }

    #[test]
    fn guest_operation_and_artifact_records_are_identity_bound_and_secret_free() {
        let directory = tempdir().unwrap();
        let store = StateStore::new(directory.path().join("state"));
        let _mutation = store.acquire_mutation_lock().unwrap();
        let cell_id = CellId::new();
        let cell = test_cell_record(&store, cell_id);
        assert!(serde_json::to_value(&cell).unwrap().get("job").is_none());
        store.save_cell(&cell).unwrap();
        let now = Utc::now();
        let mut operation = GuestOperationRecord::intent(cell_id, GuestOperationKind::Exec, now);
        operation.phase = GuestOperationPhase::Failed;
        operation.failure = Some(GuestFailureClass::Authentication);
        operation.completed_at = Some(now);
        store.save_guest_operation(&operation).unwrap();
        assert_eq!(store.load_guest_operation(operation.id).unwrap(), operation);
        let operation_json = fs::read_to_string(store.guest_operation_path(operation.id)).unwrap();
        assert!(!operation_json.contains("credential-sentinel"));
        assert!(!operation_json.contains("argument-sentinel"));
        assert!(!operation_json.contains("\"job_id\""));

        let mut ambiguous = GuestOperationRecord::intent(cell_id, GuestOperationKind::Exec, now);
        ambiguous.phase = GuestOperationPhase::Failed;
        ambiguous.failure = Some(GuestFailureClass::Unknown);
        ambiguous.completed_at = Some(now);
        assert!(matches!(
            store.save_guest_operation(&ambiguous),
            Err(StateError::GuestOperationIntegrity { .. })
        ));

        let artifact_id = GuestOperationId::new();
        let mut artifact_operation =
            GuestOperationRecord::intent(cell_id, GuestOperationKind::ArtifactCollect, now);
        artifact_operation.id = artifact_id;
        artifact_operation.phase = GuestOperationPhase::TransportActive;
        store.save_guest_operation(&artifact_operation).unwrap();
        let guard = store.prepare_artifact_root(cell_id, artifact_id).unwrap();
        let relative = store
            .write_artifact_file(&guard, 0, b"bounded-artifact")
            .unwrap();
        let artifact = ArtifactRecord {
            schema_version: ARTIFACT_SCHEMA_VERSION,
            id: artifact_id,
            cell_id,
            created_at: now,
            entries: vec![ArtifactEntry {
                guest_path: "results/output.bin".to_owned(),
                host_relative_path: relative,
                sha256: format!("{:x}", Sha256::digest(b"bounded-artifact")),
                size: 16,
            }],
            job_id: None,
        };
        store.save_artifact_new(&guard, &artifact).unwrap();
        assert_eq!(store.load_artifact(cell_id, artifact_id).unwrap(), artifact);
        assert!(
            serde_json::to_value(&artifact)
                .unwrap()
                .get("job_id")
                .is_none()
        );

        let mut oversized_file = artifact.clone();
        oversized_file.entries[0].size = MAX_ARTIFACT_FILE_BYTES + 1;
        assert!(matches!(
            store.save_artifact_new(&guard, &oversized_file),
            Err(StateError::ArtifactIntegrity { .. })
        ));
        let mut too_many_files = artifact.clone();
        too_many_files.entries = vec![artifact.entries[0].clone(); MAX_ARTIFACT_FILES + 1];
        assert!(matches!(
            store.save_artifact_new(&guard, &too_many_files),
            Err(StateError::ArtifactIntegrity { .. })
        ));

        let artifact_file = guard.files_root.join("0000.bin");
        fs::write(&artifact_file, b"tampered-artifact").unwrap();
        assert!(matches!(
            store.load_artifact(cell_id, artifact_id),
            Err(StateError::ArtifactIntegrity { .. })
        ));
        fs::write(&artifact_file, b"bounded-artifact").unwrap();
        let extra_file = guard.files_root.join("9999.bin");
        fs::write(&extra_file, b"unexpected").unwrap();
        assert!(matches!(
            store.load_artifact(cell_id, artifact_id),
            Err(StateError::ArtifactIntegrity { .. })
        ));
        fs::remove_file(extra_file).unwrap();

        let manifest_path = guard.root.join("manifest.json");
        let mut unsupported = artifact.clone();
        unsupported.schema_version = MAX_ARTIFACT_SCHEMA_VERSION + 1;
        fs::write(&manifest_path, serde_json::to_vec(&unsupported).unwrap()).unwrap();
        assert!(matches!(
            store.load_artifact(cell_id, artifact_id),
            Err(StateError::UnsupportedSchema {
                kind: "artifact record",
                ..
            })
        ));

        fs::write(&manifest_path, serde_json::to_vec(&artifact).unwrap()).unwrap();
        fs::remove_file(&artifact_file).unwrap();
        let external = directory.path().join("external-artifact.bin");
        fs::write(&external, b"bounded-artifact").unwrap();
        if create_file_link(&external, &artifact_file).is_ok() {
            assert!(matches!(
                store.load_artifact(cell_id, artifact_id),
                Err(StateError::UnsafeRuntimePath(_))
            ));
        }
    }

    #[test]
    fn job_correlation_is_validated_immutable_and_bound_across_records() {
        use crate::core::job::{JobCorrelation, JobId};

        let directory = tempdir().unwrap();
        let store = StateStore::new(directory.path().join("state"));
        let _mutation = store.acquire_mutation_lock().unwrap();
        let cell_id = CellId::new();
        let now = Utc::now();
        let correlation =
            JobCorrelation::new(JobId::new(), "d".repeat(64), now).expect("valid correlation");
        let cell = test_job_cell_record(&store, cell_id, correlation.clone());
        store.save_cell(&cell).unwrap();

        let operation = GuestOperationRecord::intent_with_job(
            cell_id,
            GuestOperationKind::ArtifactCollect,
            now,
            Some(correlation.job_id),
        );
        store.save_guest_operation(&operation).unwrap();

        let mut mutated_cell = cell.clone();
        mutated_cell.job = None;
        mutated_cell.schema_version = CellRecord::schema_version_for_job(None);
        assert!(matches!(
            store.save_cell(&mutated_cell),
            Err(StateError::JobCorrelationIntegrity {
                reason: "cell job correlation is immutable",
                ..
            })
        ));

        let mut mutated_operation = operation.clone();
        mutated_operation.job_id = None;
        mutated_operation.schema_version = GuestOperationRecord::schema_version_for_job(None);
        assert!(matches!(
            store.save_guest_operation(&mutated_operation),
            Err(StateError::JobCorrelationIntegrity {
                reason: "guest operation job correlation is immutable",
                ..
            })
        ));

        let mismatched = GuestOperationRecord::intent_with_job(
            cell_id,
            GuestOperationKind::Exec,
            now,
            Some(JobId::new()),
        );
        assert!(matches!(
            store.save_guest_operation(&mismatched),
            Err(StateError::JobCorrelationIntegrity {
                reason: "guest operation job id does not match its cell",
                ..
            })
        ));

        let guard = store.prepare_artifact_root(cell_id, operation.id).unwrap();
        let relative = store
            .write_artifact_file(&guard, 0, b"correlated-artifact")
            .unwrap();
        let artifact = ArtifactRecord {
            schema_version: ArtifactRecord::schema_version_for_job(Some(JobId::new())),
            id: operation.id,
            cell_id,
            created_at: now,
            entries: vec![ArtifactEntry {
                guest_path: "result.bin".to_owned(),
                host_relative_path: relative,
                sha256: format!("{:x}", Sha256::digest(b"correlated-artifact")),
                size: 19,
            }],
            job_id: Some(JobId::new()),
        };
        assert!(matches!(
            store.save_artifact_new(&guard, &artifact),
            Err(StateError::JobCorrelationIntegrity {
                reason: "artifact job id does not match its guest operation",
                ..
            })
        ));

        let invalid = JobCorrelation {
            job_id: correlation.job_id,
            job_spec_sha256: "not-a-digest".to_owned(),
            started_at: now,
        };
        let invalid_cell = test_job_cell_record(&store, CellId::new(), invalid);
        assert!(matches!(
            store.save_cell(&invalid_cell),
            Err(StateError::JobCorrelationIntegrity {
                reason: "job specification digest is not canonical SHA-256",
                ..
            })
        ));
    }

    #[test]
    fn job_correlated_records_use_v2_and_direct_operations_on_them_remain_v1() {
        use crate::core::job::{JobCorrelation, JobId};

        let directory = tempdir().unwrap();
        let store = StateStore::new(directory.path().join("state"));
        let _mutation = store.acquire_mutation_lock().unwrap();
        let cell_id = CellId::new();
        let now = Utc::now();
        let correlation =
            JobCorrelation::new(JobId::new(), "7".repeat(64), now).expect("valid correlation");
        let cell = test_job_cell_record(&store, cell_id, correlation.clone());
        store.save_cell(&cell).unwrap();
        assert_eq!(cell.schema_version, JOB_CORRELATED_CELL_SCHEMA_VERSION);

        let mut operation = GuestOperationRecord::intent_with_job(
            cell_id,
            GuestOperationKind::ArtifactCollect,
            now,
            Some(correlation.job_id),
        );
        assert_eq!(
            operation.schema_version,
            JOB_CORRELATED_GUEST_OPERATION_SCHEMA_VERSION
        );
        operation.phase = GuestOperationPhase::TransportActive;
        store.save_guest_operation(&operation).unwrap();

        let guard = store.prepare_artifact_root(cell_id, operation.id).unwrap();
        let relative = store
            .write_artifact_file(&guard, 0, b"v2-correlated-artifact")
            .unwrap();
        let artifact = ArtifactRecord {
            schema_version: ArtifactRecord::schema_version_for_job(Some(correlation.job_id)),
            id: operation.id,
            cell_id,
            created_at: now,
            entries: vec![ArtifactEntry {
                guest_path: "results/v2.bin".to_owned(),
                host_relative_path: relative,
                sha256: format!("{:x}", Sha256::digest(b"v2-correlated-artifact")),
                size: 22,
            }],
            job_id: Some(correlation.job_id),
        };
        store.save_artifact_new(&guard, &artifact).unwrap();
        assert_eq!(
            artifact.schema_version,
            JOB_CORRELATED_ARTIFACT_SCHEMA_VERSION
        );

        operation.phase = GuestOperationPhase::ArtifactCommitted;
        operation.artifact_id = Some(operation.id);
        store.save_guest_operation(&operation).unwrap();
        operation.phase = GuestOperationPhase::Completed;
        operation.completed_at = Some(now);
        store.save_guest_operation(&operation).unwrap();
        assert_eq!(
            store
                .load_guest_operation(operation.id)
                .unwrap()
                .schema_version,
            JOB_CORRELATED_GUEST_OPERATION_SCHEMA_VERSION
        );
        assert_eq!(
            store
                .load_artifact(cell_id, operation.id)
                .unwrap()
                .schema_version,
            JOB_CORRELATED_ARTIFACT_SCHEMA_VERSION
        );

        // A later direct command on the retained job cell is intentionally not
        // retroactively attributed and remains an ordinary v1 record.
        let mut direct = GuestOperationRecord::intent(cell_id, GuestOperationKind::Exec, now);
        direct.phase = GuestOperationPhase::Failed;
        direct.failure = Some(GuestFailureClass::Authentication);
        direct.completed_at = Some(now);
        store.save_guest_operation(&direct).unwrap();
        assert_eq!(direct.schema_version, GUEST_OPERATION_SCHEMA_VERSION);
        assert!(direct.job_id.is_none());

        let report = store.check_compatibility().unwrap();
        assert_eq!(report.status, StateCompatibilityStatus::Compatible);
        assert_eq!(
            report.durable_state_format_version,
            DURABLE_STATE_FORMAT_VERSION
        );
    }

    #[test]
    fn correlation_and_schema_versions_are_mutually_required() {
        use crate::core::job::{JobCorrelation, JobId};

        let directory = tempdir().unwrap();
        let store = StateStore::new(directory.path().join("state"));
        let _mutation = store.acquire_mutation_lock().unwrap();
        let cell_id = CellId::new();
        let now = Utc::now();
        let correlation =
            JobCorrelation::new(JobId::new(), "8".repeat(64), now).expect("valid correlation");

        let mut v1_cell_with_job = test_job_cell_record(&store, cell_id, correlation.clone());
        v1_cell_with_job.schema_version = CELL_SCHEMA_VERSION;
        assert!(matches!(
            store.save_cell(&v1_cell_with_job),
            Err(StateError::JobCorrelationIntegrity {
                reason: "cell schema v1 must not carry job correlation",
                ..
            })
        ));
        let mut v2_cell_without_job = test_cell_record(&store, cell_id);
        v2_cell_without_job.schema_version = JOB_CORRELATED_CELL_SCHEMA_VERSION;
        assert!(matches!(
            store.save_cell(&v2_cell_without_job),
            Err(StateError::JobCorrelationIntegrity {
                reason: "cell schema v2 requires job correlation",
                ..
            })
        ));

        let cell = test_job_cell_record(&store, cell_id, correlation.clone());
        store.save_cell(&cell).unwrap();
        let mut v1_operation_with_job = GuestOperationRecord::intent_with_job(
            cell_id,
            GuestOperationKind::ArtifactCollect,
            now,
            Some(correlation.job_id),
        );
        v1_operation_with_job.schema_version = GUEST_OPERATION_SCHEMA_VERSION;
        assert!(matches!(
            store.save_guest_operation(&v1_operation_with_job),
            Err(StateError::JobCorrelationIntegrity {
                reason: "guest operation schema v1 must not carry job correlation",
                ..
            })
        ));
        let mut v2_operation_without_job =
            GuestOperationRecord::intent(cell_id, GuestOperationKind::Exec, now);
        v2_operation_without_job.schema_version = JOB_CORRELATED_GUEST_OPERATION_SCHEMA_VERSION;
        assert!(matches!(
            store.save_guest_operation(&v2_operation_without_job),
            Err(StateError::JobCorrelationIntegrity {
                reason: "guest operation schema v2 requires job correlation",
                ..
            })
        ));

        let direct_operation =
            GuestOperationRecord::intent(cell_id, GuestOperationKind::ArtifactCollect, now);
        store.save_guest_operation(&direct_operation).unwrap();
        let guard = store
            .prepare_artifact_root(cell_id, direct_operation.id)
            .unwrap();
        let relative = store
            .write_artifact_file(&guard, 0, b"schema-bound-artifact")
            .unwrap();
        let mut v1_artifact_with_job = ArtifactRecord {
            schema_version: ARTIFACT_SCHEMA_VERSION,
            id: direct_operation.id,
            cell_id,
            created_at: now,
            entries: vec![ArtifactEntry {
                guest_path: "results/schema.bin".to_owned(),
                host_relative_path: relative.clone(),
                sha256: format!("{:x}", Sha256::digest(b"schema-bound-artifact")),
                size: 21,
            }],
            job_id: Some(correlation.job_id),
        };
        assert!(matches!(
            store.save_artifact_new(&guard, &v1_artifact_with_job),
            Err(StateError::JobCorrelationIntegrity {
                reason: "artifact schema v1 must not carry job correlation",
                ..
            })
        ));
        v1_artifact_with_job.schema_version = JOB_CORRELATED_ARTIFACT_SCHEMA_VERSION;
        v1_artifact_with_job.job_id = None;
        assert!(matches!(
            store.save_artifact_new(&guard, &v1_artifact_with_job),
            Err(StateError::JobCorrelationIntegrity {
                reason: "artifact schema v2 requires job correlation",
                ..
            })
        ));
    }

    #[test]
    fn v03_style_v1_schema_gate_refuses_each_correlated_v2_record_before_mutation() {
        let path = Path::new("v0.4-correlation.json");
        for (kind, actual) in [
            ("cell record", JOB_CORRELATED_CELL_SCHEMA_VERSION),
            (
                "guest operation record",
                JOB_CORRELATED_GUEST_OPERATION_SCHEMA_VERSION,
            ),
            ("artifact record", JOB_CORRELATED_ARTIFACT_SCHEMA_VERSION),
        ] {
            assert!(matches!(
                as_upgrade_required(ensure_schema(path, kind, actual, 1).unwrap_err()),
                StateError::UpgradeRequired {
                    kind: rejected_kind,
                    supported: 1,
                    actual: 2,
                } if rejected_kind == kind
            ));
        }
    }

    #[test]
    fn duplicate_job_id_is_rejected_on_save_and_normal_cell_reads() {
        use crate::core::job::{JobCorrelation, JobId};

        let directory = tempdir().unwrap();
        let store = StateStore::new(directory.path().join("state"));
        let _mutation = store.acquire_mutation_lock().unwrap();
        let correlation = JobCorrelation::new(JobId::new(), "e".repeat(64), Utc::now()).unwrap();

        let first = test_job_cell_record(&store, CellId::new(), correlation.clone());
        store.save_cell(&first).unwrap();

        let second = test_job_cell_record(&store, CellId::new(), correlation);
        assert!(matches!(
            store.save_cell(&second),
            Err(StateError::JobCorrelationIntegrity {
                reason: "job id is already bound to a different cell",
                ..
            })
        ));

        // A same-user writer can bypass save-time validation.  Ordinary
        // inspection and state compatibility must still fail closed.
        write_json_atomic(&store.cell_path(second.id), &second).unwrap();
        assert!(matches!(
            store.load_cell(second.id),
            Err(StateError::JobCorrelationIntegrity {
                reason: "job id is already bound to a different cell",
                ..
            })
        ));
        assert!(matches!(
            store.list_cells(),
            Err(StateError::JobCorrelationIntegrity {
                reason: "job id is already bound to a different cell",
                ..
            })
        ));
        assert!(store.check_compatibility().is_err());
    }

    #[test]
    fn persisted_operation_job_tampering_is_rejected_on_normal_reads() {
        use crate::core::job::{JobCorrelation, JobId};

        let directory = tempdir().unwrap();
        let store = StateStore::new(directory.path().join("state"));
        let _mutation = store.acquire_mutation_lock().unwrap();
        let cell_id = CellId::new();
        let now = Utc::now();
        let correlation =
            JobCorrelation::new(JobId::new(), "f".repeat(64), now).expect("valid correlation");
        let cell = test_job_cell_record(&store, cell_id, correlation.clone());
        store.save_cell(&cell).unwrap();

        let mut operation = GuestOperationRecord::intent_with_job(
            cell_id,
            GuestOperationKind::ArtifactCollect,
            now,
            Some(correlation.job_id),
        );
        operation.phase = GuestOperationPhase::TransportActive;
        store.save_guest_operation(&operation).unwrap();
        let guard = store.prepare_artifact_root(cell_id, operation.id).unwrap();
        let relative = store
            .write_artifact_file(&guard, 0, b"bound-artifact")
            .unwrap();
        store
            .save_artifact_new(
                &guard,
                &ArtifactRecord {
                    schema_version: ArtifactRecord::schema_version_for_job(Some(
                        correlation.job_id,
                    )),
                    id: operation.id,
                    cell_id,
                    created_at: now,
                    entries: vec![ArtifactEntry {
                        guest_path: "results/bound-artifact.bin".to_owned(),
                        host_relative_path: relative,
                        sha256: format!("{:x}", Sha256::digest(b"bound-artifact")),
                        size: 14,
                    }],
                    job_id: Some(correlation.job_id),
                },
            )
            .unwrap();

        // Simulate a same-user external writer changing both operation and
        // potential artifact provenance.  Loading the operation first gives
        // all normal public surfaces the parent-cell integrity check.
        operation.job_id = Some(JobId::new());
        fs::write(
            store.guest_operation_path(operation.id),
            serde_json::to_vec(&operation).unwrap(),
        )
        .unwrap();

        for result in [
            store.load_guest_operation(operation.id).map(|_| ()),
            store.list_guest_operations().map(|_| ()),
            store.load_artifact(cell_id, operation.id).map(|_| ()),
        ] {
            assert!(matches!(
                result,
                Err(StateError::JobCorrelationIntegrity {
                    reason: "guest operation job id does not match its cell",
                    ..
                })
            ));
        }
    }

    #[test]
    fn persisted_artifact_cell_mismatch_is_rejected_on_normal_read() {
        let directory = tempdir().unwrap();
        let store = StateStore::new(directory.path().join("state"));
        let _mutation = store.acquire_mutation_lock().unwrap();
        let owner_cell_id = CellId::new();
        let foreign_cell_id = CellId::new();
        let now = Utc::now();
        store
            .save_cell(&test_cell_record(&store, owner_cell_id))
            .unwrap();

        let mut operation =
            GuestOperationRecord::intent(owner_cell_id, GuestOperationKind::ArtifactCollect, now);
        operation.phase = GuestOperationPhase::TransportActive;
        store.save_guest_operation(&operation).unwrap();

        // Bypass save-time checks as an external same-user state writer could
        // do. The requested artifact root is internally consistent, but its
        // operation belongs to a different cell and must never be adopted.
        let foreign_guard = store
            .prepare_artifact_root(foreign_cell_id, operation.id)
            .unwrap();
        let relative = store
            .write_artifact_file(&foreign_guard, 0, b"foreign-artifact")
            .unwrap();
        let foreign_artifact = ArtifactRecord {
            schema_version: ARTIFACT_SCHEMA_VERSION,
            id: operation.id,
            cell_id: foreign_cell_id,
            created_at: now,
            entries: vec![ArtifactEntry {
                guest_path: "results/foreign-artifact.bin".to_owned(),
                host_relative_path: relative,
                sha256: format!("{:x}", Sha256::digest(b"foreign-artifact")),
                size: 16,
            }],
            job_id: None,
        };
        write_json_atomic(&foreign_guard.root.join("manifest.json"), &foreign_artifact).unwrap();

        assert!(matches!(
            store.load_artifact(foreign_cell_id, operation.id),
            Err(StateError::JobCorrelationIntegrity {
                reason: "artifact cell id does not match its guest operation",
                ..
            })
        ));
    }

    #[test]
    fn guest_operation_schema_and_filename_identity_are_gated() {
        let directory = tempdir().unwrap();
        let store = StateStore::new(directory.path().join("state"));
        drop(store.acquire_mutation_lock().unwrap());
        let cell_id = CellId::new();
        let requested_id = GuestOperationId::new();
        let mut operation =
            GuestOperationRecord::intent(cell_id, GuestOperationKind::CopyIn, Utc::now());
        let operations = store.root().join("operations");
        let operation_path = operations.join(format!("{requested_id}.json"));
        write_json_atomic(&operation_path, &operation).unwrap();
        assert!(matches!(
            store.load_guest_operation(requested_id),
            Err(StateError::IdentityMismatch {
                kind: "guest operation record",
                ..
            })
        ));

        operation.id = requested_id;
        operation.schema_version = GUEST_OPERATION_SCHEMA_VERSION;
        operation.phase = GuestOperationPhase::Completed;
        write_json_atomic(&operation_path, &operation).unwrap();
        assert!(matches!(
            store.load_guest_operation(requested_id),
            Err(StateError::GuestOperationIntegrity { .. })
        ));

        operation.schema_version = MAX_GUEST_OPERATION_SCHEMA_VERSION + 1;
        write_json_atomic(&operation_path, &operation).unwrap();
        assert!(matches!(
            store.load_guest_operation(requested_id),
            Err(StateError::UnsupportedSchema {
                kind: "guest operation record",
                ..
            })
        ));
    }

    #[test]
    fn artifact_bytes_survive_real_process_abort_only_after_atomic_rename() {
        if std::env::var_os("VMCELL_TEST_ARTIFACT_CRASH_CHILD").is_some() {
            let root = PathBuf::from(std::env::var_os("VMCELL_TEST_STATE_ROOT").unwrap());
            let cell_id =
                CellId::from_str(std::env::var("VMCELL_TEST_CELL_ID").unwrap().as_str()).unwrap();
            let operation_id = GuestOperationId::from_str(
                std::env::var("VMCELL_TEST_OPERATION_ID").unwrap().as_str(),
            )
            .unwrap();
            let store = StateStore::new(root);
            let guard = store.prepare_artifact_root(cell_id, operation_id).unwrap();
            store
                .write_artifact_file(&guard, 0, b"atomic-artifact")
                .unwrap();
            std::process::exit(77);
        }

        let directory = tempdir().unwrap();
        let store = StateStore::new(directory.path().join("state"));
        drop(store.acquire_mutation_lock().unwrap());
        let cell_id = CellId::new();
        for (checkpoint, should_exist) in [
            ("before_artifact_rename", false),
            ("after_artifact_rename", true),
        ] {
            let operation_id = GuestOperationId::new();
            let output = subprocess_for(
                "state::tests::artifact_bytes_survive_real_process_abort_only_after_atomic_rename",
            )
            .env("VMCELL_TEST_ARTIFACT_CRASH_CHILD", "1")
            .env("VMCELL_TEST_ATOMIC_CRASH_CHILD", "1")
            .env("VMCELL_TEST_ABORT_AT", checkpoint)
            .env("VMCELL_TEST_STATE_ROOT", store.root())
            .env("VMCELL_TEST_CELL_ID", cell_id.to_string())
            .env("VMCELL_TEST_OPERATION_ID", operation_id.to_string())
            .output()
            .unwrap();
            assert!(
                !output.status.success(),
                "child unexpectedly succeeded: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            assert_ne!(
                output.status.code(),
                Some(77),
                "child missed checkpoint: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            let committed = store
                .artifact_root(cell_id, operation_id)
                .join("files")
                .join("0000.bin");
            if should_exist {
                for _ in 0..100 {
                    if committed.exists() {
                        break;
                    }
                    std::thread::sleep(std::time::Duration::from_millis(10));
                }
            }
            let present = fs::read_dir(committed.parent().unwrap())
                .map(|entries| {
                    entries
                        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            assert_eq!(
                committed.exists(),
                should_exist,
                "checkpoint {checkpoint} left {present:?} under {}; child stderr: {}",
                committed.parent().unwrap().display(),
                String::from_utf8_lossy(&output.stderr)
            );
            if should_exist {
                assert_eq!(fs::read(committed).unwrap(), b"atomic-artifact");
            }
        }
    }

    #[test]
    fn artifact_prune_tombstone_survives_abort_before_exact_removal() {
        if std::env::var_os("VMCELL_TEST_ARTIFACT_PRUNE_CHILD").is_some() {
            let root = PathBuf::from(std::env::var_os("VMCELL_TEST_STATE_ROOT").unwrap());
            let cell_id =
                CellId::from_str(std::env::var("VMCELL_TEST_CELL_ID").unwrap().as_str()).unwrap();
            let operation_id = GuestOperationId::from_str(
                std::env::var("VMCELL_TEST_OPERATION_ID").unwrap().as_str(),
            )
            .unwrap();
            let store = StateStore::new(root);
            let mutation = store.acquire_mutation_lock().unwrap();
            let mut operation = store.load_guest_operation(operation_id).unwrap();
            let now = Utc::now();
            operation.updated_at = now;
            operation.artifact_pruned_at = Some(now);
            store.save_guest_operation(&operation).unwrap();
            store
                .remove_artifact_root(&mutation, cell_id, operation_id)
                .unwrap();
            std::process::exit(77);
        }

        let directory = tempdir().unwrap();
        let store = StateStore::new(directory.path().join("state"));
        let cell_id = CellId::new();
        let operation_id = GuestOperationId::new();
        let mutation = store.acquire_mutation_lock().unwrap();
        store.save_cell(&test_cell_record(&store, cell_id)).unwrap();
        drop(mutation);
        let guard = store.prepare_artifact_root(cell_id, operation_id).unwrap();
        let relative = store
            .write_artifact_file(&guard, 0, b"prune-crash")
            .unwrap();
        let now = Utc::now();
        let mut operation =
            GuestOperationRecord::intent(cell_id, GuestOperationKind::ArtifactCollect, now);
        operation.id = operation_id;
        operation.phase = GuestOperationPhase::TransportActive;
        store.save_guest_operation(&operation).unwrap();
        store
            .save_artifact_new(
                &guard,
                &ArtifactRecord {
                    schema_version: ARTIFACT_SCHEMA_VERSION,
                    id: operation_id,
                    cell_id,
                    created_at: now,
                    entries: vec![ArtifactEntry {
                        guest_path: "results/prune-crash.bin".to_owned(),
                        host_relative_path: relative,
                        sha256: format!("{:x}", Sha256::digest(b"prune-crash")),
                        size: 11,
                    }],
                    job_id: None,
                },
            )
            .unwrap();
        operation.phase = GuestOperationPhase::Completed;
        operation.updated_at = now;
        operation.completed_at = Some(now);
        operation.artifact_id = Some(operation_id);
        store.save_guest_operation(&operation).unwrap();
        drop(guard);

        let output = subprocess_for(
            "state::tests::artifact_prune_tombstone_survives_abort_before_exact_removal",
        )
        .env("VMCELL_TEST_ARTIFACT_PRUNE_CHILD", "1")
        .env("VMCELL_TEST_ATOMIC_CRASH_CHILD", "1")
        .env("VMCELL_TEST_ABORT_AT", "before_artifact_remove")
        .env("VMCELL_TEST_STATE_ROOT", store.root())
        .env("VMCELL_TEST_CELL_ID", cell_id.to_string())
        .env("VMCELL_TEST_OPERATION_ID", operation_id.to_string())
        .output()
        .unwrap();
        assert!(!output.status.success());
        assert_ne!(output.status.code(), Some(77));
        assert!(
            store
                .load_guest_operation(operation_id)
                .unwrap()
                .artifact_pruned_at
                .is_some()
        );
        assert!(store.artifact_root_exists(cell_id, operation_id).unwrap());

        // Simulate a process dying after recursive removal has deleted only part
        // of the tombstoned, operation-owned artifact subtree. Recovery must not
        // require the now-incomplete artifact manifest to validate again.
        fs::remove_file(
            store
                .artifact_root(cell_id, operation_id)
                .join("files")
                .join("0000.bin"),
        )
        .unwrap();

        let mutation = store.acquire_mutation_lock().unwrap();
        assert!(
            store
                .remove_artifact_root(&mutation, cell_id, operation_id)
                .unwrap()
        );
        assert!(!store.artifact_root_exists(cell_id, operation_id).unwrap());
    }

    #[test]
    fn guest_operation_unknown_and_artifact_boundaries_survive_process_abort() {
        if std::env::var_os("VMCELL_TEST_GUEST_OPERATION_CRASH_CHILD").is_some() {
            let root = PathBuf::from(std::env::var_os("VMCELL_TEST_STATE_ROOT").unwrap());
            let operation_id = GuestOperationId::from_str(
                std::env::var("VMCELL_TEST_OPERATION_ID").unwrap().as_str(),
            )
            .unwrap();
            let phase = std::env::var("VMCELL_TEST_GUEST_PHASE").unwrap();
            let store = StateStore::new(root);
            let mut operation = store.load_guest_operation(operation_id).unwrap();
            operation.phase = match phase.as_str() {
                "transport_active" => GuestOperationPhase::TransportActive,
                "artifact_committed" => {
                    operation.artifact_id = Some(operation.id);
                    GuestOperationPhase::ArtifactCommitted
                }
                value => panic!("unknown guest phase {value}"),
            };
            operation.updated_at = Utc::now();
            store.save_guest_operation(&operation).unwrap();
            std::process::exit(77);
        }

        let directory = tempdir().unwrap();
        let store = StateStore::new(directory.path().join("state"));
        let cell_id = CellId::new();
        let mutation = store.acquire_mutation_lock().unwrap();
        store.save_cell(&test_cell_record(&store, cell_id)).unwrap();
        drop(mutation);
        let operation =
            GuestOperationRecord::intent(cell_id, GuestOperationKind::ArtifactCollect, Utc::now());
        store.save_guest_operation(&operation).unwrap();

        for phase in ["transport_active", "artifact_committed"] {
            let baseline = store.load_guest_operation(operation.id).unwrap();
            for (checkpoint, committed) in [
                ("before_manifest_rename", false),
                ("after_manifest_rename", true),
            ] {
                let status = subprocess_for(
                    "state::tests::guest_operation_unknown_and_artifact_boundaries_survive_process_abort",
                )
                .env("VMCELL_TEST_GUEST_OPERATION_CRASH_CHILD", "1")
                .env("VMCELL_TEST_ATOMIC_CRASH_CHILD", "1")
                .env("VMCELL_TEST_ABORT_AT", checkpoint)
                .env("VMCELL_TEST_STATE_ROOT", store.root())
                .env("VMCELL_TEST_OPERATION_ID", operation.id.to_string())
                .env("VMCELL_TEST_GUEST_PHASE", phase)
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .unwrap();
                assert!(!status.success());
                assert_ne!(status.code(), Some(77));
                let persisted = store.load_guest_operation(operation.id).unwrap();
                if committed {
                    let expected = match phase {
                        "transport_active" => GuestOperationPhase::TransportActive,
                        "artifact_committed" => GuestOperationPhase::ArtifactCommitted,
                        _ => unreachable!(),
                    };
                    assert_eq!(persisted.phase, expected);
                    assert!(!persisted.phase.is_terminal());
                } else {
                    assert_eq!(persisted, baseline);
                }
            }
        }
    }

    #[test]
    fn manifest_filename_must_match_persisted_identity() {
        let directory = tempdir().unwrap();
        let store = StateStore::new(directory.path().join("state"));
        let persisted_id = ImageId::parse("persisted").unwrap();
        let requested_id = ImageId::parse("requested").unwrap();
        let record = ImageRecord {
            schema_version: IMAGE_SCHEMA_VERSION,
            id: persisted_id,
            guest_os: GuestOs::Windows,
            guest_arch: Architecture::X86_64,
            variants: Vec::new(),
            registered_at: Utc::now(),
        };
        let path = store.image_path(&requested_id);
        write_json_atomic(&path, &record).unwrap();

        assert!(matches!(
            store.load_image(&requested_id),
            Err(StateError::IdentityMismatch {
                kind: "image record",
                ..
            })
        ));
        assert!(matches!(
            store.list_images(),
            Err(StateError::IdentityMismatch {
                kind: "image record",
                ..
            })
        ));
    }

    #[test]
    fn state_manifest_reparse_is_never_followed() {
        let directory = tempdir().unwrap();
        let store = StateStore::new(directory.path().join("state"));
        let installation = store.installation().unwrap();
        let external = directory.path().join("external-installation.json");
        fs::write(&external, serde_json::to_vec(&installation).unwrap()).unwrap();
        let manifest = store.root().join("installation.json");
        fs::remove_file(&manifest).unwrap();
        if create_file_link(&external, &manifest).is_err() {
            return;
        }

        assert!(matches!(
            store.load_installation(),
            Err(StateError::UnsafeRuntimePath(_))
        ));
        assert!(matches!(
            store.acquire_installation_authority(),
            Err(StateError::UnsafeRuntimePath(_))
        ));
    }

    #[test]
    fn image_removal_rejects_manifest_reparse_without_touching_target() {
        let directory = tempdir().unwrap();
        let store = StateStore::new(directory.path().join("state"));
        let image_id = ImageId::parse("reparse-image").unwrap();
        let record = ImageRecord {
            schema_version: IMAGE_SCHEMA_VERSION,
            id: image_id.clone(),
            guest_os: GuestOs::Windows,
            guest_arch: Architecture::X86_64,
            variants: vec![ImageVariant {
                provider: "hyperv".to_owned(),
                disk_format: "vhdx".to_owned(),
                path: directory.path().join("base.vhdx"),
                sha256: "reparse-sentinel".to_owned(),
                file_size: 1,
            }],
            registered_at: Utc::now(),
        };
        store.save_image_new(&record).unwrap();
        let manifest = store.image_path(&image_id);
        fs::remove_file(&manifest).unwrap();
        let external = directory.path().join("external-image.json");
        let external_bytes = serde_json::to_vec(&record).unwrap();
        fs::write(&external, &external_bytes).unwrap();
        if create_file_link(&external, &manifest).is_err() {
            return;
        }

        let mutation = store.acquire_mutation_lock().unwrap();
        assert!(matches!(
            store.remove_image_record(&mutation, &image_id),
            Err(StateError::UnsafeRuntimePath(_))
        ));
        assert_eq!(fs::read(&external).unwrap(), external_bytes);
    }

    #[test]
    fn state_list_directory_reparse_is_never_followed() {
        let directory = tempdir().unwrap();
        let state_root = directory.path().join("state");
        let external = directory.path().join("external-images");
        fs::create_dir_all(&state_root).unwrap();
        fs::create_dir_all(&external).unwrap();
        if create_directory_link(&external, &state_root.join("images")).is_err() {
            return;
        }

        let store = StateStore::new(state_root);
        assert!(matches!(
            store.list_images(),
            Err(StateError::UnsafeRuntimePath(_))
        ));
    }

    #[test]
    fn mutation_lock_is_exclusive() {
        let directory = tempdir().unwrap();
        let store = StateStore::new(directory.path().join("state"));
        let first = store.acquire_mutation_lock().unwrap();
        assert!(matches!(
            store.acquire_mutation_lock(),
            Err(StateError::MutationBusy)
        ));
        drop(first);
        assert!(store.acquire_mutation_lock().is_ok());
    }

    #[test]
    fn mutation_lock_wait_is_bounded_and_acquires_after_release() {
        let directory = tempdir().unwrap();
        let root = directory.path().join("state");
        let first = StateStore::new(root.clone())
            .acquire_mutation_lock()
            .unwrap();
        let releaser = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(75));
            drop(first);
        });
        let start = Instant::now();
        let second = StateStore::new(root)
            .with_mutation_lock_timeout(Duration::from_secs(1))
            .acquire_mutation_lock()
            .unwrap();
        let elapsed = start.elapsed();
        assert!(elapsed >= Duration::from_millis(50));
        // The one-second lock deadline begins inside acquire_mutation_lock,
        // after filesystem identity setup. A saturated runner can delay that
        // setup and the releaser thread without weakening lock semantics.
        assert!(elapsed < Duration::from_secs(5));
        drop(second);
        releaser.join().unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn mutation_lock_leaf_reparse_is_rejected_without_touching_target() {
        let directory = tempdir().unwrap();
        let store = StateStore::new(directory.path().join("state"));
        drop(store.acquire_mutation_lock().unwrap());
        let lock_path = store.root().join("locks").join("mutation.lock");
        fs::remove_file(&lock_path).unwrap();
        let external = directory.path().join("external-lock-target");
        fs::write(&external, b"external-sentinel").unwrap();
        if create_file_link(&external, &lock_path).is_err() {
            return;
        }

        assert!(matches!(
            store.acquire_mutation_lock(),
            Err(StateError::UnsafeRuntimePath(_))
        ));
        assert_eq!(fs::read(&external).unwrap(), b"external-sentinel");
    }

    #[test]
    fn manifest_phase_transitions_survive_real_process_abort_at_atomic_boundaries() {
        if std::env::var_os("VMCELL_TEST_ATOMIC_CRASH_CHILD").is_some() {
            let root = PathBuf::from(std::env::var_os("VMCELL_TEST_STATE_ROOT").unwrap());
            let cell_id =
                CellId::from_str(std::env::var("VMCELL_TEST_CELL_ID").unwrap().as_str()).unwrap();
            let label = std::env::var("VMCELL_TEST_TARGET_PHASE").unwrap();
            let (phase, state) = phase_and_state(&label);
            let store = StateStore::new(root);
            let mut record = store.load_cell(cell_id).unwrap();
            record.phase = phase;
            record.state = state;
            record.last_error = Some(format!("vmcell.test.{label}"));
            store.save_cell(&record).unwrap();
            std::process::exit(77);
        }

        let directory = tempdir().unwrap();
        let store = StateStore::new(directory.path().join("state"));
        let cell_id = CellId::new();
        store.save_cell(&test_cell_record(&store, cell_id)).unwrap();

        for label in [
            "intent",
            "overlay",
            "provider_created",
            "provider_claimed",
            "ready",
            "destroying_provisioning",
            "destroying",
            "destroyed",
        ] {
            let baseline = store.load_cell(cell_id).unwrap();
            let before = subprocess_for(
                "state::tests::manifest_phase_transitions_survive_real_process_abort_at_atomic_boundaries",
            )
            .env("VMCELL_TEST_ATOMIC_CRASH_CHILD", "1")
            .env("VMCELL_TEST_ABORT_AT", "before_manifest_rename")
            .env("VMCELL_TEST_STATE_ROOT", store.root())
            .env("VMCELL_TEST_CELL_ID", cell_id.to_string())
            .env("VMCELL_TEST_TARGET_PHASE", label)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .unwrap();
            assert!(!before.success());
            assert_ne!(before.code(), Some(77));
            assert_eq!(store.load_cell(cell_id).unwrap(), baseline);

            let after = subprocess_for(
                "state::tests::manifest_phase_transitions_survive_real_process_abort_at_atomic_boundaries",
            )
            .env("VMCELL_TEST_ATOMIC_CRASH_CHILD", "1")
            .env("VMCELL_TEST_ABORT_AT", "after_manifest_rename")
            .env("VMCELL_TEST_STATE_ROOT", store.root())
            .env("VMCELL_TEST_CELL_ID", cell_id.to_string())
            .env("VMCELL_TEST_TARGET_PHASE", label)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .unwrap();
            assert!(!after.success());
            assert_ne!(after.code(), Some(77));
            let committed = store.load_cell(cell_id).unwrap();
            let (phase, state) = phase_and_state(label);
            assert_eq!(committed.phase, phase);
            assert_eq!(committed.state, state);
            assert_eq!(
                committed.last_error.as_deref(),
                Some(format!("vmcell.test.{label}").as_str())
            );
        }
    }

    #[cfg(windows)]
    #[test]
    fn mutation_guard_blocks_cross_process_duplicate_root_and_directory_replacement() {
        if let Some(mode) = std::env::var_os("VMCELL_TEST_MUTATION_GUARD_CHILD") {
            let root = PathBuf::from(std::env::var_os("VMCELL_TEST_STATE_ROOT").unwrap());
            let store = StateStore::new(root);
            match mode.to_string_lossy().as_ref() {
                "busy" => assert!(store.acquire_mutation_lock().is_err()),
                "wait_busy" => {
                    let start = Instant::now();
                    assert!(matches!(
                        store
                            .with_mutation_lock_timeout(Duration::from_millis(100))
                            .acquire_mutation_lock(),
                        Err(StateError::MutationBusy)
                    ));
                    assert!(start.elapsed() >= Duration::from_millis(75));
                    assert!(start.elapsed() < Duration::from_secs(2));
                }
                "available" => drop(store.acquire_mutation_lock().unwrap()),
                "rename" => {
                    let moved = store.root().join("cells-moved-by-child");
                    assert!(fs::rename(store.root().join("cells"), moved).is_err());
                }
                value => panic!("unknown child mode {value}"),
            }
            return;
        }

        let directory = tempdir().unwrap();
        let store = StateStore::new(directory.path().join("state"));
        let guard = store.acquire_mutation_lock().unwrap();
        for mode in ["busy", "wait_busy", "rename"] {
            let output = subprocess_for(
                "state::tests::mutation_guard_blocks_cross_process_duplicate_root_and_directory_replacement",
            )
            .env("VMCELL_TEST_MUTATION_GUARD_CHILD", mode)
            .env("VMCELL_TEST_STATE_ROOT", store.root())
            .output()
            .unwrap();
            assert!(
                output.status.success(),
                "child mode {mode} failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
        drop(guard);

        let status = subprocess_for(
            "state::tests::mutation_guard_blocks_cross_process_duplicate_root_and_directory_replacement",
        )
        .env("VMCELL_TEST_MUTATION_GUARD_CHILD", "available")
        .env("VMCELL_TEST_STATE_ROOT", store.root())
        .status()
        .unwrap();
        assert!(status.success());
    }

    #[cfg(windows)]
    #[test]
    fn mutation_guard_pins_state_subdirectories_against_replacement() {
        let directory = tempdir().unwrap();
        let store = StateStore::new(directory.path().join("state"));
        let guard = store.acquire_mutation_lock().unwrap();
        for name in [
            "locks",
            "images",
            "cells",
            "runtime",
            "operations",
            "artifacts",
        ] {
            let source = store.root().join(name);
            let moved = store.root().join(format!("{name}-moved"));
            assert!(
                fs::rename(&source, &moved).is_err(),
                "{name} was replaceable"
            );
            assert!(source.is_dir());
        }

        drop(guard);
        let cells = store.root().join("cells");
        let moved = store.root().join("cells-moved");
        fs::rename(&cells, &moved).unwrap();
        assert!(moved.is_dir());
    }

    #[test]
    fn runtime_removal_is_scoped_to_one_cell() {
        let directory = tempdir().unwrap();
        let store = StateStore::new(directory.path().join("state"));
        let first = CellId::new();
        let second = CellId::new();
        fs::write(
            store.ensure_cell_runtime(first).unwrap().join("owned"),
            b"x",
        )
        .unwrap();
        fs::write(
            store.ensure_cell_runtime(second).unwrap().join("other"),
            b"y",
        )
        .unwrap();

        let guard = store.pin_cell_runtime(first).unwrap();
        store.remove_cell_runtime(first, guard).unwrap();

        assert!(!store.cell_runtime_root(first).exists());
        assert!(store.cell_runtime_root(second).exists());
    }

    #[test]
    fn unsupported_persisted_schemas_are_rejected() {
        let directory = tempdir().unwrap();
        let store = StateStore::new(directory.path().join("state"));
        let mut installation = store.installation().unwrap();
        installation.schema_version += 1;
        fs::write(
            store.root().join("installation.json"),
            serde_json::to_vec(&installation).unwrap(),
        )
        .unwrap();
        assert!(matches!(
            store.load_installation(),
            Err(StateError::UnsupportedSchema {
                kind: "installation record",
                ..
            })
        ));

        let image_id = ImageId::parse("unsupported-image").unwrap();
        let image_path = store.image_path(&image_id);
        let image = ImageRecord {
            schema_version: IMAGE_SCHEMA_VERSION + 1,
            id: image_id.clone(),
            guest_os: GuestOs::Windows,
            guest_arch: Architecture::X86_64,
            variants: Vec::new(),
            registered_at: Utc::now(),
        };
        write_json_atomic(&image_path, &image).unwrap();
        assert!(matches!(
            store.load_image(&image_id),
            Err(StateError::UnsupportedSchema {
                kind: "image record",
                ..
            })
        ));

        let cell_id = CellId::new();
        let cell_path = store.cell_path(cell_id);
        let ownership = CellOwnership::new(
            Uuid::new_v4(),
            cell_id,
            Uuid::new_v4(),
            store.cell_configuration_path(cell_id),
            store.cell_overlay_path(cell_id),
        );
        let mut cell = CellRecord {
            schema_version: MAX_CELL_SCHEMA_VERSION + 1,
            id: cell_id,
            provider: "hyperv".to_owned(),
            spec: CellSpec {
                image: image_id.clone(),
                provider: Some("hyperv".to_owned()),
                cpu_count: 2,
                memory_mib: 4096,
                ttl_seconds: None,
                accelerator: None,
                allow_tcg: false,
            },
            image: ImageBinding {
                image_id,
                guest_os: Some(GuestOs::Windows),
                provider: "hyperv".to_owned(),
                disk_format: "vhdx".to_owned(),
                path: directory.path().join("base.vhdx"),
                sha256: "abc".to_owned(),
                file_size: 42,
            },
            ownership,
            provider_object: None,
            state: CellState::Creating,
            phase: CellPhase::IntentRecorded,
            created_at: Utc::now(),
            updated_at: Utc::now(),
            expires_at: None,
            last_error: None,
            job: None,
        };
        write_json_atomic(&cell_path, &cell).unwrap();
        assert!(matches!(
            store.load_cell(cell_id),
            Err(StateError::UnsupportedSchema {
                kind: "cell record",
                ..
            })
        ));

        cell.schema_version = CELL_SCHEMA_VERSION;
        cell.ownership.schema_version = OWNERSHIP_MARKER_SCHEMA + 1;
        write_json_atomic(&cell_path, &cell).unwrap();
        assert!(matches!(
            store.load_cell(cell_id),
            Err(StateError::UnsupportedSchema {
                kind: "cell ownership",
                ..
            })
        ));
    }

    #[test]
    fn runtime_reparse_ancestor_never_creates_or_deletes_outside_state_root() {
        let directory = tempdir().unwrap();
        let state_root = directory.path().join("state");
        let external = directory.path().join("external");
        fs::create_dir_all(&state_root).unwrap();
        fs::create_dir_all(&external).unwrap();
        let runtime = state_root.join("runtime");
        if create_directory_link(&external, &runtime).is_err() {
            // Windows may require Developer Mode or symlink privilege.
            return;
        }

        let store = StateStore::new(state_root);
        let cell_id = CellId::new();
        assert!(matches!(
            store.ensure_cell_runtime(cell_id),
            Err(StateError::UnsafeRuntimePath(_))
        ));
        assert!(!external.join(cell_id.0.to_string()).exists());

        let external_cell = external.join(cell_id.0.to_string());
        fs::create_dir_all(&external_cell).unwrap();
        fs::write(external_cell.join("foreign"), b"preserve").unwrap();
        assert!(matches!(
            store.pin_cell_runtime(cell_id),
            Err(StateError::UnsafeRuntimePath(_))
        ));
        assert_eq!(
            fs::read(external_cell.join("foreign")).unwrap(),
            b"preserve"
        );
    }

    #[cfg(windows)]
    fn create_directory_link(target: &Path, link: &Path) -> std::io::Result<()> {
        std::os::windows::fs::symlink_dir(target, link)
    }

    #[cfg(not(windows))]
    fn create_directory_link(target: &Path, link: &Path) -> std::io::Result<()> {
        std::os::unix::fs::symlink(target, link)
    }

    #[cfg(windows)]
    fn create_file_link(target: &Path, link: &Path) -> std::io::Result<()> {
        std::os::windows::fs::symlink_file(target, link)
    }

    #[cfg(not(windows))]
    fn create_file_link(target: &Path, link: &Path) -> std::io::Result<()> {
        std::os::unix::fs::symlink(target, link)
    }
}
