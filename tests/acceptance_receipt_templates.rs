use serde_json::Value;

const WINDOWS_WHPX_TEMPLATE: &str =
    include_str!("../docs/receipts/windows-whpx-acceptance-template.json");
const LINUX_KVM_TEMPLATE: &str =
    include_str!("../docs/receipts/linux-kvm-acceptance-template.json");
const ACCEPTANCE_MATRIX: &str = include_str!("../docs/release-acceptance-matrix.md");
const OWNER_PACKET_TEMPLATE: &str =
    include_str!("../docs/receipts/real-platform-owner-packet-template.md");
const ACCEPTANCE_VALIDATOR_DOC: &str = include_str!("../docs/acceptance-receipt-validator.md");

fn template(name: &str, source: &str) -> Value {
    serde_json::from_str(source)
        .unwrap_or_else(|error| panic!("{name} must be valid JSON: {error}"))
}

fn required<'a>(value: &'a Value, pointer: &str) -> &'a Value {
    value
        .pointer(pointer)
        .unwrap_or_else(|| panic!("template omitted required field {pointer}"))
}

fn required_string<'a>(value: &'a Value, pointer: &str) -> &'a str {
    required(value, pointer)
        .as_str()
        .unwrap_or_else(|| panic!("template field {pointer} must be a string"))
}

fn required_placeholder(value: &Value, pointer: &str) {
    let observed = required_string(value, pointer);
    assert!(
        observed.starts_with("REQUIRED_"),
        "template field {pointer} must use a required placeholder, got {observed:?}"
    );
}

fn assert_no_forbidden_disclosure_keys(value: &Value) {
    match value {
        Value::Array(values) => {
            for value in values {
                assert_no_forbidden_disclosure_keys(value);
            }
        }
        Value::Object(values) => {
            for (key, value) in values {
                let normalized: String = key
                    .chars()
                    .filter(char::is_ascii_alphanumeric)
                    .flat_map(char::to_lowercase)
                    .collect();
                assert!(
                    ![
                        "password",
                        "credential",
                        "secret",
                        "commandargv",
                        "guestoutput",
                    ]
                    .iter()
                    .any(|forbidden| normalized.contains(forbidden)),
                    "receipt template must not model disclosure field {key}"
                );
                assert_no_forbidden_disclosure_keys(value);
            }
        }
        Value::String(value) => {
            assert!(
                value == "/dev/kvm"
                    || !["C:\\", "\\\\", "/Users/", "/home/", "/var/",]
                        .iter()
                        .any(|prefix| value.starts_with(prefix)),
                "receipt template must not embed a raw host path: {value:?}"
            );
        }
        _ => {}
    }
}

fn assert_common_qemu_template(value: &Value, contract: &str) {
    assert_eq!(required(value, "/schema_version"), 1);
    assert_eq!(required_string(value, "/contract"), contract);
    assert_eq!(required(value, "/authorizing"), false);
    assert_eq!(
        required_string(value, "/real_platform_acceptance"),
        "pending"
    );
    assert_eq!(
        required_string(value, "/repository/slug"),
        "JerrySkywalker/vm-cell-manager"
    );
    assert_eq!(
        required_string(value, "/repository/candidate_sha"),
        "REQUIRED_EXACT_40_HEX_SHA"
    );
    for field in [
        "/candidate_binary/version",
        "/candidate_binary/sha256",
        "/candidate_binary/checkout_clean",
        "/preflight_receipt_sha256",
    ] {
        required_placeholder(value, field);
    }
    for executable in ["/qemu_system", "/qemu_img"] {
        required_placeholder(value, &format!("{executable}/canonical_path"));
        required_placeholder(value, &format!("{executable}/version"));
        required_placeholder(value, &format!("{executable}/sha256"));
    }
    assert_eq!(required_string(value, "/immutable_base/format"), "qcow2");
    assert!(required(value, "/immutable_base/backing_parent").is_null());
    assert!(required(value, "/qga").is_object());
    assert!(required(value, "/writer_exclusivity").is_object());
    assert_eq!(
        required_string(value, "/cleanup/policy"),
        "exact-owned-only"
    );
    assert_eq!(
        required_string(value, "/provider_path/support_status"),
        "untested"
    );
    assert_eq!(
        required_string(value, "/result"),
        "PENDING_REAL_PLATFORM_GATE"
    );
    assert_no_forbidden_disclosure_keys(value);
}

