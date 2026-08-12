//! Opt-in deterministic model matrices for repository-local reliability work.
//!
//! These checks complement, rather than replace, the focused unit and
//! integration suites. They use fixed inputs and independent expected
//! outcomes, so a failure identifies a stable case without touching provider,
//! guest, state-root, or host authority. The seeded lifecycle corpus remains
//! owned by `reliability_harness`; this target owns cross-contract selection,
//! job-plan/result, and durable-provenance properties.

use std::fs;
use std::path::Path;

use chrono::{Duration, TimeZone, Utc};
use sha2::{Digest, Sha256};
use vm_cell_manager::core::automation::AUTOMATION_SCHEMA_VERSION;
use vm_cell_manager::core::capability::ProviderCapabilities;
use vm_cell_manager::core::image::{Architecture, GuestOs, ImageId, ImageRecord, ImageVariant};
use vm_cell_manager::core::job_spec::{JobSpecError, load_job_spec, parse_job_spec};
use vm_cell_manager::core::run_selection::{
    HostPlatform, RequestedAccelerator, RunSelectionError, RunSelectionIntent, RunSelectionSource,
    resolve_run_execution_plan,
};
use vm_cell_manager::core::support::{
    Accelerator, GuestTransportId, HostOs, ProviderId, SupportStatus,
};
use vm_cell_manager::core::{
    cell::CellRecord,
    guest::{ArtifactRecord, GuestOperationRecord},
    job::JobRunContext,
    job_plan::resolve_job_plan,
};
use vm_cell_manager::providers::{ProviderProbe, ProviderProbeStatus};

const CONTRACT: &str = "vmcell.reliability-model-matrix.v1";
#[test]
#[ignore = "extended reliability suite; run explicitly with cargo test --test reliability_model_matrix -- --ignored"]
fn run_selection_matrix_has_stable_outcomes_and_never_implicitly_selects_tcg() {
    let linux = HostPlatform {
        os: HostOs::Linux,
        architecture: Architecture::X86_64,
    };
    let windows = HostPlatform {
        os: HostOs::Windows,
        architecture: Architecture::X86_64,
    };
    let linux_image = image(GuestOs::Linux, Architecture::X86_64, &["qemu"]);

    assert_selection_plan(
        "native-linux-kvm",
        linux,
        &linux_image,
        &[ready_probe("qemu", &["kvm", "tcg"], &["linux"], &["qga"])],
        intent(),
        SelectionExpectation {
            provider: ProviderId::Qemu,
            accelerator: Accelerator::Kvm,
            transport: GuestTransportId::Qga,
            status: SupportStatus::Untested,
            source: RunSelectionSource::NativeDefault,
        },
    );
    assert_selection_error(
        "allow-tcg-without-explicit-tcg-is-rejected-even-when-native-is-ready",
        linux,
        &linux_image,
        &[ready_probe("qemu", &["kvm", "tcg"], &["linux"], &["qga"])],
        RunSelectionIntent {
            allow_tcg: true,
            ..intent()
        },
        RunSelectionError::TcgRequiresExplicitOptIn,
        "vmcell.run_plan.tcg_requires_explicit_opt_in",
    );
    assert_selection_plan(
        "explicit-tcg-requires-and-uses-opt-in",
        linux,
        &linux_image,
        &[ready_probe("qemu", &["kvm", "tcg"], &["linux"], &["qga"])],
        RunSelectionIntent {
            explicit_accelerator: Some(RequestedAccelerator::Tcg),
            allow_tcg: true,
            ..intent()
        },
        SelectionExpectation {
            provider: ProviderId::Qemu,
            accelerator: Accelerator::Tcg,
            transport: GuestTransportId::Qga,
            status: SupportStatus::DevelopmentOnly,
            source: RunSelectionSource::ExplicitCli,
        },
    );
    assert_selection_plan(
        "explicit-windows-qemu-whpx",
        windows,
        &linux_image,
        &[ready_probe("qemu", &["whpx"], &["linux"], &["qga"])],
        RunSelectionIntent {
            explicit_provider: Some(ProviderId::Qemu),
            explicit_accelerator: Some(RequestedAccelerator::Whpx),
            ..intent()
        },
        SelectionExpectation {
            provider: ProviderId::Qemu,
            accelerator: Accelerator::Whpx,
            transport: GuestTransportId::Qga,
            status: SupportStatus::Untested,
            source: RunSelectionSource::ExplicitCli,
        },
    );

    assert_selection_error(
        "missing-explicit-kvm",
        linux,
        &linux_image,
        &[ready_probe("qemu", &["tcg"], &["linux"], &["qga"])],
        RunSelectionIntent {
            explicit_accelerator: Some(RequestedAccelerator::Kvm),
            ..intent()
        },
        RunSelectionError::AcceleratorUnavailable,
        "vmcell.run_plan.accelerator_unavailable",
    );
    assert_selection_error(
        "image-provider-mismatch",
        linux,
        &linux_image,
        &[ready_probe("qemu", &["kvm"], &["linux"], &["qga"])],
        RunSelectionIntent {
            explicit_provider: Some(ProviderId::Hyperv),
            ..intent()
        },
        RunSelectionError::IncompatibleImageVariant,
        "vmcell.run_plan.incompatible_image_variant",
    );
    assert_selection_error(
        "guest-architecture-mismatch",
        linux,
        &image(GuestOs::Linux, Architecture::Aarch64, &["qemu"]),
        &[ready_probe("qemu", &["kvm"], &["linux"], &["qga"])],
        intent(),
        RunSelectionError::ArchitectureMismatch,
        "vmcell.run_plan.architecture_mismatch",
    );
    assert_selection_error(
        "guest-transport-missing",
        linux,
        &linux_image,
        &[ready_probe("qemu", &["kvm"], &["linux"], &[])],
        intent(),
        RunSelectionError::UnsupportedGuestTransport,
        "vmcell.run_plan.guest_transport_unsupported",
    );
    let mut contradictory = ready_probe("qemu", &["kvm"], &["linux"], &["qga"]);
    contradictory.available = false;
    assert_selection_error(
        "contradictory-probe",
        linux,
        &linux_image,
        &[contradictory],
        intent(),
        RunSelectionError::ContradictoryCapabilityEvidence,
        "vmcell.run_plan.capability_conflict",
    );
    assert_selection_error(
        "unsupported-contract-row",
        windows,
        &image(GuestOs::Windows, Architecture::X86_64, &["qemu"]),
        &[ready_probe("qemu", &["whpx"], &["windows"], &["qga"])],
        RunSelectionIntent {
            explicit_provider: Some(ProviderId::Qemu),
            explicit_accelerator: Some(RequestedAccelerator::Whpx),
            ..intent()
        },
        RunSelectionError::UnsupportedCombination,
        "vmcell.run_plan.unsupported_combination",
    );
    assert_selection_error(
        "undocumented-apple-silicon-row",
        HostPlatform {
            os: HostOs::Macos,
            architecture: Architecture::Aarch64,
        },
        &image(GuestOs::Linux, Architecture::Aarch64, &["qemu"]),
        &[ready_probe_with_architecture(
            "qemu",
            &["hvf"],
            &["linux"],
            &["aarch64"],
            &["qga"],
        )],
        intent(),
        RunSelectionError::UndocumentedCombination,
        "vmcell.run_plan.undocumented_combination",
    );
}

