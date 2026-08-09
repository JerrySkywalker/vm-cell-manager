mod executor;

use std::path::PathBuf;

use serde::Deserialize;

use crate::core::capability::ProviderCapabilities;
use crate::providers::{
    ClaimVmRequest, ConfigureVmRequest, CreateOverlayRequest, CreateVmRequest, LocalVmProvider,
    ProviderError, ProviderImageInfo, ProviderMutationAuthority, ProviderProbe, ProviderVm,
    ProviderVmIdentity, VmLookup,
};

pub use executor::PowerShellHyperVExecutor;
pub(crate) use executor::{HyperVAction, HyperVCommandExecutor};

pub struct HyperVProvider<E = PowerShellHyperVExecutor> {
    executor: E,
}

impl HyperVProvider<PowerShellHyperVExecutor> {
    #[must_use]
    pub fn system() -> Self {
        Self::new(PowerShellHyperVExecutor)
    }
}

impl<E> HyperVProvider<E> {
    #[must_use]
    pub(crate) fn new(executor: E) -> Self {
        Self { executor }
    }
}

impl<E: HyperVCommandExecutor> HyperVProvider<E> {
    fn execute<T: serde::de::DeserializeOwned>(
        &self,
        action: HyperVAction,
    ) -> Result<T, ProviderError> {
        let value = self.executor.execute(action)?;
        serde_json::from_value(value)
            .map_err(|error| ProviderError::InvalidResponse(error.to_string()))
    }
}

impl<E: HyperVCommandExecutor> LocalVmProvider for HyperVProvider<E> {
    fn name(&self) -> &'static str {
        "hyperv"
    }

    fn probe(&self) -> ProviderProbe {
        #[cfg(not(target_os = "windows"))]
        {
            ProviderProbe {
                name: "hyperv",
                available: false,
                detail: "Hyper-V provider is only available on Windows hosts".to_owned(),
                capabilities: ProviderCapabilities::unavailable(),
            }
        }

        #[cfg(target_os = "windows")]
        {
            let result: Result<ProbeResponse, ProviderError> = self.execute(HyperVAction::Probe);
            match result {
                Ok(response) if response.available => ProviderProbe {
                    name: "hyperv",
                    available: true,
                    detail: response.detail,
                    capabilities: ProviderCapabilities {
                        full_system_vm: true,
                        cow_overlay: true,
                        hardware_acceleration: true,
                        accelerators: vec!["hyper-v".to_owned()],
                        guest_os: vec!["windows".to_owned(), "linux".to_owned()],
                        guest_arch: vec![std::env::consts::ARCH.to_owned()],
                        guest_transports: vec!["powershell-direct".to_owned(), "ssh".to_owned()],
                        networkless_guest_exec: true,
                    },
                },
                Ok(response) => ProviderProbe {
                    name: "hyperv",
                    available: false,
                    detail: response.detail,
                    capabilities: ProviderCapabilities::unavailable(),
                },
                Err(error) => ProviderProbe {
                    name: "hyperv",
                    available: false,
                    detail: format!("failed to probe Hyper-V: {error}"),
                    capabilities: ProviderCapabilities::unavailable(),
                },
            }
        }
    }

    fn inspect_image(&self, path: PathBuf) -> Result<ProviderImageInfo, ProviderError> {
        self.execute(HyperVAction::InspectVhd { path })
    }

    fn create_overlay(
        &self,
        authority: &ProviderMutationAuthority<'_>,
        request: &CreateOverlayRequest,
    ) -> Result<ProviderImageInfo, ProviderError> {
        authority.validate_overlay_request(request)?;
        self.execute(HyperVAction::CreateDifferencingVhd {
            parent_path: request.parent_path.clone(),
            overlay_path: request.overlay_path.clone(),
        })
    }

    fn create_vm(
        &self,
        authority: &ProviderMutationAuthority<'_>,
        request: &CreateVmRequest,
    ) -> Result<ProviderVmIdentity, ProviderError> {
        authority.validate_create_request(request)?;
        self.execute(HyperVAction::CreateVm {
            request: request.clone(),
        })
    }

    fn claim_vm(
        &self,
        authority: &ProviderMutationAuthority<'_>,
        request: &ClaimVmRequest,
    ) -> Result<ProviderVm, ProviderError> {
        authority.validate_claim_request(request)?;
        self.execute(HyperVAction::ClaimVm {
            request: request.clone(),
        })
    }

    fn configure_vm(
        &self,
        authority: &ProviderMutationAuthority<'_>,
        request: &ConfigureVmRequest,
    ) -> Result<ProviderVm, ProviderError> {
        authority.validate_vm(&request.expected)?;
        self.execute(HyperVAction::ConfigureVm {
            request: request.clone(),
        })
    }

    fn inspect_vm(&self, lookup: &VmLookup) -> Result<Option<ProviderVm>, ProviderError> {
        let response: VmEnvelope = self.execute(HyperVAction::InspectVm {
            lookup: lookup.clone(),
        })?;
        Ok(response.vm)
    }

    fn start_vm(
        &self,
        authority: &ProviderMutationAuthority<'_>,
        expected: &ProviderVm,
    ) -> Result<(), ProviderError> {
        authority.validate_vm(expected)?;
        let _: EmptyResponse = self.execute(HyperVAction::StartVm {
            expected: expected.clone(),
        })?;
        Ok(())
    }

    fn stop_vm(
        &self,
        authority: &ProviderMutationAuthority<'_>,
        expected: &ProviderVm,
    ) -> Result<(), ProviderError> {
        authority.validate_vm(expected)?;
        let _: EmptyResponse = self.execute(HyperVAction::StopVm {
            expected: expected.clone(),
        })?;
        Ok(())
    }

    fn remove_vm(
        &self,
        authority: &ProviderMutationAuthority<'_>,
        expected: &ProviderVm,
    ) -> Result<(), ProviderError> {
        authority.validate_vm(expected)?;
        let _: EmptyResponse = self.execute(HyperVAction::RemoveVm {
            expected: expected.clone(),
        })?;
        Ok(())
    }
}