#[test]
fn windows_whpx_acceptance_template_is_complete_and_non_authorizing() {
    let value = template("windows WHPX acceptance", WINDOWS_WHPX_TEMPLATE);
    assert_common_qemu_template(&value, "vmcell.windows-whpx-acceptance.v1");
    assert_eq!(required_string(&value, "/host/os"), "windows");
    assert_eq!(required_string(&value, "/host/architecture"), "x86_64");
    assert_eq!(required_string(&value, "/provider_path/provider"), "qemu");
    assert_eq!(
        required_string(&value, "/provider_path/accelerator"),
        "whpx"
    );
    assert_eq!(required_string(&value, "/provider_path/guest_os"), "linux");
    assert_eq!(
        required_string(&value, "/provider_path/guest_architecture"),
        "x86_64"
    );
    assert_eq!(
        required_string(&value, "/provider_path/guest_transport"),
        "qga"
    );
    assert!(required(&value, "/foreign_qemu").is_object());
    assert!(required(&value, "/ownership").is_object());
    for field in [
        "/host/fingerprint_sha256",
        "/whpx/functional_evidence",
        "/state_root/canonical_path",
        "/state_root/identity_fingerprint",
        "/immutable_base/canonical_path",
        "/immutable_base/sha256_before",
        "/immutable_base/sha256_after",
        "/qga/readiness_evidence",
        "/foreign_qemu/prestate_fingerprint_sha256",
        "/foreign_qemu/poststate_fingerprint_sha256",
        "/foreign_qemu/unchanged",
        "/writer_exclusivity/evidence",
        "/writer_exclusivity/violations",
        "/ownership/exact_owned_namespace",
        "/ownership/process_identity",
        "/ownership/process_tree_containment",
        "/ownership/overlay_identity",
        "/ownership/qmp_identity",
        "/cleanup/rollback_evidence",
        "/cleanup/manual_review_retained",
    ] {
        required_placeholder(&value, field);
    }
    assert_eq!(required(&value, "/whpx/advertised_preflight"), true);
}

#[test]
fn linux_kvm_acceptance_template_is_complete_and_non_authorizing() {
    let value = template("Linux KVM acceptance", LINUX_KVM_TEMPLATE);
    assert_common_qemu_template(&value, "vmcell.linux-kvm-acceptance.v1");
    assert_eq!(required_string(&value, "/host/os"), "linux");
    assert_eq!(required_string(&value, "/host/architecture"), "x86_64");
    assert_eq!(required_string(&value, "/provider_path/provider"), "qemu");
    assert_eq!(required_string(&value, "/provider_path/accelerator"), "kvm");
    assert_eq!(required_string(&value, "/provider_path/guest_os"), "linux");
    assert_eq!(
        required_string(&value, "/provider_path/guest_architecture"),
        "x86_64"
    );
    assert_eq!(
        required_string(&value, "/provider_path/guest_transport"),
        "qga"
    );
    assert_eq!(
        required(&value, "/kvm/ioctl_or_vm_performed_by_preflight"),
        false
    );
    assert!(required(&value, "/foreign_prestate").is_object());
    assert!(required(&value, "/recovery").is_object());
    for field in [
        "/host/native_host_proof",
        "/host/fingerprint_sha256",
        "/host/effective_uid",
        "/kvm/device_inode_identity",
        "/kvm/read_write_open_by_effective_identity",
        "/kvm/functional_evidence",
        "/state_root/canonical_path",
        "/state_root/owner_uid",
        "/state_root/device_inode_identity",
        "/immutable_base/canonical_path",
        "/immutable_base/size_before",
        "/immutable_base/sha256_before",
        "/immutable_base/sha256_after",
        "/qga/readiness_evidence",
        "/foreign_prestate/qemu_process_fingerprint_before",
        "/foreign_prestate/qemu_process_fingerprint_after",
        "/foreign_prestate/runtime_tree_fingerprint_before",
        "/foreign_prestate/runtime_tree_fingerprint_after",
        "/foreign_prestate/network_fingerprint_before",
        "/foreign_prestate/network_fingerprint_after",
        "/foreign_prestate/unchanged_except_exact_owned_resources",
        "/writer_exclusivity/evidence",
        "/writer_exclusivity/violations",
        "/ownership/exact_owned_namespace",
        "/ownership/process_identity",
        "/ownership/process_group_identity",
        "/ownership/overlay_identity",
        "/ownership/qmp_identity",
        "/recovery/crash_reconciliation_evidence",
        "/recovery/manual_review_retained",
        "/cleanup/rollback_evidence",
        "/cleanup/foreign_state_unchanged",
    ] {
        required_placeholder(&value, field);
    }
}