#[test]
#[ignore = "extended reliability suite; run explicitly with cargo test --test reliability_model_matrix -- --ignored"]
fn job_spec_plan_and_result_metadata_bind_provenance_without_authority_or_secrets() {
    let directory = tempfile::tempdir().unwrap();
    let first_path = directory.path().join("first.toml");
    let equivalent_path = directory.path().join("equivalent.toml");
    let changed_path = directory.path().join("changed.toml");
    let first_source = job_spec_source("first-secret-argument");
    let equivalent_source = format!(
        "# equivalent formatting must not change the resolved execution contract\n\n{}\n# end\n",
        first_source
    );
    let changed_source = job_spec_source("second-secret-argument");
    write_private(&first_path, &first_source);
    write_private(&equivalent_path, &equivalent_source);
    write_private(&changed_path, &changed_source);

    let first = load_job_spec(&first_path).unwrap();
    let first_again = load_job_spec(&first_path).unwrap();
    let equivalent = load_job_spec(&equivalent_path).unwrap();
    let changed = load_job_spec(&changed_path).unwrap();
    assert_eq!(first.source_sha256(), first_again.source_sha256());
    assert_eq!(
        first.source_sha256(),
        format!("{:x}", Sha256::digest(first_source.as_bytes()))
    );
    assert_ne!(first.source_sha256(), equivalent.source_sha256());
    assert_ne!(first.source_sha256(), changed.source_sha256());

    let host = HostPlatform {
        os: HostOs::Linux,
        architecture: Architecture::X86_64,
    };
    let image = image(GuestOs::Linux, Architecture::X86_64, &["qemu"]);
    let probes = [ready_probe("qemu", &["kvm", "tcg"], &["linux"], &["qga"])];
    let plan = resolve_job_plan(&first, host, &image, &probes).unwrap();
    let repeated = resolve_job_plan(&first_again, host, &image, &probes).unwrap();
    let equivalent_plan = resolve_job_plan(&equivalent, host, &image, &probes).unwrap();
    let changed_plan = resolve_job_plan(&changed, host, &image, &probes).unwrap();
    assert_eq!(
        plan, repeated,
        "{CONTRACT}: same source must resolve identically"
    );
    for semantic_peer in [&equivalent_plan, &changed_plan] {
        assert_eq!(plan.execution, semantic_peer.execution);
        assert_eq!(plan.resources, semantic_peer.resources);
        assert_eq!(plan.timeouts, semantic_peer.timeouts);
        assert_eq!(plan.cleanup, semantic_peer.cleanup);
        assert_eq!(
            plan.declared_copy_in_count,
            semantic_peer.declared_copy_in_count
        );
        assert_eq!(
            plan.declared_artifact_count,
            semantic_peer.declared_artifact_count
        );
        assert_ne!(plan.job_spec_sha256, semantic_peer.job_spec_sha256);
    }
    assert!(!plan.authorizing);
    assert!(!plan.execution.authorizing);
    assert_eq!(plan.execution.accelerator, Accelerator::Kvm);
    assert_eq!(plan.execution.support_status, SupportStatus::Untested);

    let rendered = serde_json::to_string(&plan).unwrap();
    for forbidden in [
        "first-secret-argument",
        "second-secret-argument",
        "secret-program",
        "inputs/private.txt",
        "results/private.txt",
        first_path.to_string_lossy().as_ref(),
        equivalent_path.to_string_lossy().as_ref(),
        changed_path.to_string_lossy().as_ref(),
        "test probe detail",
    ] {
        assert!(
            !rendered.contains(forbidden),
            "{CONTRACT}: job plan leaked {forbidden:?}"
        );
    }

    let started = Utc.with_ymd_and_hms(2026, 8, 12, 0, 0, 0).unwrap();
    let first_context = JobRunContext::new(plan.job_spec_sha256.clone(), started).unwrap();
    let equivalent_context =
        JobRunContext::new(equivalent_plan.job_spec_sha256.clone(), started).unwrap();
    let changed_context =
        JobRunContext::new(changed_plan.job_spec_sha256.clone(), started).unwrap();
    assert_ne!(first_context.job_id(), equivalent_context.job_id());
    assert_ne!(first_context.job_id(), changed_context.job_id());

    let result = first_context.result_metadata(started + Duration::milliseconds(17));
    assert_eq!(result.job_id, first_context.job_id());
    assert_eq!(result.job_spec_sha256, plan.job_spec_sha256);
    assert_eq!(result.started_at, started);
    assert_eq!(result.completed_at, started + Duration::milliseconds(17));
    assert_eq!(result.elapsed_milliseconds, 17);
    let rendered_result = serde_json::to_string(&result).unwrap();
    for forbidden in [
        "first-secret-argument",
        "secret-program",
        first_path.to_string_lossy().as_ref(),
    ] {
        assert!(
            !rendered_result.contains(forbidden),
            "{CONTRACT}: job result leaked {forbidden:?}"
        );
    }

    let unpaired_tcg = job_spec_source("tcg").replace("accelerator = \"kvm\"", "allow_tcg = true");
    assert!(matches!(
        parse_job_spec(&unpaired_tcg),
        Err(JobSpecError::InvalidValue(_))
    ));
}