#[derive(Debug, Deserialize)]
struct ProbeResponse {
    available: bool,
    detail: String,
}

#[derive(Debug, Deserialize)]
struct VmEnvelope {
    vm: Option<ProviderVm>,
}

#[derive(Debug, Deserialize)]
struct EmptyResponse {
    #[allow(dead_code)]
    ok: bool,
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use serde_json::json;

    use super::*;
    struct RecordingExecutor {
        actions: Mutex<Vec<HyperVAction>>,
        response: serde_json::Value,
    }

    impl HyperVCommandExecutor for RecordingExecutor {
        fn execute(&self, action: HyperVAction) -> Result<serde_json::Value, ProviderError> {
            self.actions.lock().unwrap().push(action);
            Ok(self.response.clone())
        }
    }

    #[test]
    fn create_vm_uses_one_typed_executor_action() {
        let (_directory, _state, installation, runtime, record) =
            crate::providers::test_mutation_fixture();
        let executor = RecordingExecutor {
            actions: Mutex::new(Vec::new()),
            response: json!({
                "id": "11111111-1111-1111-1111-111111111111",
                "name": record.ownership.provider_object_name.clone()
            }),
        };
        let provider = HyperVProvider::new(executor);
        let request = CreateVmRequest {
            name: record.ownership.provider_object_name.clone(),
            configuration_path: record.ownership.configuration_path.clone(),
            overlay_path: record.ownership.overlay_path.clone(),
            memory_mib: 4096,
        };
        let authority = ProviderMutationAuthority::new(&record, &installation, &runtime);

        let identity = provider.create_vm(&authority, &request).unwrap();

        assert_eq!(identity.name, request.name);
        assert_eq!(
            provider.executor.actions.lock().unwrap().as_slice(),
            &[HyperVAction::CreateVm { request }]
        );
    }

    #[test]
    fn mutation_authority_rejects_out_of_cell_provider_path() {
        let (_directory, _state, installation, runtime, record) =
            crate::providers::test_mutation_fixture();
        let executor = RecordingExecutor {
            actions: Mutex::new(Vec::new()),
            response: json!(null),
        };
        let provider = HyperVProvider::new(executor);
        let request = CreateVmRequest {
            name: record.ownership.provider_object_name.clone(),
            configuration_path: PathBuf::from(r"C:\foreign"),
            overlay_path: record.ownership.overlay_path.clone(),
            memory_mib: 4096,
        };
        let authority = ProviderMutationAuthority::new(&record, &installation, &runtime);

        assert!(matches!(
            provider.create_vm(&authority, &request),
            Err(ProviderError::Authority(_))
        ));
        assert!(provider.executor.actions.lock().unwrap().is_empty());
    }
}
