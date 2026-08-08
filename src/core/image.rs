use std::fmt;
use std::path::PathBuf;
use std::str::FromStr;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ImageId(String);

impl ImageId {
    pub fn parse(value: impl Into<String>) -> Result<Self, ImageIdError> {
        let value = value.into();
        let valid_length = !value.is_empty() && value.len() <= 128;
        let valid_characters = value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'));

        if !valid_length || !valid_characters || matches!(value.as_str(), "." | "..") {
            return Err(ImageIdError(value));
        }

        Ok(Self(value))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ImageId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl FromStr for ImageId {
    type Err = ImageIdError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::parse(value)
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
#[error("invalid image id: {0}")]
pub struct ImageIdError(pub String);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GuestOs {
    Windows,
    Linux,
    Macos,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Architecture {
    X86_64,
    Aarch64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImageVariant {
    pub provider: String,
    pub disk_format: String,
    pub path: PathBuf,
    pub sha256: String,
    pub file_size: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImageRecord {
    pub schema_version: u32,
    pub id: ImageId,
    pub guest_os: GuestOs,
    pub guest_arch: Architecture,
    pub variants: Vec<ImageVariant>,
    pub registered_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImageBinding {
    pub image_id: ImageId,
    pub provider: String,
    pub disk_format: String,
    pub path: PathBuf,
    pub sha256: String,
    pub file_size: u64,
}

impl ImageBinding {
    #[must_use]
    pub fn from_variant(image_id: ImageId, variant: &ImageVariant) -> Self {
        Self {
            image_id,
            provider: variant.provider.clone(),
            disk_format: variant.disk_format.clone(),
            path: variant.path.clone(),
            sha256: variant.sha256.clone(),
            file_size: variant.file_size,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn image_ids_are_safe_path_components() {
        assert!(ImageId::parse("windows-dev_2026.08").is_ok());
        assert!(ImageId::parse("").is_err());
        assert!(ImageId::parse("..").is_err());
        assert!(ImageId::parse("windows/dev").is_err());
        assert!(ImageId::parse("windows dev").is_err());
    }
}
