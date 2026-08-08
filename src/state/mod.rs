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

use crate::core::cell::{CellId, CellRecord};
use crate::core::image::{ImageId, ImageRecord};

const INSTALL_SCHEMA_VERSION: u32 = 1;

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
}

pub struct MutationGuard {
    file: File,
    process_key: PathBuf,
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

    pub fn acquire_mutation_lock(&self) -> Result<MutationGuard, StateError> {
        let lock_dir = self.root.join("locks");
        create_dir_all(&lock_dir)?;
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

        Ok(MutationGuard { file, process_key })
    }

    pub fn installation(&self) -> Result<InstallationRecord, StateError> {
        let path = self.root.join("installation.json");
        if path.exists() {
            return read_json(&path);
        }

        let record = InstallationRecord {
            schema_version: INSTALL_SCHEMA_VERSION,
            install_id: Uuid::new_v4(),
        };
        write_json_new(&path, &record)?;
        Ok(record)
    }

    pub fn save_image_new(&self, record: &ImageRecord) -> Result<(), StateError> {
        write_json_new(&self.image_path(&record.id), record)
    }

    pub fn load_image(&self, image_id: &ImageId) -> Result<ImageRecord, StateError> {
        read_json(&self.image_path(image_id))
    }

    pub fn list_images(&self) -> Result<Vec<ImageRecord>, StateError> {
        read_json_directory(&self.root.join("images"))
    }

    pub fn save_cell(&self, record: &CellRecord) -> Result<(), StateError> {
        write_json_atomic(&self.cell_path(record.id), record)
    }

    pub fn load_cell(&self, cell_id: CellId) -> Result<CellRecord, StateError> {
        read_json(&self.cell_path(cell_id))
    }

    pub fn list_cells(&self) -> Result<Vec<CellRecord>, StateError> {
        read_json_directory(&self.root.join("cells"))
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

    pub fn ensure_cell_runtime(&self, cell_id: CellId) -> Result<PathBuf, StateError> {
        let path = self.cell_runtime_root(cell_id);
        create_dir_all(&path)?;
        Ok(path)
    }

    pub fn remove_cell_runtime(&self, cell_id: CellId) -> Result<(), StateError> {
        let runtime_root = self.root.join("runtime");
        let cell_root = self.cell_runtime_root(cell_id);
        if !cell_root.exists() {
            return Ok(());
        }

        let runtime_root = runtime_root
            .canonicalize()
            .map_err(|source| io_error(&runtime_root, source))?;
        let cell_root = cell_root
            .canonicalize()
            .map_err(|source| io_error(&cell_root, source))?;

        if cell_root.parent() != Some(runtime_root.as_path()) || is_reparse_point(&cell_root)? {
            return Err(StateError::UnsafeRuntimePath(cell_root));
        }

        fs::remove_dir_all(&cell_root).map_err(|source| io_error(&cell_root, source))
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

fn read_json_directory<T: DeserializeOwned>(directory: &Path) -> Result<Vec<T>, StateError> {
    if !directory.exists() {
        return Ok(Vec::new());
    }

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
    paths.into_iter().map(|path| read_json(&path)).collect()
}

fn read_json<T: DeserializeOwned>(path: &Path) -> Result<T, StateError> {
    if !path.exists() {
        return Err(StateError::NotFound(path.to_path_buf()));
    }

    let mut file = File::open(path).map_err(|source| io_error(path, source))?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)
        .map_err(|source| io_error(path, source))?;
    serde_json::from_slice(&bytes).map_err(|source| StateError::Json {
        path: path.to_path_buf(),
        source,
    })
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
    create_dir_all(parent)?;

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
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temporary)
            .map_err(|source| io_error(&temporary, source))?;
        file.write_all(&bytes)
            .map_err(|source| io_error(&temporary, source))?;
        file.sync_all()
            .map_err(|source| io_error(&temporary, source))?;
        drop(file);
        fs::rename(&temporary, path).map_err(|source| io_error(path, source))
    })();

    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

fn create_dir_all(path: &Path) -> Result<(), StateError> {
    fs::create_dir_all(path).map_err(|source| io_error(path, source))
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
    use chrono::Utc;
    use tempfile::tempdir;

    use super::*;
    use crate::core::cell::{CellPhase, CellSpec, CellState};
    use crate::core::image::{Architecture, GuestOs, ImageBinding, ImageVariant};
    use crate::core::ownership::CellOwnership;

    #[test]
    fn installation_identity_is_durable() {
        let directory = tempdir().unwrap();
        let store = StateStore::new(directory.path().join("state"));

        let first = store.installation().unwrap();
        let second = store.installation().unwrap();

        assert_eq!(first, second);
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

        store.remove_cell_runtime(first).unwrap();

        assert!(!store.cell_runtime_root(first).exists());
        assert!(store.cell_runtime_root(second).exists());
    }
}
