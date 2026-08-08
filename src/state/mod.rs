use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use directories::ProjectDirs;
use fs2::FileExt;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

use crate::core::cell::{CELL_SCHEMA_VERSION, CellId, CellRecord};
use crate::core::image::{IMAGE_SCHEMA_VERSION, ImageId, ImageRecord};
use crate::core::ownership::OWNERSHIP_MARKER_SCHEMA;

pub const INSTALL_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone)]
pub struct StateStore {
    root: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstallationRecord {
    pub schema_version: u32,
    pub install_id: Uuid,
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
}

pub struct MutationGuard {
    file: File,
    _state_root: File,
    _state_directories: Vec<File>,
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
    _state_handle: File,
    _runtime_handle: File,
    cell_handle: Option<File>,
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
}

impl Drop for MutationGuard {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.file);
        if let Ok(mut roots) = process_mutation_roots().lock() {
            roots.remove(&self.process_key);
        }
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
        Self { root }
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

    pub(crate) fn acquire_mutation_lock(&self) -> Result<MutationGuard, StateError> {
        ensure_directory(&self.root)?;
        let state_root_handle = open_ordinary_directory(&self.root)?;
        let mut state_directories = Vec::new();
        for name in ["locks", "images", "cells", "runtime"] {
            let directory = self.root.join(name);
            create_direct_child_directory(&directory)?;
            state_directories.push(open_ordinary_directory(&directory)?);
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
        let file = match OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .open(&lock_path)
        {
            Ok(file) => file,
            Err(source) => {
                release_process_mutation_root(&process_key);
                return Err(io_error(&lock_path, source));
            }
        };

        if let Err(source) = file.try_lock_exclusive() {
            release_process_mutation_root(&process_key);
            if source.kind() == std::io::ErrorKind::WouldBlock {
                return Err(StateError::MutationBusy);
            } else {
                return Err(io_error(&lock_path, source));
            }
        }

        Ok(MutationGuard {
            file,
            _state_root: state_root_handle,
            _state_directories: state_directories,
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

    pub(crate) fn save_cell(&self, record: &CellRecord) -> Result<(), StateError> {
        let path = self.cell_path(record.id);
        validate_cell_schema(&path, record)?;
        write_json_atomic(&path, record)
    }

    pub fn load_cell(&self, cell_id: CellId) -> Result<CellRecord, StateError> {
        let path = self.cell_path(cell_id);
        let record = read_json(&path)?;
        validate_cell_schema(&path, &record)?;
        if record.id != cell_id {
            return Err(StateError::IdentityMismatch {
                kind: "cell record",
                path,
                expected: cell_id.to_string(),
            });
        }
        Ok(record)
    }

    pub fn list_cells(&self) -> Result<Vec<CellRecord>, StateError> {
        read_json_directory(&self.root.join("cells"), validate_cell_schema)
    }

    #[must_use]
    pub fn cell_runtime_root(&self, cell_id: CellId) -> PathBuf {
        self.root.join("runtime").join(cell_id.0.to_string())
    }

    #[must_use]
    pub fn cell_overlay_path(&self, cell_id: CellId) -> PathBuf {
        self.cell_runtime_root(cell_id).join("cell.vhdx")
    }

    #[must_use]
    pub fn cell_configuration_path(&self, cell_id: CellId) -> PathBuf {
        self.cell_runtime_root(cell_id).join("hyperv")
    }

    #[cfg(test)]
    pub(crate) fn ensure_cell_runtime(&self, cell_id: CellId) -> Result<PathBuf, StateError> {
        let guard = self.prepare_cell_runtime(cell_id)?;
        Ok(guard.cell_root.clone())
    }

    pub(crate) fn prepare_cell_runtime(
        &self,
        cell_id: CellId,
    ) -> Result<CellRuntimeGuard, StateError> {
        ensure_directory(&self.root)?;
        let state_handle = open_ordinary_directory(&self.root)?;
        let runtime_root = self.root.join("runtime");
        create_direct_child_directory(&runtime_root)?;
        let runtime_handle = open_ordinary_directory(&runtime_root)?;
        let cell_root = self.cell_runtime_root(cell_id);
        create_direct_child_directory(&cell_root)?;
        let cell_handle = open_ordinary_directory(&cell_root)?;
        validate_runtime_chain(&self.root, &cell_root)?;
        ensure_no_reparse_tree(&cell_root)?;
        Ok(CellRuntimeGuard {
            cell_id,
            state_root: self.root.clone(),
            runtime_root,
            configuration_path: self.cell_configuration_path(cell_id),
            overlay_path: self.cell_overlay_path(cell_id),
            cell_root,
            _state_handle: state_handle,
            _runtime_handle: runtime_handle,
            cell_handle: Some(cell_handle),
        })
    }

    pub(crate) fn pin_cell_runtime(&self, cell_id: CellId) -> Result<CellRuntimeGuard, StateError> {
        let state_handle = open_ordinary_directory(&self.root)?;
        let runtime_root = self.root.join("runtime");
        let runtime_handle = open_ordinary_directory(&runtime_root)?;
        let cell_root = self.cell_runtime_root(cell_id);
        let cell_handle = open_ordinary_directory(&cell_root)?;
        validate_runtime_chain(&self.root, &cell_root)?;
        ensure_no_reparse_tree(&cell_root)?;
        Ok(CellRuntimeGuard {
            cell_id,
            state_root: self.root.clone(),
            runtime_root,
            configuration_path: self.cell_configuration_path(cell_id),
            overlay_path: self.cell_overlay_path(cell_id),
            cell_root,
            _state_handle: state_handle,
            _runtime_handle: runtime_handle,
            cell_handle: Some(cell_handle),
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
        if !cell_root.exists() {
            return Ok(());
        }

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
        drop(guard.cell_handle.take());

        // Rust's Windows implementation performs handle-relative recursive
        // removal and does not follow a child swapped to a reparse point. The
        // pinned runtime-root handle prevents an ancestor swap while it runs.
        fs::remove_dir_all(&physical_cell_root)
            .map_err(|source| io_error(&physical_cell_root, source))
    }

    fn image_path(&self, image_id: &ImageId) -> PathBuf {
        self.root
            .join("images")
            .join(format!("{}.json", image_id.as_str()))
    }

    fn cell_path(&self, cell_id: CellId) -> PathBuf {
        self.root.join("cells").join(format!("{}.json", cell_id.0))
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
    let mut options = OpenOptions::new();
    options.read(true);
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
    ensure_existing_ancestors_are_ordinary(path)?;
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
    fs::create_dir_all(path).map_err(|source| io_error(path, source))?;
    ensure_existing_ancestors_are_ordinary(path)
}

fn create_direct_child_directory(path: &Path) -> Result<(), StateError> {
    match fs::create_dir(path) {
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
    }
}

fn open_ordinary_directory(path: &Path) -> Result<File, StateError> {
    ensure_existing_ancestors_are_ordinary(path)?;
    let mut options = OpenOptions::new();
    options.read(true);
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
    ensure_existing_ancestors_are_ordinary(path)?;
    Ok(file)
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

fn validate_cell_schema(path: &Path, record: &CellRecord) -> Result<(), StateError> {
    ensure_schema(
        path,
        "cell record",
        record.schema_version,
        CELL_SCHEMA_VERSION,
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
    Ok(())
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
    use crate::core::image::{Architecture, GuestOs, ImageBinding, ImageVariant};
    use crate::core::ownership::CellOwnership;

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
            },
            image: ImageBinding::from_variant(image_id, &variant),
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
            last_error: Some("baseline".to_owned()),
        }
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
            },
            image: ImageBinding::from_variant(image_id, &image.variants[0]),
            ownership,
            provider_object: None,
            state: CellState::Creating,
            phase: CellPhase::IntentRecorded,
            created_at: now,
            updated_at: now,
            expires_at: None,
            last_error: None,
        };
        store.save_cell(&cell).unwrap();
        assert_eq!(store.load_cell(cell_id).unwrap().id, cell_id);

        cell.last_error = Some("updated atomically".to_owned());
        store.save_cell(&cell).unwrap();
        assert_eq!(
            store.load_cell(cell_id).unwrap().last_error.as_deref(),
            Some("updated atomically")
        );
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
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, serde_json::to_vec(&record).unwrap()).unwrap();

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
            record.last_error = Some(label);
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
            assert_eq!(committed.last_error.as_deref(), Some(label));
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
        for mode in ["busy", "rename"] {
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
        let cells = store.root().join("cells");
        let moved = store.root().join("cells-moved");

        assert!(fs::rename(&cells, &moved).is_err());
        assert!(cells.is_dir());

        drop(guard);
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
        fs::create_dir_all(image_path.parent().unwrap()).unwrap();
        let image = ImageRecord {
            schema_version: IMAGE_SCHEMA_VERSION + 1,
            id: image_id.clone(),
            guest_os: GuestOs::Windows,
            guest_arch: Architecture::X86_64,
            variants: Vec::new(),
            registered_at: Utc::now(),
        };
        fs::write(&image_path, serde_json::to_vec(&image).unwrap()).unwrap();
        assert!(matches!(
            store.load_image(&image_id),
            Err(StateError::UnsupportedSchema {
                kind: "image record",
                ..
            })
        ));

        let cell_id = CellId::new();
        let cell_path = store.cell_path(cell_id);
        fs::create_dir_all(cell_path.parent().unwrap()).unwrap();
        let ownership = CellOwnership::new(
            Uuid::new_v4(),
            cell_id,
            Uuid::new_v4(),
            store.cell_configuration_path(cell_id),
            store.cell_overlay_path(cell_id),
        );
        let mut cell = CellRecord {
            schema_version: CELL_SCHEMA_VERSION + 1,
            id: cell_id,
            provider: "hyperv".to_owned(),
            spec: CellSpec {
                image: image_id.clone(),
                provider: Some("hyperv".to_owned()),
                cpu_count: 2,
                memory_mib: 4096,
                ttl_seconds: None,
            },
            image: ImageBinding {
                image_id,
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
        };
        fs::write(&cell_path, serde_json::to_vec(&cell).unwrap()).unwrap();
        assert!(matches!(
            store.load_cell(cell_id),
            Err(StateError::UnsupportedSchema {
                kind: "cell record",
                ..
            })
        ));

        cell.schema_version = CELL_SCHEMA_VERSION;
        cell.ownership.schema_version = OWNERSHIP_MARKER_SCHEMA + 1;
        fs::write(&cell_path, serde_json::to_vec(&cell).unwrap()).unwrap();
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
