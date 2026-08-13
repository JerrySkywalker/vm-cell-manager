use serde_json::Value;

const WINDOWS_WHPX_TEMPLATE: &str =
    include_str!("../docs/receipts/windows-whpx-acceptance-template.json");
const LINUX_KVM_TEMPLATE: &str =
    include_str!("../docs/receipts/linux-kvm-acceptance-template.json");
const ACCEPTANCE_MATRIX: &str = include_str!("../docs/release-acceptance-matrix.md");
const OWNER_PACKET_TEMPLATE: &str =
    include_str!("../docs/receipts/real-platform-owner-packet-template.md");
const ACCEPTANCE_VALIDATOR_DOC: &str = include_str!("../docs/acceptance-receipt-validator.md");
const V041_R5_REHEARSAL: &str = include_str!("../docs/receipts/v041-r5-contract-rehearsal.json");

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
        "/ownership/windows_job/job_identity_sha256",
        "/ownership/windows_job/receipt_persisted_before_resume",
        "/ownership/windows_job/leader_pid",
        "/ownership/windows_job/leader_start_token_sha256",
        "/ownership/windows_job/pre_cleanup_active_processes",
        "/ownership/windows_job/terminal_active_processes",
        "/ownership/windows_job/terminal_process_id_count",
        "/ownership/windows_job/empty_tree_evidence",
        "/ownership/overlay_identity",
        "/ownership/qmp_identity",
        "/cleanup/rollback_evidence",
        "/cleanup/manual_review_retained",
    ] {
        required_placeholder(&value, field);
    }
    assert_eq!(required(&value, "/ownership/windows_job/config_schema"), 2);
    assert_eq!(
        required_string(&value, "/ownership/windows_job/atomic_create_method"),
        "PROC_THREAD_ATTRIBUTE_JOB_LIST_CREATE_SUSPENDED"
    );
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
fn release_acceptance_matrix_retires_frozen_candidates_without_promotion() {
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
            "RETIRED_CORRECTION_REQUIRED",
        ),
        (
            "ed2ed31ae2f0182fc1626321b81e86d09db378c2",
            "Windows/x86_64 + Hyper-V + Windows/x86_64 + PowerShell Direct",
            "RETIRED_CORRECTION_REQUIRED",
        ),
        (
            "d0af04b2e84cf2226628173d2ed0d295aed01f2b",
            "Windows/x86_64 + QEMU/WHPX + Linux/x86_64 + credentialless QGA",
            "RETIRED_CORRECTION_REQUIRED",
        ),
        (
            "d0af04b2e84cf2226628173d2ed0d295aed01f2b",
            "Native Linux/x86_64 + QEMU/KVM + Linux/x86_64 + credentialless QGA",
            "RETIRED_CORRECTION_REQUIRED",
        ),
        (
            "c741be99ef4632b436f394f1c53b71ed57d0d2d9",
            "Overlay on an independently accepted v0.1/v0.3 tuple",
            "RETIRED_CORRECTION_REQUIRED",
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

#[test]
fn v041_r5_rehearsal_binds_four_exact_non_authorizing_owner_handoffs() {
    let rehearsal = template("v0.4.1 R5 rehearsal", V041_R5_REHEARSAL);
    assert_eq!(required(&rehearsal, "/schema_version"), 1);
    assert_eq!(
        required_string(&rehearsal, "/contract"),
        "vmcell.v041-r5-contract-rehearsal.v1"
    );
    assert_eq!(required(&rehearsal, "/authorizing"), false);
    assert_eq!(
        required_string(&rehearsal, "/candidate/release_ref"),
        "release/v0.4.1"
    );
    assert_eq!(
        required_string(&rehearsal, "/candidate/sha"),
        "0e7fcf37f4310562d318f9d5c709ddf8e8ca1637"
    );
    assert_eq!(
        required_string(&rehearsal, "/candidate/tree"),
        "18c2e81acc4db57e2275175b138d31049df000da"
    );
    assert_eq!(required_string(&rehearsal, "/candidate/version"), "0.4.1");
    assert_eq!(required(&rehearsal, "/candidate/immutable"), true);
    assert_eq!(required_string(&rehearsal, "/dry_run_result"), "PASS");
    assert_eq!(required_string(&rehearsal, "/r5_result"), "NOT_EXECUTED");
    assert_eq!(
        required_string(&rehearsal, "/support_promotion"),
        "not_evaluated"
    );

    let packets = required(&rehearsal, "/packets")
        .as_array()
        .expect("v0.4.1 R5 packets must be an array");
    assert_eq!(packets.len(), 4);
    let expected = [
        (
            "V041-R5-HYPERV-PSD-V1",
            "windows|x86_64|hyperv|none|windows|x86_64|powershell-direct",
        ),
        (
            "V041-R5-WHPX-QGA-V1",
            "windows|x86_64|qemu|whpx|linux|x86_64|qga",
        ),
        (
            "V041-R5-KVM-QGA-V1",
            "linux|x86_64|qemu|kvm|linux|x86_64|qga",
        ),
        (
            "V041-R5-JOBSPEC-OVERLAY-V1",
            "inherited-from-one-exact-base-packet|inherited-from-one-exact-base-packet|inherited-from-one-exact-base-packet|inherited-from-one-exact-base-packet|inherited-from-one-exact-base-packet|inherited-from-one-exact-base-packet|inherited-from-one-exact-base-packet",
        ),
    ];
    for (index, (packet_id, tuple)) in expected.into_iter().enumerate() {
        let packet = &packets[index];
        assert_eq!(required_string(packet, "/packet_id"), packet_id);
        assert_eq!(required_string(packet, "/packet_status"), "NOT_EXECUTED");
        assert_eq!(required_string(packet, "/support_status"), "untested");
        let actual_tuple = [
            required_string(packet, "/tuple/host_os"),
            required_string(packet, "/tuple/host_architecture"),
            required_string(packet, "/tuple/provider"),
            required_string(packet, "/tuple/accelerator"),
            required_string(packet, "/tuple/guest_os"),
            required_string(packet, "/tuple/guest_architecture"),
            required_string(packet, "/tuple/guest_transport"),
        ]
        .join("|");
        assert_eq!(actual_tuple, tuple, "{packet_id} tuple drifted");
        assert!(
            required(packet, "/minimum_dedicated_host_prerequisites")
                .as_array()
                .is_some_and(|values| values.len() >= 7),
            "{packet_id} omitted minimum dedicated-host prerequisites"
        );
        for (authority, profile, mode, ceiling) in [
            (
                "a4",
                "PROTECTED_PREFLIGHT_V2",
                "observe-only-preflight",
                "PREFLIGHT_PASS",
            ),
            (
                "a5",
                "PROTECTED_TRANSACTION_V2",
                "authorized-real-run",
                "PASS",
            ),
        ] {
            assert_eq!(
                required_string(packet, &format!("/{authority}/profile")),
                profile
            );
            assert_eq!(required_string(packet, &format!("/{authority}/mode")), mode);
            assert_eq!(
                required_string(packet, &format!("/{authority}/result_ceiling")),
                ceiling
            );
            let command = required_string(packet, &format!("/{authority}/one_command"));
            assert!(command.contains(packet_id));
            assert!(command.ends_with(&format!("-Authority {}", authority.to_uppercase())));
        }
    }

    let truth = required(&rehearsal, "/terminal_truth_table")
        .as_array()
        .expect("v0.4.1 R5 truth table must be an array");
    assert_eq!(truth.len(), 5);
    assert!(truth.iter().all(|row| {
        required_string(row, "/support_status") == "untested"
            && match required_string(row, "/result") {
                "PASS" => {
                    required_string(row, "/mode") == "authorized-real-run"
                        && required_string(row, "/real_platform_acceptance") == "completed"
                }
                "PREFLIGHT_PASS" => {
                    required_string(row, "/mode") == "observe-only-preflight"
                        && required_string(row, "/real_platform_acceptance") == "pending"
                }
                "PARTIAL" | "BLOCKED_EXTERNAL" | "OWNER_DECISION_REQUIRED" => {
                    required_string(row, "/real_platform_acceptance") == "pending"
                }
                _ => false,
            }
    }));
}
