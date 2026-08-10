use std::fs::{self, File, OpenOptions};
use std::io::Read;
use std::path::{Component, Path, PathBuf};

use directories::ProjectDirs;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::core::cell::{MAX_CPU_COUNT, MAX_MEMORY_MIB, MIN_MEMORY_MIB};
use crate::guest::{DEFAULT_ACTION_TIMEOUT_SECONDS, DEFAULT_READINESS_TIMEOUT_SECONDS};

pub const CONFIG_SCHEMA_VERSION: u32 = 1;
const MAX_CONFIG_BYTES: u64 = 64 * 1024;
const DEFAULT_CPU_COUNT: u16 = 2;
const DEFAULT_MEMORY_MIB: u64 = 4096;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConfigProvider {
    Hyperv,
    Qemu,
}

impl ConfigProvider {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Hyperv => "hyperv",
            Self::Qemu => "qemu",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HumanOutputPreference {
    Normal,
    Quiet,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UserConfig {
    pub schema_version: u32,
    #[serde(default)]
    pub defaults: UserDefaults,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct UserDefaults {
    pub state_root: Option<PathBuf>,
    pub provider: Option<ConfigProvider>,
    pub cpu_count: Option<u16>,
    pub memory_mib: Option<u64>,
    pub lock_timeout_ms: Option<u64>,
    pub readiness_timeout_seconds: Option<u64>,
    pub action_timeout_seconds: Option<u64>,
    pub human_output: Option<HumanOutputPreference>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedConfig {
    pub state_root: Option<PathBuf>,
    pub provider: ConfigProvider,
    pub provider_preference: Option<ConfigProvider>,
    pub cpu_count: u16,
    pub memory_mib: u64,
    pub lock_timeout_ms: u64,
    pub readiness_timeout_seconds: u64,
    pub action_timeout_seconds: u64,
    pub human_output: HumanOutputPreference,
}

impl Default for ResolvedConfig {
    fn default() -> Self {
        Self {
            state_root: None,
            provider: ConfigProvider::Hyperv,
            provider_preference: None,
            cpu_count: DEFAULT_CPU_COUNT,
            memory_mib: DEFAULT_MEMORY_MIB,
            lock_timeout_ms: 0,
            readiness_timeout_seconds: DEFAULT_READINESS_TIMEOUT_SECONDS,
            action_timeout_seconds: DEFAULT_ACTION_TIMEOUT_SECONDS,
            human_output: HumanOutputPreference::Normal,
        }
    }
}

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("configuration file was not found")]
    NotFound,
    #[error("configuration path is not an ordinary private file")]
    UnsafePath,
    #[error("configuration exceeds the bounded size limit")]
    TooLarge,
    #[error("configuration is not valid JSON: {0}")]
    Json(#[source] serde_json::Error),
    #[error("unsupported configuration schema version {actual}; expected {expected}")]
    UnsupportedSchema { expected: u32, actual: u32 },
    #[error("configuration default is outside the supported policy: {0}")]
    InvalidValue(&'static str),
    #[error("configuration I/O failed: {0}")]
    Io(#[source] std::io::Error),
}

#[must_use]
pub fn default_config_path() -> Option<PathBuf> {
    ProjectDirs::from("dev", "vmcell", "VM Cell Manager")
        .map(|dirs| dirs.config_dir().join("config.json"))
}

pub fn load_config(explicit_path: Option<&Path>) -> Result<ResolvedConfig, ConfigError> {
    let (path, required) = match explicit_path {
        Some(path) => (path.to_path_buf(), true),
        None => {
            let Some(path) = default_config_path() else {
                return Ok(ResolvedConfig::default());
            };
            (path, false)
        }
    };
    let path = absolute_existing_config_path(&path, required)?;
    let Some(path) = path else {
        return Ok(ResolvedConfig::default());
    };
    let mut file = open_config_file(&path)?;
    let mut bytes = Vec::new();
    file.by_ref()
        .take(MAX_CONFIG_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(ConfigError::Io)?;
    if bytes.len() as u64 > MAX_CONFIG_BYTES {
        return Err(ConfigError::TooLarge);
    }
    let config: UserConfig = serde_json::from_slice(&bytes).map_err(ConfigError::Json)?;
    if config.schema_version != CONFIG_SCHEMA_VERSION {
        return Err(ConfigError::UnsupportedSchema {
            expected: CONFIG_SCHEMA_VERSION,
            actual: config.schema_version,
        });
    }
    resolve_config(config.defaults, &path)
}

fn resolve_config(
    defaults: UserDefaults,
    config_path: &Path,
) -> Result<ResolvedConfig, ConfigError> {
    let mut resolved = ResolvedConfig::default();
    if let Some(state_root) = defaults.state_root {
        if state_root
            .components()
            .any(|component| matches!(component, Component::CurDir | Component::ParentDir))
        {
            return Err(ConfigError::InvalidValue(
                "state_root must not contain dot segments",
            ));
        }
        let state_root = if state_root.is_absolute() {
            state_root
        } else {
            config_path
                .parent()
                .ok_or(ConfigError::UnsafePath)?
                .join(state_root)
        };
        resolved.state_root = Some(state_root);
    }
    if let Some(provider) = defaults.provider {
        resolved.provider = provider;
        resolved.provider_preference = Some(provider);
    }
    if let Some(cpu_count) = defaults.cpu_count {
        if !(1..=MAX_CPU_COUNT).contains(&cpu_count) {
            return Err(ConfigError::InvalidValue(
                "cpu_count is outside the engine-supported range",
            ));
        }
        resolved.cpu_count = cpu_count;
    }
    if let Some(memory_mib) = defaults.memory_mib {
        if !(MIN_MEMORY_MIB..=MAX_MEMORY_MIB).contains(&memory_mib) {
            return Err(ConfigError::InvalidValue(
                "memory_mib is outside the engine-supported range",
            ));
        }
        resolved.memory_mib = memory_mib;
    }
    if let Some(lock_timeout_ms) = defaults.lock_timeout_ms {
        if lock_timeout_ms > 30_000 {
            return Err(ConfigError::InvalidValue(
                "lock_timeout_ms must be between 0 and 30000",
            ));
        }
        resolved.lock_timeout_ms = lock_timeout_ms;
    }
    if let Some(timeout) = defaults.readiness_timeout_seconds {
        if !(1..=3_600).contains(&timeout) {
            return Err(ConfigError::InvalidValue(
                "readiness_timeout_seconds must be between 1 and 3600",
            ));
        }
        resolved.readiness_timeout_seconds = timeout;
    }
    if let Some(timeout) = defaults.action_timeout_seconds {
        if !(1..=3_600).contains(&timeout) {
            return Err(ConfigError::InvalidValue(
                "action_timeout_seconds must be between 1 and 3600",
            ));
        }
        resolved.action_timeout_seconds = timeout;
    }
    if let Some(human_output) = defaults.human_output {
        resolved.human_output = human_output;
    }
    Ok(resolved)
}

fn absolute_existing_config_path(
    path: &Path,
    required: bool,
) -> Result<Option<PathBuf>, ConfigError> {
    let path = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir().map_err(ConfigError::Io)?.join(path)
    };
    match fs::symlink_metadata(&path) {
        Ok(_) => {
            for ancestor in path.ancestors() {
                if ancestor.as_os_str().is_empty() || !ancestor.exists() {
                    continue;
                }
                if path_is_reparse(ancestor)? {
                    return Err(ConfigError::UnsafePath);
                }
            }
            path.canonicalize().map(Some).map_err(ConfigError::Io)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound && !required => Ok(None),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Err(ConfigError::NotFound),
        Err(error) => Err(ConfigError::Io(error)),
    }
}

fn open_config_file(path: &Path) -> Result<File, ConfigError> {
    for ancestor in path.ancestors() {
        if ancestor.as_os_str().is_empty() || !ancestor.exists() {
            continue;
        }
        if path_is_reparse(ancestor)? {
            return Err(ConfigError::UnsafePath);
        }
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
        options
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
            .share_mode(FILE_SHARE_READ);
    }
    let file = options.open(path).map_err(ConfigError::Io)?;
    let metadata = file.metadata().map_err(ConfigError::Io)?;
    if !metadata.is_file() || path_is_reparse(path)? {
        return Err(ConfigError::UnsafePath);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;

        if metadata.uid() != unsafe { libc::geteuid() } || metadata.mode() & 0o077 != 0 {
            return Err(ConfigError::UnsafePath);
        }
    }
    Ok(file)
}

#[cfg(windows)]
fn path_is_reparse(path: &Path) -> Result<bool, ConfigError> {
    use std::os::windows::fs::MetadataExt;

    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
    let metadata = fs::symlink_metadata(path).map_err(ConfigError::Io)?;
    Ok(metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0)
}

#[cfg(not(windows))]
fn path_is_reparse(path: &Path) -> Result<bool, ConfigError> {
    let metadata = fs::symlink_metadata(path).map_err(ConfigError::Io)?;
    Ok(metadata.file_type().is_symlink())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_is_versioned_bounded_and_rejects_authority_like_fields() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("config.json");
        fs::write(
            &path,
            r#"{"schema_version":1,"defaults":{"provider":"qemu","cpu_count":4,"memory_mib":8192,"state_root":"state","lock_timeout_ms":250,"readiness_timeout_seconds":30,"action_timeout_seconds":90,"human_output":"quiet"}}"#,
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        }
        let resolved = load_config(Some(&path)).unwrap();
        assert_eq!(resolved.provider, ConfigProvider::Qemu);
        assert_eq!(resolved.provider_preference, Some(ConfigProvider::Qemu));
        assert_eq!(resolved.cpu_count, 4);
        assert_eq!(resolved.memory_mib, 8192);
        assert_eq!(resolved.lock_timeout_ms, 250);
        assert_eq!(resolved.readiness_timeout_seconds, 30);
        assert_eq!(resolved.action_timeout_seconds, 90);
        assert_eq!(resolved.human_output, HumanOutputPreference::Quiet);
        assert_eq!(
            resolved.state_root,
            Some(directory.path().canonicalize().unwrap().join("state"))
        );

        for forbidden in [
            r#"{"schema_version":1,"defaults":{"password":"secret"}}"#,
            r#"{"schema_version":1,"defaults":{"allow_tcg":true}}"#,
            r#"{"schema_version":1,"defaults":{"accelerator":"tcg"}}"#,
        ] {
            fs::write(&path, forbidden).unwrap();
            assert!(matches!(
                load_config(Some(&path)),
                Err(ConfigError::Json(_))
            ));
        }
        fs::write(&path, r#"{"schema_version":2,"defaults":{}}"#).unwrap();
        assert!(matches!(
            load_config(Some(&path)),
            Err(ConfigError::UnsupportedSchema { actual: 2, .. })
        ));
    }

    #[test]
    fn absent_implicit_config_uses_non_authorizing_defaults() {
        let resolved = ResolvedConfig::default();
        assert_eq!(resolved.provider, ConfigProvider::Hyperv);
        assert_eq!(resolved.provider_preference, None);
        assert_eq!(resolved.cpu_count, DEFAULT_CPU_COUNT);
        assert_eq!(resolved.memory_mib, DEFAULT_MEMORY_MIB);
        assert_eq!(resolved.lock_timeout_ms, 0);
    }

    #[test]
    fn explicit_config_is_required_bounded_and_rejects_dot_segments() {
        let directory = tempfile::tempdir().unwrap();
        let missing = directory.path().join("missing.json");
        assert!(matches!(
            load_config(Some(&missing)),
            Err(ConfigError::NotFound)
        ));

        let path = directory.path().join("config.json");
        fs::write(&path, vec![b' '; MAX_CONFIG_BYTES as usize + 1]).unwrap();
        set_private_test_permissions(&path);
        assert!(matches!(
            load_config(Some(&path)),
            Err(ConfigError::TooLarge)
        ));

        fs::write(
            &path,
            r#"{"schema_version":1,"defaults":{"state_root":"state/../foreign"}}"#,
        )
        .unwrap();
        assert!(matches!(
            load_config(Some(&path)),
            Err(ConfigError::InvalidValue(_))
        ));

        for unsupported in [
            r#"{"schema_version":1,"defaults":{"cpu_count":65}}"#,
            r#"{"schema_version":1,"defaults":{"memory_mib":511}}"#,
        ] {
            fs::write(&path, unsupported).unwrap();
            assert!(matches!(
                load_config(Some(&path)),
                Err(ConfigError::InvalidValue(_))
            ));
        }
    }

    #[cfg(unix)]
    #[test]
    fn unix_config_must_be_private_and_not_a_symlink() {
        use std::os::unix::fs::{PermissionsExt, symlink};

        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("config.json");
        fs::write(&path, r#"{"schema_version":1,"defaults":{}}"#).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        assert!(matches!(
            load_config(Some(&path)),
            Err(ConfigError::UnsafePath)
        ));

        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        let link = directory.path().join("linked.json");
        symlink(&path, &link).unwrap();
        assert!(matches!(
            load_config(Some(&link)),
            Err(ConfigError::UnsafePath)
        ));
    }

    #[cfg(windows)]
    #[test]
    fn windows_config_leaf_reparse_is_rejected_when_links_are_available() {
        use std::os::windows::fs::symlink_file;

        let directory = tempfile::tempdir().unwrap();
        let target = directory.path().join("config.json");
        fs::write(&target, r#"{"schema_version":1,"defaults":{}}"#).unwrap();
        let link = directory.path().join("linked.json");
        if symlink_file(&target, &link).is_err() {
            return;
        }
        assert!(matches!(
            load_config(Some(&link)),
            Err(ConfigError::UnsafePath)
        ));
    }

    fn set_private_test_permissions(_path: &Path) {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(_path, fs::Permissions::from_mode(0o600)).unwrap();
        }
    }
}
