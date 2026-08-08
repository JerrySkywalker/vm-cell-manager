pub mod hyperv;
pub mod qemu;

use serde::Serialize;

use crate::core::capability::ProviderCapabilities;

pub trait LocalVmProvider: Send + Sync {
    fn name(&self) -> &'static str;
    fn probe(&self) -> ProviderProbe;
}

#[derive(Debug, Clone, Serialize)]
pub struct ProviderProbe {
    pub name: &'static str,
    pub available: bool,
    pub detail: String,
    pub capabilities: ProviderCapabilities,
}

#[must_use]
pub fn builtin_provider_probes() -> Vec<ProviderProbe> {
    let providers: Vec<Box<dyn LocalVmProvider>> = vec![
        Box::new(hyperv::HyperVProvider),
        Box::new(qemu::QemuProvider),
    ];

    providers
        .into_iter()
        .map(|provider| provider.probe())
        .collect()
}