#[test]
#[ignore = "extended reliability suite; run explicitly with cargo test --test reliability_model_matrix -- --ignored"]
fn durable_correlation_schema_fence_is_property_exact() {
    let started = Utc.with_ymd_and_hms(2026, 8, 12, 0, 0, 0).unwrap();
    let context = JobRunContext::new("a".repeat(64), started).unwrap();
    let correlation = context.correlation();
    assert_eq!(CellRecord::schema_version_for_job(None), 1);
    assert_eq!(CellRecord::schema_version_for_job(Some(&correlation)), 2);
    assert_eq!(GuestOperationRecord::schema_version_for_job(None), 1);
    assert_eq!(
        GuestOperationRecord::schema_version_for_job(Some(correlation.job_id)),
        2
    );
    assert_eq!(ArtifactRecord::schema_version_for_job(None), 1);
    assert_eq!(
        ArtifactRecord::schema_version_for_job(Some(correlation.job_id)),
        2
    );
    assert!(
        !serde_json::to_string(&correlation)
            .unwrap()
            .contains("secret-program")
    );
}

struct SelectionExpectation {
    provider: ProviderId,
    accelerator: Accelerator,
    transport: GuestTransportId,
    status: SupportStatus,
    source: RunSelectionSource,
}

fn assert_selection_plan(
    name: &str,
    host: HostPlatform,
    image: &ImageRecord,
    probes: &[ProviderProbe],
    intent: RunSelectionIntent,
    expected: SelectionExpectation,
) {
    let first = resolve_run_execution_plan(host, image, probes, intent)
        .unwrap_or_else(|error| panic!("{CONTRACT}: {name} unexpectedly failed: {}", error.code()));
    let second = resolve_run_execution_plan(host, image, probes, intent).unwrap();
    assert_eq!(first, second, "{CONTRACT}: {name} was not deterministic");
    assert_eq!(first.provider, expected.provider, "{CONTRACT}: {name}");
    assert_eq!(
        first.accelerator, expected.accelerator,
        "{CONTRACT}: {name}"
    );
    assert_eq!(
        first.guest_transport, expected.transport,
        "{CONTRACT}: {name}"
    );
    assert_eq!(first.support_status, expected.status, "{CONTRACT}: {name}");
    assert_eq!(
        first.selection_source, expected.source,
        "{CONTRACT}: {name}"
    );
    assert!(!first.authorizing, "{CONTRACT}: {name}");
    assert!(
        matches!(
            first.support_status,
            SupportStatus::Untested | SupportStatus::DevelopmentOnly
        ),
        "{CONTRACT}: {name} must not promote support"
    );
}

