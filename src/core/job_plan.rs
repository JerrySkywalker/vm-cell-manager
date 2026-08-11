//! Read-only resolution for one validated reproducible job specification.
//!
//! The output is deliberately descriptive.  It binds a plan to the exact
//! specification bytes and current read-only capability evidence, but does not
//! authorize a lifecycle action or contain command, credential, or host-path
//! data.

use serde::Serialize;

use super::image::ImageRecord;
use super::job_spec::{JOB_SPEC_CONTRACT, JobAccelerator, LoadedJobSpec};
use super::run_selection::{
    HostPlatform, RequestedAccelerator, RunExecutionPlan, RunSelectionError, RunSelectionIntent,
    resolve_run_execution_plan,
};
use crate::providers::ProviderProbe;

pub const JOB_PLAN_SCHEMA_VERSION: u32 = 1;
pub const JOB_PLAN_CONTRACT: &str = "vmcell.job-plan.v1";

/// Safe projection of a fully resolved execution job.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ResolvedJobPlan {
    pub schema_version: u32,
    pub contract: String,
    pub authorizing: bool,
    pub job_spec_contract: String,
    pub job_spec_schema_version: u32,
    pub job_spec_sha256: String,
    pub execution: RunExecutionPlan,
    pub resources: JobResourcePlan,
    pub timeouts: JobTimeoutPlan,
    pub cleanup: JobCleanupPlan,
    pub declared_copy_in_count: usize,
    pub declared_artifact_count: usize,
}

/// Normalized resource policy.  This is informative only and contains no
/// state-root, host, provider, or guest secrets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct JobResourcePlan {
    pub cpu_count: u16,
    pub memory_mib: u64,
    pub ttl_seconds: Option<u64>,
}

/// Normalized guest-operation timing policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct JobTimeoutPlan {
    pub readiness_timeout_seconds: u64,
    pub action_timeout_seconds: u64,
    pub max_output_bytes: u64,
}

/// The safe cleanup policy requested by the declarative input.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct JobCleanupPlan {
    pub keep: bool,
    pub keep_on_failure: bool,
}

/// Resolve a job through the existing provider-neutral selector.  The caller
/// supplies read-only image and capability observations; this helper itself
/// does not access state, providers, or the filesystem.
pub fn resolve_job_plan(
    loaded: &LoadedJobSpec,
    host: HostPlatform,
    image: &ImageRecord,
    probes: &[ProviderProbe],
) -> Result<ResolvedJobPlan, RunSelectionError> {
    let spec = loaded.spec();
    let execution = resolve_run_execution_plan(
        host,
        image,
        probes,
        RunSelectionIntent {
            explicit_provider: spec.provider,
            // A job document is self-contained.  It must not inherit a hidden
            // mutable user preference when its provider is omitted.
            config_provider_preference: None,
            explicit_accelerator: spec.accelerator.map(requested_accelerator),
            allow_tcg: spec.allow_tcg,
        },
    )?;
    Ok(ResolvedJobPlan {
        schema_version: JOB_PLAN_SCHEMA_VERSION,
        contract: JOB_PLAN_CONTRACT.to_owned(),
        authorizing: false,
        job_spec_contract: JOB_SPEC_CONTRACT.to_owned(),
        job_spec_schema_version: spec.schema_version,
        job_spec_sha256: loaded.source_sha256().to_owned(),
        execution,
        resources: JobResourcePlan {
            cpu_count: spec.cpu_count,
            memory_mib: spec.memory_mib,
            ttl_seconds: spec.ttl_seconds,
        },
        timeouts: JobTimeoutPlan {
            readiness_timeout_seconds: spec.readiness_timeout_seconds,
            action_timeout_seconds: spec.action_timeout_seconds,
            max_output_bytes: spec.max_output_bytes,
        },
        cleanup: JobCleanupPlan {
            keep: spec.cleanup.keep,
            keep_on_failure: spec.cleanup.keep_on_failure,
        },
        declared_copy_in_count: spec.copy_in.len(),
        declared_artifact_count: spec.artifacts.sources.len(),
    })
}

const fn requested_accelerator(accelerator: JobAccelerator) -> RequestedAccelerator {
    match accelerator {
        JobAccelerator::Whpx => RequestedAccelerator::Whpx,
        JobAccelerator::Kvm => RequestedAccelerator::Kvm,
        JobAccelerator::Hvf => RequestedAccelerator::Hvf,
        JobAccelerator::Tcg => RequestedAccelerator::Tcg,
    }
}

