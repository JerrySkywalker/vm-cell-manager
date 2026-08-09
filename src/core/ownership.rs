use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::cell::CellId;

pub const OWNERSHIP_MARKER_SCHEMA: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CellOwnership {
    pub schema_version: u32,
    pub install_id: Uuid,
    pub operation_id: Uuid,
    pub provider_object_name: String,
    pub provider_marker: String,
    pub configuration_path: PathBuf,
    pub overlay_path: PathBuf,
}

impl CellOwnership {
    #[must_use]
    pub fn new(
        install_id: Uuid,
        cell_id: CellId,
        operation_id: Uuid,
        configuration_path: PathBuf,
        overlay_path: PathBuf,
    ) -> Self {
        let provider_object_name = format!("vmcell-{}", cell_id.0);
        let provider_marker = format!(
            "vmcell:v{}:{install_id}:{}:{operation_id}",
            OWNERSHIP_MARKER_SCHEMA, cell_id.0
        );

        Self {
            schema_version: OWNERSHIP_MARKER_SCHEMA,
            install_id,
            operation_id,
            provider_object_name,
            provider_marker,
            configuration_path,
            overlay_path,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderObjectIdentity {
    pub id: String,
    pub name: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn marker_binds_install_cell_and_operation() {
        let install_id = Uuid::nil();
        let cell_id = CellId(Uuid::from_u128(1));
        let operation_id = Uuid::from_u128(2);
        let ownership = CellOwnership::new(
            install_id,
            cell_id,
            operation_id,
            PathBuf::from("config"),
            PathBuf::from("cell.vhdx"),
        );

        assert_eq!(
            ownership.provider_object_name,
            format!("vmcell-{}", cell_id.0)
        );
        assert_eq!(
            ownership.provider_marker,
            format!("vmcell:v1:{install_id}:{}:{operation_id}", cell_id.0)
        );
    }
}
