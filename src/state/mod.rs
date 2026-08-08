use std::path::PathBuf;

use directories::ProjectDirs;

pub struct StateStore;

impl StateStore {
    #[must_use]
    pub fn default_root() -> PathBuf {
        ProjectDirs::from("dev", "vmcell", "VM Cell Manager")
            .map(|dirs| dirs.data_local_dir().to_path_buf())
            .unwrap_or_else(|| PathBuf::from(".vmcell"))
    }
}