#[cfg(test)]
mod tests {
    use chrono::Utc;

    use super::*;
    use crate::core::automation::AUTOMATION_SCHEMA_VERSION;
    use crate::core::capability::ProviderCapabilities;
    use crate::core::image::{Architecture, ImageId, ImageVariant};
    use crate::core::support::{GuestTransportId, HostOs, SupportStatus};
    use crate::providers::ProviderProbeStatus;

    const SPEC: &str = r#"
schema_version = 1
image = "linux-qemu"
cpu_count = 2
memory_mib = 2048
provider = "qemu"
accelerator = "kvm"
readiness_timeout_seconds = 30
action_timeout_seconds = 45
max_output_bytes = 4096

[command]
program = "secret-program-must-not-appear"
args = ["secret-argument-must-not-appear"]

[cleanup]
keep = false
keep_on_failure = true

[[copy_in]]
source = "inputs/message.txt"
destination = "inputs/message.txt"

[artifacts]
sources = ["results/output.txt"]
"#;

    fn loaded_spec() -> LoadedJobSpec {
        LoadedJobSpec::from_validated_parts_for_test(
            "job.toml".into(),
            "a".repeat(64),
            super::super::job_spec::parse_job_spec(SPEC).unwrap(),
        )
    }

    fn image() -> ImageRecord {
        ImageRecord {
            schema_version: 1,
            id: ImageId::parse("linux-qemu").unwrap(),
            guest_os: crate::core::image::GuestOs::Linux,
            guest_arch: Architecture::X86_64,
            variants: vec![ImageVariant {
                provider: "qemu".to_owned(),
                disk_format: "qcow2".to_owned(),
                path: "immutable.qcow2".into(),
                sha256: "a".repeat(64),
                file_size: 1024,
            }],
            registered_at: Utc::now(),
        }
    }

    fn qemu_probe() -> ProviderProbe {
        ProviderProbe {
            name: "qemu",
            status: ProviderProbeStatus::Ready,
            available: true,
            detail: "test evidence".to_owned(),
            capabilities: ProviderCapabilities {
                schema_version: AUTOMATION_SCHEMA_VERSION,
                full_system_vm: true,
                cow_overlay: true,
                hardware_acceleration: true,
                accelerators: vec!["kvm".to_owned()],
                guest_os: vec!["linux".to_owned()],
                guest_arch: vec!["x86_64".to_owned()],
                guest_transports: vec!["qga".to_owned()],
                networkless_guest_exec: true,
            },
        }
    }

    #[test]
    fn resolves_through_existing_selection_without_authority_or_secret_fields() {
        let plan = resolve_job_plan(
            &loaded_spec(),
            HostPlatform {
                os: HostOs::Linux,
                architecture: Architecture::X86_64,
            },
            &image(),
            &[qemu_probe()],
        )
        .unwrap();
        assert_eq!(plan.schema_version, JOB_PLAN_SCHEMA_VERSION);
        assert_eq!(plan.contract, JOB_PLAN_CONTRACT);
        assert!(!plan.authorizing);
        assert_eq!(plan.execution.guest_transport, GuestTransportId::Qga);
        assert_eq!(plan.execution.support_status, SupportStatus::Untested);
        assert_eq!(plan.resources.cpu_count, 2);
        assert_eq!(plan.timeouts.action_timeout_seconds, 45);
        assert_eq!(plan.declared_copy_in_count, 1);
        assert_eq!(plan.declared_artifact_count, 1);

        let encoded = serde_json::to_string(&plan).unwrap();
        for forbidden in [
            "secret-program-must-not-appear",
            "secret-argument-must-not-appear",
            "inputs/message.txt",
            "results/output.txt",
            "job.toml",
            "test evidence",
        ] {
            assert!(!encoded.contains(forbidden), "leaked {forbidden}");
        }
    }

    #[test]
    fn absent_spec_provider_uses_native_selection_not_a_config_preference() {
        let mut spec = super::super::job_spec::parse_job_spec(SPEC).unwrap();
        spec.provider = None;
        spec.accelerator = None;
        let loaded =
            LoadedJobSpec::from_validated_parts_for_test("job.toml".into(), "a".repeat(64), spec);
        let plan = resolve_job_plan(
            &loaded,
            HostPlatform {
                os: HostOs::Linux,
                architecture: Architecture::X86_64,
            },
            &image(),
            &[qemu_probe()],
        )
        .unwrap();
        assert_eq!(
            plan.execution.selection_source,
            crate::core::run_selection::RunSelectionSource::NativeDefault
        );
    }
}