fn assert_selection_error(
    name: &str,
    host: HostPlatform,
    image: &ImageRecord,
    probes: &[ProviderProbe],
    intent: RunSelectionIntent,
    expected: RunSelectionError,
    expected_code: &str,
) {
    let first = resolve_run_execution_plan(host, image, probes, intent);
    let second = resolve_run_execution_plan(host, image, probes, intent);
    assert_eq!(first, second, "{CONTRACT}: {name} was not deterministic");
    assert_eq!(first, Err(expected.clone()), "{CONTRACT}: {name}");
    assert_eq!(
        expected.code(),
        expected_code,
        "{CONTRACT}: {name} changed its stable error code"
    );
}

fn image(guest_os: GuestOs, guest_arch: Architecture, providers: &[&str]) -> ImageRecord {
    ImageRecord {
        schema_version: 1,
        id: ImageId::parse("reliability-model-image").unwrap(),
        guest_os,
        guest_arch,
        variants: providers
            .iter()
            .map(|provider| ImageVariant {
                provider: (*provider).to_owned(),
                disk_format: if *provider == "hyperv" {
                    "vhdx".to_owned()
                } else {
                    "qcow2".to_owned()
                },
                path: format!("immutable-{provider}.img").into(),
                sha256: "a".repeat(64),
                file_size: 1024,
            })
            .collect(),
        registered_at: Utc.with_ymd_and_hms(2026, 8, 12, 0, 0, 0).unwrap(),
    }
}

fn ready_probe(
    provider: &'static str,
    accelerators: &[&str],
    guest_os: &[&str],
    transports: &[&str],
) -> ProviderProbe {
    ready_probe_with_architecture(provider, accelerators, guest_os, &["x86_64"], transports)
}

fn ready_probe_with_architecture(
    provider: &'static str,
    accelerators: &[&str],
    guest_os: &[&str],
    guest_arch: &[&str],
    transports: &[&str],
) -> ProviderProbe {
    let hardware_acceleration = accelerators
        .iter()
        .any(|accelerator| matches!(*accelerator, "hyper-v" | "whpx" | "kvm" | "hvf"));
    ProviderProbe {
        name: provider,
        status: ProviderProbeStatus::Ready,
        available: true,
        detail: "test probe detail".to_owned(),
        capabilities: ProviderCapabilities {
            schema_version: AUTOMATION_SCHEMA_VERSION,
            full_system_vm: true,
            cow_overlay: true,
            hardware_acceleration,
            accelerators: accelerators
                .iter()
                .map(|value| (*value).to_owned())
                .collect(),
            guest_os: guest_os.iter().map(|value| (*value).to_owned()).collect(),
            guest_arch: guest_arch.iter().map(|value| (*value).to_owned()).collect(),
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

fn job_spec_source(argument: &str) -> String {
    format!(
        r#"schema_version = 1
image = "reliability-model-image"
cpu_count = 2
memory_mib = 2048
provider = "qemu"
accelerator = "kvm"
readiness_timeout_seconds = 30
action_timeout_seconds = 45
max_output_bytes = 4096

[command]
program = "secret-program"
args = ["{argument}"]

[cleanup]
keep = false
keep_on_failure = true

[[copy_in]]
source = "inputs/private.txt"
destination = "inputs/private.txt"

[artifacts]
sources = ["results/private.txt"]
"#
    )
}

fn write_private(path: &Path, source: &str) {
    fs::write(path, source).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
    }
}
