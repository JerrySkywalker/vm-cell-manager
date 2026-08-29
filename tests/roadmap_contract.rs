use vm_cell_manager::core::image::{Architecture, GuestOs};
use vm_cell_manager::core::support::{
    Accelerator, GuestTransportId, HostOs, ProviderId, SUPPORT_MATRIX, SupportKey,
    SupportLookupError, SupportStatus, support_for,
};

const ROADMAP: &str = include_str!("../docs/roadmap.md");
const ACCEPTANCE_MATRIX: &str = include_str!("../docs/release-acceptance-matrix.md");
const SUPPORT_MATRIX_DOC: &str = include_str!("../docs/support-matrix.md");
const CONTRIBUTING: &str = include_str!("../CONTRIBUTING.md");
const ADR_0018: &str =
    include_str!("../docs/adr/0018-defer-macos-hvf-and-resequence-pre-v1-roadmap.md");

fn section<'a>(text: &'a str, start: &str, end: &str) -> &'a str {
    let start = text
        .find(start)
        .unwrap_or_else(|| panic!("contract omitted section start: {start}"));
    let remainder = &text[start..];
    let end = remainder
        .find(end)
        .unwrap_or_else(|| panic!("contract omitted section end: {end}"));
    &remainder[..end]
}

fn assert_no_apple_dependency(text: &str, scope: &str) {
    let normalized = text.to_ascii_lowercase();
    for term in [
        "macos",
        "apple silicon",
        "intel mac",
        "hvf",
        "virtualization.framework",
    ] {
        assert!(
            !normalized.contains(term),
            "{scope} contains active Apple dependency term {term}"
        );
    }
}