#[test]
fn release_acceptance_matrix_binds_every_current_release_without_promotion() {
    let row = |candidate: &str, tuple: &str| {
        ACCEPTANCE_MATRIX
            .lines()
            .find(|line| {
                line.starts_with("| v0.") && line.contains(candidate) && line.contains(tuple)
            })
            .unwrap_or_else(|| panic!("acceptance matrix omitted row {candidate} / {tuple}"))
    };
    for (candidate, tuple, status) in [
        (
            "32f4adad3881c5248c6c8c5d47982368b7b55799",
            "Windows/x86_64 + Hyper-V + Windows/x86_64 + PowerShell Direct",
            "PENDING_REAL_PLATFORM_GATE",
        ),
        (
            "ed2ed31ae2f0182fc1626321b81e86d09db378c2",
            "Windows/x86_64 + Hyper-V + Windows/x86_64 + PowerShell Direct",
            "PENDING_REAL_PLATFORM_GATE",
        ),
        (
            "d0af04b2e84cf2226628173d2ed0d295aed01f2b",
            "Windows/x86_64 + QEMU/WHPX + Linux/x86_64 + credentialless QGA",
            "PENDING_REAL_PLATFORM_GATE",
        ),
        (
            "d0af04b2e84cf2226628173d2ed0d295aed01f2b",
            "Native Linux/x86_64 + QEMU/KVM + Linux/x86_64 + credentialless QGA",
            "PENDING_REAL_PLATFORM_GATE",
        ),
        (
            "c741be99ef4632b436f394f1c53b71ed57d0d2d9",
            "Overlay on an independently accepted v0.1/v0.3 tuple",
            "PENDING_REAL_PLATFORM_GATE",
        ),
        (
            "c741be99ef4632b436f394f1c53b71ed57d0d2d9",
            "macOS/Apple Silicon/aarch64 + QEMU/HVF + Linux/aarch64 + credentialless QGA",
            "BLOCKED_EXTERNAL",
        ),
    ] {
        assert!(row(candidate, tuple).contains(status));
    }
    assert!(ACCEPTANCE_MATRIX.contains("issue #43"));
    assert!(
        ACCEPTANCE_MATRIX
            .contains("A later release does not retroactively accept an older frozen candidate.")
    );
    assert!(
        ACCEPTANCE_MATRIX.contains("does not create a `supported` or `experimental` support row")
    );
}

#[test]
fn owner_packet_template_is_sanitized_and_non_authorizing() {
    for required_text in [
        "contract: vmcell.real-platform-owner-packet.v1",
        "authorizing: false",
        "real_platform_acceptance: pending",
        "result: NOT_EXECUTED",
        "candidate_sha: REQUIRED_EXACT_40_HEX_SHA",
        "candidate_binary_version: REQUIRED_VERSION",
        "candidate_binary_sha256: REQUIRED_SHA256",
        "candidate_checkout_clean: REQUIRED_BOOLEAN",
        "mode: observe-only-preflight | authorized-real-run",
        "operator_identity_fingerprint: REQUIRED_SANITIZED_HASH_OR_ID",
        "authorization_evidence_id: REQUIRED_OPAQUE_ID_OR_NOT_APPLICABLE",
        "isolation_evidence_id: REQUIRED_OPAQUE_ID",
        "cell_id: REQUIRED_CELL_ID_OR_NOT_APPLICABLE",
        "provider_object_identity: REQUIRED_SANITIZED_PROVIDER_ID_OR_NOT_APPLICABLE",
        "runtime_receipt_identity: REQUIRED_SANITIZED_HASH_OR_ID_OR_NOT_APPLICABLE",
        "unknown_guest_effect_replayed: false",
        "cleanup_policy: exact-owned-only",
        "v0.1 baseline lifecycle and PowerShell Direct",
        "v0.2 repeated session/image/state behavior",
        "v0.3 Windows WHPX or native Linux KVM QGA path",
        "v0.4 JobSpec/result correlation",
        "v0.5 Apple-Silicon observe-only preflight",
        "`PREFLIGHT_PASS`, `PASS`, `PARTIAL`, `BLOCKED_EXTERNAL`, or",
        "real_platform_acceptance: pending`",
        "with `completed` only for a completed authorized run",
        "Only `authorized-real-run` may report `PASS`",
        "`observe-only-preflight` reports `PREFLIGHT_PASS`",
    ] {
        assert!(
            OWNER_PACKET_TEMPLATE.contains(required_text),
            "owner packet template omitted required contract field: {required_text}"
        );
    }
    for prohibited_text in ["password:", "credential:", "command_argv:", "guest_output:"] {
        assert!(
            !OWNER_PACKET_TEMPLATE.contains(prohibited_text),
            "owner packet template must not model disclosure field {prohibited_text}"
        );
    }
}

#[test]
fn offline_acceptance_validator_document_retains_the_real_platform_boundary() {
    for required_text in [
        "vmcell receipt validate",
        "vmcell.acceptance-receipt-validation-request.v1",
        "authorizing: false",
        "support_promotion: \"not_evaluated\"",
        "preflight can never be relabelled as `PASS`",
        "does not mark a support row supported or experimental",
        "does not contact a host, GitHub, a provider, or a guest",
        "release/v0.3.0@d0af04b2e84cf2226628173d2ed0d295aed01f2b",
    ] {
        assert!(
            ACCEPTANCE_VALIDATOR_DOC.contains(required_text),
            "acceptance validator documentation omitted required boundary: {required_text}"
        );
    }
}
