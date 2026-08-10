use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::providers::{ProviderProbe, ProviderProbeStatus};

use super::automation::AUTOMATION_SCHEMA_VERSION;
use super::image::{Architecture, GuestOs, ImageId, ImageRecord};
use super::support::{
    Accelerator, GuestTransportId, HostOs, ProviderId, SUPPORT_MATRIX, SupportKey,
    SupportMatrixEntry, SupportStatus, support_for,
};

pub const RUN_PLAN_SCHEMA_VERSION: u32 = 1;
pub const RUN_PLAN_CONTRACT: &str = "vmcell.run-plan.v1";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostPlatform {
    pub os: HostOs,
    pub architecture: Architecture,
}

impl HostPlatform {
    pub fn current() -> Result<Self, RunSelectionError> {
        let os = match std::env::consts::OS {
            "windows" => HostOs::Windows,
            "linux" => HostOs::Linux,
            "macos" => HostOs::Macos,
            _ => return Err(RunSelectionError::UnsupportedHost),
        };
        let architecture = match std::env::consts::ARCH {
            "x86_64" => Architecture::X86_64,
            "aarch64" => Architecture::Aarch64,
            _ => return Err(RunSelectionError::UnsupportedHostArchitecture),
        };
        Ok(Self { os, architecture })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RequestedAccelerator {
    Auto,
    Whpx,
    Kvm,
    Hvf,
    Tcg,
}

impl RequestedAccelerator {
    const fn exact(self) -> Option<Accelerator> {
        match self {
            Self::Auto => None,
            Self::Whpx => Some(Accelerator::Whpx),
            Self::Kvm => Some(Accelerator::Kvm),
            Self::Hvf => Some(Accelerator::Hvf),
            Self::Tcg => Some(Accelerator::Tcg),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RunSelectionIntent {
    pub explicit_provider: Option<ProviderId>,
    pub config_provider_preference: Option<ProviderId>,
    pub explicit_accelerator: Option<RequestedAccelerator>,
    pub allow_tcg: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunSelectionSource {
    ExplicitCli,
    ConfigPreference,
    NativeDefault,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunExecutionPlan {
    pub schema_version: u32,
    pub contract: String,
    pub image: ImageId,
    pub host_os: HostOs,
    pub host_architecture: Architecture,
    pub guest_os: GuestOs,
    pub guest_architecture: Architecture,
    pub provider: ProviderId,
    pub accelerator: Accelerator,
    pub guest_transport: GuestTransportId,
    pub support_status: SupportStatus,
    pub selection_source: RunSelectionSource,
    pub authorizing: bool,
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum RunSelectionError {
    #[error("the current host operating system is not modeled for run selection")]
    UnsupportedHost,
    #[error("the current host architecture is not modeled for run selection")]
    UnsupportedHostArchitecture,
    #[error("more than one equally preferred execution path is compatible")]
    Ambiguous,
    #[error("the logical image has no compatible provider variant")]
    IncompatibleImageVariant,
    #[error("the selected provider is unavailable")]
    ProviderUnavailable,
    #[error("the required accelerator is unavailable")]
    AcceleratorUnavailable,
    #[error("the image architecture does not match the selected native path")]
    ArchitectureMismatch,
    #[error("the required guest transport is unsupported or unavailable")]
    UnsupportedGuestTransport,
    #[error("provider capability evidence is contradictory")]
    ContradictoryCapabilityEvidence,
    #[error("the execution combination is absent from the support contract")]
    UndocumentedCombination,
    #[error("the execution combination is declared unsupported")]
    UnsupportedCombination,
    #[error("TCG requires explicit --accelerator tcg and --allow-tcg together")]
    TcgRequiresExplicitOptIn,
    #[error("the explicit provider and accelerator cannot be combined")]
    ImpossibleProviderAccelerator,
    #[error("the execution plan no longer matches current read-only evidence")]
    PlanDrift,
}

impl RunSelectionError {
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::UnsupportedHost | Self::UnsupportedHostArchitecture => {
                "vmcell.run_plan.host_unsupported"
            }
            Self::Ambiguous => "vmcell.run_plan.ambiguous",
            Self::IncompatibleImageVariant => "vmcell.run_plan.incompatible_image_variant",
            Self::ProviderUnavailable => "vmcell.run_plan.provider_unavailable",
            Self::AcceleratorUnavailable => "vmcell.run_plan.accelerator_unavailable",
            Self::ArchitectureMismatch => "vmcell.run_plan.architecture_mismatch",
            Self::UnsupportedGuestTransport => "vmcell.run_plan.guest_transport_unsupported",
            Self::ContradictoryCapabilityEvidence => "vmcell.run_plan.capability_conflict",
            Self::UndocumentedCombination => "vmcell.run_plan.undocumented_combination",
            Self::UnsupportedCombination => "vmcell.run_plan.unsupported_combination",
            Self::TcgRequiresExplicitOptIn => "vmcell.run_plan.tcg_requires_explicit_opt_in",
            Self::ImpossibleProviderAccelerator => {
                "vmcell.run_plan.impossible_provider_accelerator"
            }
            Self::PlanDrift => "vmcell.run_plan.drift",
        }
    }
}

#[derive(Default)]
struct Rejections {
    provider_unavailable: bool,
    accelerator_unavailable: bool,
    architecture_mismatch: bool,
    transport_unsupported: bool,
    undocumented: bool,
    unsupported: bool,
}

pub fn resolve_run_execution_plan(
    host: HostPlatform,
    image: &ImageRecord,
    probes: &[ProviderProbe],
    intent: RunSelectionIntent,
) -> Result<RunExecutionPlan, RunSelectionError> {
    resolve_run_execution_plan_with_matrix(host, image, probes, intent, SUPPORT_MATRIX)
}

fn resolve_run_execution_plan_with_matrix(
    host: HostPlatform,
    image: &ImageRecord,
    probes: &[ProviderProbe],
    intent: RunSelectionIntent,
    matrix: &[SupportMatrixEntry],
) -> Result<RunExecutionPlan, RunSelectionError> {
    validate_probe_snapshots(probes)?;
    validate_tcg_intent(intent)?;

    let explicit_accelerator_provider = intent.explicit_accelerator.map(|_| ProviderId::Qemu);
    if intent.explicit_provider == Some(ProviderId::Hyperv)
        && explicit_accelerator_provider.is_some()
    {
        return Err(RunSelectionError::ImpossibleProviderAccelerator);
    }
    let selected_provider = intent
        .explicit_provider
        .or(explicit_accelerator_provider)
        .or(intent.config_provider_preference);
    let selection_source =
        if intent.explicit_provider.is_some() || intent.explicit_accelerator.is_some() {
            RunSelectionSource::ExplicitCli
        } else if intent.config_provider_preference.is_some() {
            RunSelectionSource::ConfigPreference
        } else {
            RunSelectionSource::NativeDefault
        };

    let mut variant_providers = BTreeSet::new();
    for variant in &image.variants {
        let provider =
            provider_id(&variant.provider).ok_or(RunSelectionError::IncompatibleImageVariant)?;
        if !variant_providers.insert(provider_rank(provider)) {
            return Err(RunSelectionError::Ambiguous);
        }
    }
    if image.variants.is_empty()
        || selected_provider.is_some_and(|provider| {
            !image
                .variants
                .iter()
                .any(|variant| provider_id(&variant.provider) == Some(provider))
        })
    {
        return Err(RunSelectionError::IncompatibleImageVariant);
    }

    let mut candidates = Vec::new();
    let mut rejected = Rejections::default();
    for variant in &image.variants {
        let provider =
            provider_id(&variant.provider).ok_or(RunSelectionError::IncompatibleImageVariant)?;
        if selected_provider.is_some_and(|selected| selected != provider) {
            continue;
        }
        if image.guest_arch != host.architecture {
            rejected.architecture_mismatch = true;
            continue;
        }
        let Some(probe) = probes.iter().find(|probe| probe.name == provider.as_str()) else {
            rejected.provider_unavailable = true;
            continue;
        };
        if probe.status != ProviderProbeStatus::Ready || !probe.available {
            rejected.provider_unavailable = true;
            continue;
        }
        if !probe
            .capabilities
            .guest_os
            .iter()
            .any(|value| value == guest_os_name(image.guest_os))
        {
            continue;
        }
        if !probe
            .capabilities
            .guest_arch
            .iter()
            .any(|value| value == architecture_name(image.guest_arch))
        {
            rejected.architecture_mismatch = true;
            continue;
        }

        let accelerator = match provider {
            ProviderId::Hyperv => Accelerator::HyperV,
            ProviderId::Qemu => intent
                .explicit_accelerator
                .and_then(RequestedAccelerator::exact)
                .unwrap_or_else(|| native_qemu_accelerator(host.os)),
        };
        if !probe
            .capabilities
            .accelerators
            .iter()
            .any(|value| value == accelerator.as_str())
        {
            rejected.accelerator_unavailable = true;
            continue;
        }

        let matching_rows = matrix
            .iter()
            .filter(|entry| {
                entry.key.host_os == host.os
                    && entry.key.host_architecture == host.architecture
                    && entry.key.provider == provider
                    && entry.key.accelerator == accelerator
                    && entry.key.guest_os == image.guest_os
                    && entry.key.guest_architecture == image.guest_arch
            })
            .collect::<Vec<_>>();
        if matching_rows.is_empty() {
            rejected.undocumented = true;
            continue;
        }
        let mut selectable_row = false;
        for entry in matching_rows {
            if entry.status == SupportStatus::Unsupported {
                rejected.unsupported = true;
                continue;
            }
            selectable_row = true;
            if !probe
                .capabilities
                .guest_transports
                .iter()
                .any(|value| value == entry.key.guest_transport.as_str())
                || !probe.capabilities.networkless_guest_exec
            {
                rejected.transport_unsupported = true;
                continue;
            }
            candidates.push((
                native_provider_rank(host.os, provider),
                RunExecutionPlan {
                    schema_version: RUN_PLAN_SCHEMA_VERSION,
                    contract: RUN_PLAN_CONTRACT.to_owned(),
                    image: image.id.clone(),
                    host_os: host.os,
                    host_architecture: host.architecture,
                    guest_os: image.guest_os,
                    guest_architecture: image.guest_arch,
                    provider,
                    accelerator,
                    guest_transport: entry.key.guest_transport,
                    support_status: entry.status,
                    selection_source,
                    authorizing: false,
                },
            ));
        }
        if selectable_row && candidates.is_empty() {
            rejected.transport_unsupported = true;
        }
    }

    if candidates.is_empty() {
        return Err(rejection_error(rejected));
    }
    candidates.sort_by_key(|(rank, _)| *rank);
    let best_rank = candidates[0].0;
    if candidates
        .iter()
        .filter(|(rank, _)| *rank == best_rank)
        .count()
        != 1
    {
        return Err(RunSelectionError::Ambiguous);
    }
    Ok(candidates.remove(0).1)
}

pub fn revalidate_run_execution_plan(
    plan: &RunExecutionPlan,
    host: HostPlatform,
    image: &ImageRecord,
    probe: &ProviderProbe,
) -> Result<(), RunSelectionError> {
    if plan.schema_version != RUN_PLAN_SCHEMA_VERSION
        || plan.contract != RUN_PLAN_CONTRACT
        || plan.authorizing
        || plan.host_os != host.os
        || plan.host_architecture != host.architecture
        || plan.image != image.id
        || plan.guest_os != image.guest_os
        || plan.guest_architecture != image.guest_arch
        || probe.name != plan.provider.as_str()
        || image
            .variants
            .iter()
            .filter(|variant| provider_id(&variant.provider) == Some(plan.provider))
            .count()
            != 1
    {
        return Err(RunSelectionError::PlanDrift);
    }
    validate_probe_snapshots(std::slice::from_ref(probe))?;
    if probe.status != ProviderProbeStatus::Ready || !probe.available {
        return Err(RunSelectionError::ProviderUnavailable);
    }
    if !probe
        .capabilities
        .accelerators
        .iter()
        .any(|value| value == plan.accelerator.as_str())
    {
        return Err(RunSelectionError::AcceleratorUnavailable);
    }
    if !probe
        .capabilities
        .guest_arch
        .iter()
        .any(|value| value == architecture_name(plan.guest_architecture))
    {
        return Err(RunSelectionError::ArchitectureMismatch);
    }
    if !probe
        .capabilities
        .guest_os
        .iter()
        .any(|value| value == guest_os_name(plan.guest_os))
        || !probe
            .capabilities
            .guest_transports
            .iter()
            .any(|value| value == plan.guest_transport.as_str())
        || !probe.capabilities.networkless_guest_exec
    {
        return Err(RunSelectionError::UnsupportedGuestTransport);
    }
    let entry = support_for(&SupportKey {
        host_os: plan.host_os,
        host_architecture: plan.host_architecture,
        provider: plan.provider,
        accelerator: plan.accelerator,
        guest_os: plan.guest_os,
        guest_architecture: plan.guest_architecture,
        guest_transport: plan.guest_transport,
    })
    .map_err(|_| RunSelectionError::UndocumentedCombination)?;
    if entry.status == SupportStatus::Unsupported {
        return Err(RunSelectionError::UnsupportedCombination);
    }
    if entry.status != plan.support_status {
        return Err(RunSelectionError::PlanDrift);
    }
    Ok(())
}

fn validate_tcg_intent(intent: RunSelectionIntent) -> Result<(), RunSelectionError> {
    let explicit_tcg = intent.explicit_accelerator == Some(RequestedAccelerator::Tcg);
    if explicit_tcg != intent.allow_tcg {
        return Err(RunSelectionError::TcgRequiresExplicitOptIn);
    }
    Ok(())
}

fn validate_probe_snapshots(probes: &[ProviderProbe]) -> Result<(), RunSelectionError> {
    let mut providers = BTreeSet::new();
    for probe in probes {
        let provider =
            provider_id(probe.name).ok_or(RunSelectionError::ContradictoryCapabilityEvidence)?;
        if !providers.insert(provider_rank(provider))
            || probe.available != (probe.status == ProviderProbeStatus::Ready)
        {
            return Err(RunSelectionError::ContradictoryCapabilityEvidence);
        }
        if probe.status != ProviderProbeStatus::Ready {
            if probe.capabilities != crate::core::capability::ProviderCapabilities::unavailable() {
                return Err(RunSelectionError::ContradictoryCapabilityEvidence);
            }
            continue;
        }
        let capabilities = &probe.capabilities;
        if capabilities.schema_version != AUTOMATION_SCHEMA_VERSION
            || !capabilities.full_system_vm
            || !capabilities.cow_overlay
            || has_duplicates(&capabilities.accelerators)
            || has_duplicates(&capabilities.guest_os)
            || has_duplicates(&capabilities.guest_arch)
            || has_duplicates(&capabilities.guest_transports)
            || capabilities
                .accelerators
                .iter()
                .any(|value| !matches!(value.as_str(), "hyper-v" | "whpx" | "kvm" | "hvf" | "tcg"))
            || capabilities
                .guest_os
                .iter()
                .any(|value| !matches!(value.as_str(), "windows" | "linux" | "macos"))
            || capabilities
                .guest_arch
                .iter()
                .any(|value| !matches!(value.as_str(), "x86_64" | "aarch64"))
            || capabilities
                .guest_transports
                .iter()
                .any(|value| !matches!(value.as_str(), "powershell-direct" | "qga" | "ssh"))
        {
            return Err(RunSelectionError::ContradictoryCapabilityEvidence);
        }
        let hardware = capabilities
            .accelerators
            .iter()
            .any(|value| matches!(value.as_str(), "hyper-v" | "whpx" | "kvm" | "hvf"));
        let accelerators_match_provider = match provider {
            ProviderId::Hyperv => {
                capabilities.accelerators == ["hyper-v"] && capabilities.hardware_acceleration
            }
            ProviderId::Qemu => {
                !capabilities
                    .accelerators
                    .iter()
                    .any(|value| value == "hyper-v")
                    && capabilities.hardware_acceleration == hardware
            }
        };
        if !accelerators_match_provider {
            return Err(RunSelectionError::ContradictoryCapabilityEvidence);
        }
    }
    Ok(())
}

fn has_duplicates(values: &[String]) -> bool {
    let mut unique = BTreeSet::new();
    values.iter().any(|value| !unique.insert(value))
}

fn rejection_error(rejected: Rejections) -> RunSelectionError {
    if rejected.architecture_mismatch {
        RunSelectionError::ArchitectureMismatch
    } else if rejected.provider_unavailable {
        RunSelectionError::ProviderUnavailable
    } else if rejected.accelerator_unavailable {
        RunSelectionError::AcceleratorUnavailable
    } else if rejected.transport_unsupported {
        RunSelectionError::UnsupportedGuestTransport
    } else if rejected.unsupported {
        RunSelectionError::UnsupportedCombination
    } else if rejected.undocumented {
        RunSelectionError::UndocumentedCombination
    } else {
        RunSelectionError::IncompatibleImageVariant
    }
}

const fn native_qemu_accelerator(host: HostOs) -> Accelerator {
    match host {
        HostOs::Windows => Accelerator::Whpx,
        HostOs::Linux => Accelerator::Kvm,
        HostOs::Macos => Accelerator::Hvf,
    }
}

const fn native_provider_rank(host: HostOs, provider: ProviderId) -> u8 {
    match (host, provider) {
        (HostOs::Windows, ProviderId::Hyperv) => 0,
        (HostOs::Windows, ProviderId::Qemu) => 1,
        (_, ProviderId::Qemu) => 0,
        (_, ProviderId::Hyperv) => 1,
    }
}

fn provider_id(value: &str) -> Option<ProviderId> {
    match value {
        "hyperv" => Some(ProviderId::Hyperv),
        "qemu" => Some(ProviderId::Qemu),
        _ => None,
    }
}

const fn provider_rank(provider: ProviderId) -> u8 {
    match provider {
        ProviderId::Hyperv => 0,
        ProviderId::Qemu => 1,
    }
}

const fn guest_os_name(value: GuestOs) -> &'static str {
    match value {
        GuestOs::Windows => "windows",
        GuestOs::Linux => "linux",
        GuestOs::Macos => "macos",
    }
}

const fn architecture_name(value: Architecture) -> &'static str {
    match value {
        Architecture::X86_64 => "x86_64",
        Architecture::Aarch64 => "aarch64",
    }
}

#[cfg(test)]
mod tests {
    use chrono::Utc;

    use crate::core::capability::ProviderCapabilities;
    use crate::core::image::{IMAGE_SCHEMA_VERSION, ImageVariant};

    use super::*;

    fn host(os: HostOs) -> HostPlatform {
        HostPlatform {
            os,
            architecture: Architecture::X86_64,
        }
    }

    fn image(guest_os: GuestOs, guest_arch: Architecture, providers: &[&str]) -> ImageRecord {
        ImageRecord {
            schema_version: IMAGE_SCHEMA_VERSION,
            id: "logical-image".parse().unwrap(),
            guest_os,
            guest_arch,
            variants: providers
                .iter()
                .map(|provider| ImageVariant {
                    provider: (*provider).to_owned(),
                    disk_format: if *provider == "hyperv" {
                        "vhdx"
                    } else {
                        "qcow2"
                    }
                    .to_owned(),
                    path: format!("base-{provider}").into(),
                    sha256: "a".repeat(64),
                    file_size: 1024,
                })
                .collect(),
            registered_at: Utc::now(),
        }
    }

    fn ready_probe(
        provider: &'static str,
        accelerators: &[&str],
        guest_os: &[&str],
        transports: &[&str],
    ) -> ProviderProbe {
        let hardware = accelerators
            .iter()
            .any(|value| matches!(*value, "hyper-v" | "whpx" | "kvm" | "hvf"));
        ProviderProbe {
            name: provider,
            status: ProviderProbeStatus::Ready,
            available: true,
            detail: "test evidence".to_owned(),
            capabilities: ProviderCapabilities {
                schema_version: AUTOMATION_SCHEMA_VERSION,
                full_system_vm: true,
                cow_overlay: true,
                hardware_acceleration: hardware,
                accelerators: accelerators
                    .iter()
                    .map(|value| (*value).to_owned())
                    .collect(),
                guest_os: guest_os.iter().map(|value| (*value).to_owned()).collect(),
                guest_arch: vec!["x86_64".to_owned()],
                guest_transports: transports.iter().map(|value| (*value).to_owned()).collect(),
                networkless_guest_exec: true,
            },
        }
    }

    const fn intent() -> RunSelectionIntent {
        RunSelectionIntent {
            explicit_provider: None,
            config_provider_preference: None,
            explicit_accelerator: None,
            allow_tcg: false,
        }
    }

    #[test]
    fn one_valid_path_resolves_without_provider_authority() {
        let image = image(GuestOs::Linux, Architecture::X86_64, &["qemu"]);
        let probes = [ready_probe("qemu", &["kvm", "tcg"], &["linux"], &["qga"])];
        let plan =
            resolve_run_execution_plan(host(HostOs::Linux), &image, &probes, intent()).unwrap();
        assert_eq!(plan.provider, ProviderId::Qemu);
        assert_eq!(plan.accelerator, Accelerator::Kvm);
        assert_eq!(plan.guest_transport, GuestTransportId::Qga);
        assert_eq!(plan.support_status, SupportStatus::Untested);
        assert_eq!(plan.selection_source, RunSelectionSource::NativeDefault);
        assert!(!plan.authorizing);
    }

    #[test]
    fn equal_paths_fail_as_ambiguous() {
        let image = image(GuestOs::Linux, Architecture::X86_64, &["qemu"]);
        let probes = [ready_probe("qemu", &["kvm"], &["linux"], &["qga", "ssh"])];
        let rows = [
            SupportMatrixEntry {
                key: SupportKey {
                    host_os: HostOs::Linux,
                    host_architecture: Architecture::X86_64,
                    provider: ProviderId::Qemu,
                    accelerator: Accelerator::Kvm,
                    guest_os: GuestOs::Linux,
                    guest_architecture: Architecture::X86_64,
                    guest_transport: GuestTransportId::Qga,
                },
                status: SupportStatus::Untested,
                acceptance_evidence: &[],
            },
            SupportMatrixEntry {
                key: SupportKey {
                    guest_transport: GuestTransportId::Ssh,
                    ..SupportKey {
                        host_os: HostOs::Linux,
                        host_architecture: Architecture::X86_64,
                        provider: ProviderId::Qemu,
                        accelerator: Accelerator::Kvm,
                        guest_os: GuestOs::Linux,
                        guest_architecture: Architecture::X86_64,
                        guest_transport: GuestTransportId::Qga,
                    }
                },
                status: SupportStatus::Untested,
                acceptance_evidence: &[],
            },
        ];
        assert_eq!(
            resolve_run_execution_plan_with_matrix(
                host(HostOs::Linux),
                &image,
                &probes,
                intent(),
                &rows
            ),
            Err(RunSelectionError::Ambiguous)
        );
    }

    #[test]
    fn explicit_provider_and_accelerator_override_config() {
        let image = image(GuestOs::Linux, Architecture::X86_64, &["qemu"]);
        let probes = [ready_probe("qemu", &["whpx", "tcg"], &["linux"], &["qga"])];
        let plan = resolve_run_execution_plan(
            host(HostOs::Windows),
            &image,
            &probes,
            RunSelectionIntent {
                explicit_provider: Some(ProviderId::Qemu),
                config_provider_preference: Some(ProviderId::Hyperv),
                explicit_accelerator: Some(RequestedAccelerator::Whpx),
                allow_tcg: false,
            },
        )
        .unwrap();
        assert_eq!(plan.provider, ProviderId::Qemu);
        assert_eq!(plan.accelerator, Accelerator::Whpx);
        assert_eq!(plan.selection_source, RunSelectionSource::ExplicitCli);
    }

    #[test]
    fn config_preference_is_used_only_when_present_and_does_not_fallback() {
        let image = image(GuestOs::Linux, Architecture::X86_64, &["qemu"]);
        let unavailable = ProviderProbe {
            name: "qemu",
            status: ProviderProbeStatus::Unavailable,
            available: false,
            detail: "not installed".to_owned(),
            capabilities: ProviderCapabilities::unavailable(),
        };
        let result = resolve_run_execution_plan(
            host(HostOs::Linux),
            &image,
            &[unavailable],
            RunSelectionIntent {
                config_provider_preference: Some(ProviderId::Qemu),
                ..intent()
            },
        );
        assert_eq!(result, Err(RunSelectionError::ProviderUnavailable));
    }

    #[test]
    fn unavailable_config_preference_does_not_fall_back_to_native_hyperv() {
        let image = image(GuestOs::Windows, Architecture::X86_64, &["hyperv", "qemu"]);
        let hyperv = ready_probe("hyperv", &["hyper-v"], &["windows"], &["powershell-direct"]);
        let qemu = ProviderProbe {
            name: "qemu",
            status: ProviderProbeStatus::Unavailable,
            available: false,
            detail: "not installed".to_owned(),
            capabilities: ProviderCapabilities::unavailable(),
        };
        let qemu_windows = SupportMatrixEntry {
            key: SupportKey {
                host_os: HostOs::Windows,
                host_architecture: Architecture::X86_64,
                provider: ProviderId::Qemu,
                accelerator: Accelerator::Whpx,
                guest_os: GuestOs::Windows,
                guest_architecture: Architecture::X86_64,
                guest_transport: GuestTransportId::Qga,
            },
            status: SupportStatus::Untested,
            acceptance_evidence: &[],
        };
        let rows = [SUPPORT_MATRIX[0], qemu_windows];

        let native = resolve_run_execution_plan_with_matrix(
            host(HostOs::Windows),
            &image,
            &[hyperv.clone(), qemu.clone()],
            intent(),
            &rows,
        )
        .unwrap();
        assert_eq!(native.provider, ProviderId::Hyperv);
        assert_eq!(native.selection_source, RunSelectionSource::NativeDefault);

        assert_eq!(
            resolve_run_execution_plan_with_matrix(
                host(HostOs::Windows),
                &image,
                &[hyperv, qemu],
                RunSelectionIntent {
                    config_provider_preference: Some(ProviderId::Qemu),
                    ..intent()
                },
                &rows,
            ),
            Err(RunSelectionError::ProviderUnavailable)
        );
    }

    #[test]
    fn incompatible_variant_and_architecture_mismatch_are_distinct() {
        let qemu = ready_probe("qemu", &["kvm"], &["linux"], &["qga"]);
        let logical_image = image(GuestOs::Linux, Architecture::X86_64, &["qemu"]);
        assert_eq!(
            resolve_run_execution_plan(
                host(HostOs::Linux),
                &logical_image,
                std::slice::from_ref(&qemu),
                RunSelectionIntent {
                    explicit_provider: Some(ProviderId::Hyperv),
                    ..intent()
                },
            ),
            Err(RunSelectionError::IncompatibleImageVariant)
        );
        let foreign = image(GuestOs::Linux, Architecture::Aarch64, &["qemu"]);
        assert_eq!(
            resolve_run_execution_plan(host(HostOs::Linux), &foreign, &[qemu], intent()),
            Err(RunSelectionError::ArchitectureMismatch)
        );
    }

    #[test]
    fn missing_transport_is_deterministic() {
        let image = image(GuestOs::Linux, Architecture::X86_64, &["qemu"]);
        let probes = [ready_probe("qemu", &["kvm"], &["linux"], &[])];
        assert_eq!(
            resolve_run_execution_plan(host(HostOs::Linux), &image, &probes, intent()),
            Err(RunSelectionError::UnsupportedGuestTransport)
        );
    }

    #[test]
    fn whpx_and_kvm_are_accelerators_under_qemu() {
        for (os, accelerator) in [(HostOs::Windows, "whpx"), (HostOs::Linux, "kvm")] {
            let image = image(GuestOs::Linux, Architecture::X86_64, &["qemu"]);
            let probes = [ready_probe("qemu", &[accelerator], &["linux"], &["qga"])];
            let plan = resolve_run_execution_plan(host(os), &image, &probes, intent()).unwrap();
            assert_eq!(plan.provider, ProviderId::Qemu);
            assert_eq!(plan.accelerator.as_str(), accelerator);
        }
    }

    #[test]
    fn tcg_requires_both_explicit_flags_and_never_falls_back() {
        let image = image(GuestOs::Linux, Architecture::X86_64, &["qemu"]);
        let probes = [ready_probe("qemu", &["tcg"], &["linux"], &["qga"])];
        for rejected in [
            RunSelectionIntent {
                allow_tcg: true,
                ..intent()
            },
            RunSelectionIntent {
                explicit_accelerator: Some(RequestedAccelerator::Tcg),
                ..intent()
            },
            intent(),
        ] {
            assert!(
                resolve_run_execution_plan(host(HostOs::Linux), &image, &probes, rejected).is_err()
            );
        }
        let plan = resolve_run_execution_plan(
            host(HostOs::Linux),
            &image,
            &probes,
            RunSelectionIntent {
                explicit_accelerator: Some(RequestedAccelerator::Tcg),
                allow_tcg: true,
                ..intent()
            },
        )
        .unwrap();
        assert_eq!(plan.accelerator, Accelerator::Tcg);
        assert_eq!(plan.support_status, SupportStatus::DevelopmentOnly);
    }

    #[test]
    fn contradictory_probe_evidence_fails_closed() {
        let image = image(GuestOs::Linux, Architecture::X86_64, &["qemu"]);
        let mut probe = ready_probe("qemu", &["kvm"], &["linux"], &["qga"]);
        probe.available = false;
        assert_eq!(
            resolve_run_execution_plan(host(HostOs::Linux), &image, &[probe], intent()),
            Err(RunSelectionError::ContradictoryCapabilityEvidence)
        );
    }

    #[test]
    fn revalidation_detects_provider_and_accelerator_drift() {
        let image = image(GuestOs::Linux, Architecture::X86_64, &["qemu"]);
        let initial = ready_probe("qemu", &["kvm", "tcg"], &["linux"], &["qga"]);
        let plan =
            resolve_run_execution_plan(host(HostOs::Linux), &image, &[initial], intent()).unwrap();
        let drifted = ready_probe("qemu", &["tcg"], &["linux"], &["qga"]);
        assert_eq!(
            revalidate_run_execution_plan(&plan, host(HostOs::Linux), &image, &drifted),
            Err(RunSelectionError::AcceleratorUnavailable)
        );
    }
}