fn normalized_whitespace(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[test]
fn active_v05_milestone_does_not_require_an_apple_platform() {
    let v05 = section(ROADMAP, "# v0.5.0", "# v0.6.0");
    assert_no_apple_dependency(v05, "v0.5");
    assert!(v05.contains("Windows x86_64 and native Linux x86_64"));
    for slice in [
        "### A. Active Windows/Linux platform contract",
        "### B. Two-host workflow and distribution parity",
        "### C. Audit and closeout",
    ] {
        assert!(v05.contains(slice), "v0.5 omitted sequential slice {slice}");
    }
}

#[test]
fn production_support_matrix_has_no_active_macos_row() {
    assert!(
        SUPPORT_MATRIX
            .iter()
            .all(|entry| entry.key.host_os != HostOs::Macos),
        "production support matrix contains a macOS host row"
    );
    assert!(!SUPPORT_MATRIX_DOC.contains("| macos |"));
}

#[test]
fn active_acceptance_matrix_has_no_macos_packet_or_gate() {
    let active = ACCEPTANCE_MATRIX
        .split("## Post-v1 deferred Apple direction")
        .next()
        .expect("acceptance matrix must have an active section");
    assert_no_apple_dependency(active, "active acceptance matrix");
    assert!(!active.contains("v0.5 planning base"));
    assert!(!active.contains("Apple-Silicon observe-only preflight"));
}

#[test]
fn host_os_macos_remains_modeled() {
    assert_eq!(HostOs::Macos.as_str(), "macos");
    assert!(ADR_0018.contains("`HostOs::Macos`"));
}

#[test]
fn accelerator_hvf_remains_modeled() {
    assert_eq!(Accelerator::Hvf.as_str(), "hvf");
    assert!(Accelerator::Hvf.is_hardware());
    assert!(ADR_0018.contains("`Accelerator::Hvf`"));
}

#[test]
fn absent_macos_tuples_fail_closed() {
    for accelerator in [Accelerator::Hvf, Accelerator::Tcg] {
        let key = SupportKey {
            host_os: HostOs::Macos,
            host_architecture: Architecture::X86_64,
            provider: ProviderId::Qemu,
            accelerator,
            guest_os: GuestOs::Linux,
            guest_architecture: Architecture::X86_64,
            guest_transport: GuestTransportId::Qga,
        };
        assert_eq!(
            support_for(&key),
            Err(SupportLookupError::UndocumentedCombination),
            "absent macOS {accelerator:?} tuple did not fail closed"
        );
    }
}

#[test]
fn tcg_rows_are_exact_development_only_and_evidence_free() {
    let tcg_rows = SUPPORT_MATRIX
        .iter()
        .filter(|entry| entry.key.accelerator == Accelerator::Tcg)
        .collect::<Vec<_>>();
    let expected_keys = [
        SupportKey {
            host_os: HostOs::Windows,
            host_architecture: Architecture::X86_64,
            provider: ProviderId::Qemu,
            accelerator: Accelerator::Tcg,
            guest_os: GuestOs::Linux,
            guest_architecture: Architecture::X86_64,
            guest_transport: GuestTransportId::Qga,
        },
        SupportKey {
            host_os: HostOs::Linux,
            host_architecture: Architecture::X86_64,
            provider: ProviderId::Qemu,
            accelerator: Accelerator::Tcg,
            guest_os: GuestOs::Linux,
            guest_architecture: Architecture::X86_64,
            guest_transport: GuestTransportId::Qga,
        },
    ];
    assert_eq!(
        tcg_rows.iter().map(|entry| entry.key).collect::<Vec<_>>(),
        expected_keys
    );
    for entry in tcg_rows {
        assert_eq!(entry.status, SupportStatus::DevelopmentOnly);
        assert!(entry.acceptance_evidence.is_empty());
    }
}

#[test]
fn issue_48_link_uses_the_canonical_repository_owner() {
    assert!(ROADMAP.contains("https://github.com/JerrySkywalker/vm-cell-manager/issues/48"));
}

#[test]
fn v08_requires_exact_candidate_specific_reliability_evidence() {
    let v08 = section(ROADMAP, "# v0.8.0", "# v0.9.0");
    let v08 = normalized_whitespace(v08);
    for obligation in [
        "bounded, candidate-specific evidence",
        "repeated/soak operation with fixed duration, case-count, and output bounds",
        "unknown-effect operations",
        "concurrency, ownership, cleanup, and preservation of foreign state",
        "resource-growth and leak observations",
        "evidence signals, not universal SLOs",
        "upgrade, rollback, and durable-state recovery rehearsal",
        "exact candidate, package, tuple, host, image, and guest identity",
        "never silently deletes user state, images, cells",
    ] {
        assert!(v08.contains(obligation), "v0.8 omitted {obligation}");
    }
    assert!(v08.contains("completed repository-local evidence is reusable foundation"));
    assert!(v08.contains("is not candidate-specific real-platform evidence"));
    assert!(v08.contains("does not create a v0.8 release or support claim"));
    assert!(
        v08.contains(
            "CI, mocks, WSL2, and static evidence cannot replace real-platform acceptance"
        )
    );
}

#[test]
fn issue_43_history_is_preserved_by_the_governance_plan() {
    let decision = normalized_whitespace(ADR_0018);
    for obligation in [
        "Its title, body, comments, and timeline must not be retitled, rewritten",
        "After PR #77 is merged and exact-dev CI is green",
        "Close Issue #43 as `not_planned` or superseded",
        "Create a new **v0.5 Windows/Linux Portability Closeout** issue",
        "Update Issues #44, #45, #49, and #50 to depend on the new issue",
        "was deferred, not completed or accepted",
        "remain post-v1, non-blocking future direction",
    ] {
        assert!(
            decision.contains(obligation),
            "ADR 0018 omitted {obligation}"
        );
    }
}

#[test]
fn contributing_separates_generic_live_and_post_v1_provider_tests() {
    let guidance = normalized_whitespace(CONTRIBUTING);
    for obligation in [
        "Generic repository validation consists of fixture, static, and unit tests",
        "does not perform live Hyper-V, WHPX, KVM, QEMU, or HVF provider access",
        "Live provider tests are separate, explicitly admitted real-platform work",
        "active pre-v1 paths are Windows x86_64",
        "native Linux x86_64 with QEMU/KVM",
        "macOS and HVF remain modeled as post-v1, non-blocking future direction",
        "not a permanent rejection of Apple support",
    ] {
        assert!(
            guidance.contains(obligation),
            "CONTRIBUTING omitted {obligation}"
        );
    }
}

#[test]
fn v05_is_windows_and_linux_portability_closeout() {
    assert!(ROADMAP.contains("# v0.5.0 — Windows and Linux Portability Closeout"));
    assert!(ADR_0018.contains("**Windows and Linux Portability Closeout**"));
}

#[test]
fn v06_through_v10_do_not_depend_on_apple_hardware() {
    let later_pre_v1 = section(ROADMAP, "# v0.6.0", "# Post-v1 direction");
    assert_no_apple_dependency(later_pre_v1, "v0.6 through v1.0");
    let later_pre_v1 = normalized_whitespace(later_pre_v1);
    assert!(later_pre_v1.contains("Reliability packets A-G are already"));
    assert!(later_pre_v1.contains("completed repository-local evidence is reusable foundation"));
    assert!(later_pre_v1.contains("does not reopen the completed repository-local A-G work"));
}

#[test]
fn all_four_frozen_v041_r5_rows_remain_not_executed() {
    let rows = ACCEPTANCE_MATRIX
        .lines()
        .filter(|line| {
            line.starts_with("| v0.4.1 ")
                && line.contains("0e7fcf37f4310562d318f9d5c709ddf8e8ca1637")
        })
        .collect::<Vec<_>>();
    assert_eq!(rows.len(), 4, "frozen v0.4.1 R5 row count drifted");
    for packet in [
        "V041-R5-HYPERV-PSD-V1",
        "V041-R5-WHPX-QGA-V1",
        "V041-R5-KVM-QGA-V1",
        "V041-R5-JOBSPEC-OVERLAY-V1",
    ] {
        let row = rows
            .iter()
            .find(|row| row.contains(packet))
            .unwrap_or_else(|| panic!("frozen v0.4.1 register omitted {packet}"));
        assert!(row.contains("`NOT_EXECUTED`"), "{packet} status drifted");
    }
}

#[test]
fn cargo_package_version_remains_v041() {
    assert_eq!(env!("CARGO_PKG_VERSION"), "0.4.1");
}

#[test]
fn no_support_row_is_promoted() {
    assert!(SUPPORT_MATRIX.iter().all(|entry| !matches!(
        entry.status,
        SupportStatus::Supported | SupportStatus::Experimental
    )));
}
