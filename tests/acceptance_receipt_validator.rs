use serde_json::{Value, json};
use vm_cell_manager::core::acceptance_receipt::{
    AcceptanceReceiptDisposition, validate_acceptance_receipt_bytes,
};

const V03_SHA: &str = "d0af04b2e84cf2226628173d2ed0d295aed01f2b";

fn sha256(character: char) -> String {
    std::iter::repeat_n(character, 64).collect()
}

fn windows_tuple() -> Value {
    json!({
        "host_os": "windows",
        "host_architecture": "x86_64",
        "provider": "qemu",
        "accelerator": "whpx",
        "guest_os": "linux",
        "guest_architecture": "x86_64",
        "guest_transport": "qga"
    })
}

fn valid_request() -> Value {
    let binary_sha256 = sha256('a');
    let base_sha256 = sha256('b');
    let preflight_sha256 = sha256('c');
    let tuple = windows_tuple();
    json!({
        "schema_version": 1,
        "contract": "vmcell.acceptance-receipt-validation-request.v1",
        "expected_binding": {
            "release_ref": "release/v0.3.0",
            "candidate_sha": V03_SHA,
            "candidate_version": "0.3.0",
            "candidate_binary_sha256": binary_sha256,
            "preflight_receipt_sha256": preflight_sha256,
            "tuple": tuple,
            "immutable_base": {
                "format": "qcow2",
                "size_bytes": 1048576,
                "sha256": base_sha256
            }
        },
        "receipt": {
            "schema_version": 1,
            "contract": "vmcell.real-platform-acceptance-receipt.v1",
            "authorizing": false,
            "real_platform_acceptance": "completed",
            "result": "PASS",
            "repository": {
                "slug": "JerrySkywalker/vm-cell-manager",
                "release_ref": "release/v0.3.0",
                "candidate_sha": V03_SHA,
                "checkout_clean": true
            },
            "candidate_binary": {
                "version": "0.3.0",
                "sha256": binary_sha256
            },
            "tuple": tuple,
            "preflight": {
                "receipt_sha256": preflight_sha256,
                "evidence_id": "evidence-preflight-123"
            },
            "immutable_base": {
                "format": "qcow2",
                "size_bytes": 1048576,
                "sha256_before": base_sha256,
                "sha256_after": base_sha256,
                "backing_parent": null
            },
            "overlay": {
                "base_sha256": base_sha256,
                "identity_evidence_id": "evidence-overlay-123",
                "cleanup_completed": true
            },
            "run": {
                "mode": "authorized-real-run",
                "authorization_evidence_id": "evidence-authorize-123",
                "isolation_evidence_id": "evidence-isolate-123",
                "exact_owned_namespace": "evidence-namespace-123",
                "cell_id": "550e8400-e29b-41d4-a716-446655440000",
                "provider_object_evidence_id": "evidence-provider-123",
                "runtime_receipt_evidence_id": "evidence-runtime-123",
                "lifecycle_evidence_id": "evidence-lifecycle-123",
                "guest_evidence_id": "evidence-guest-123",
                "unknown_guest_effect_replayed": false
            },
            "cleanup": {
                "policy": "exact-owned-only",
                "evidence_id": "evidence-cleanup-123",
                "immutable_base_sha256_after": base_sha256,
                "foreign_state_unchanged": true,
                "manual_review_retained": false
            },
            "support_status": "untested",
            "support_promotion": "not_evaluated"
        }
    })
}

fn report(
    value: Value,
) -> vm_cell_manager::core::acceptance_receipt::AcceptanceReceiptValidationReport {
    validate_acceptance_receipt_bytes(&serde_json::to_vec(&value).unwrap())
}

#[test]
fn completed_v03_windows_receipt_requires_exact_supplied_binding_without_promotion() {
    let report = report(valid_request());

    assert!(report.document_valid);
    assert_eq!(report.disposition, AcceptanceReceiptDisposition::Pass);
    assert!(report.findings.is_empty());
    assert!(!report.authorizing);
    assert_eq!(report.support_promotion, "not_evaluated");
    assert_eq!(report.document_sha256.len(), 64);
}

#[test]
fn completed_v03_linux_receipt_is_a_distinct_exact_tuple() {
    let mut request = valid_request();
    let linux_tuple = json!({
        "host_os": "linux",
        "host_architecture": "x86_64",
        "provider": "qemu",
        "accelerator": "kvm",
        "guest_os": "linux",
        "guest_architecture": "x86_64",
        "guest_transport": "qga"
    });
    request["expected_binding"]["tuple"] = linux_tuple.clone();
    request["receipt"]["tuple"] = linux_tuple;

    let report = report(request);
    assert!(report.document_valid);
    assert_eq!(report.disposition, AcceptanceReceiptDisposition::Pass);
}

#[test]
fn validator_rejects_candidate_or_base_binding_drift_without_echoing_input() {
    let mut request = valid_request();
    request["receipt"]["repository"]["candidate_sha"] = Value::String("f".repeat(40));
    request["receipt"]["immutable_base"]["sha256_after"] = Value::String(sha256('d'));

    let report = report(request);
    assert!(!report.document_valid);
    assert_eq!(report.disposition, AcceptanceReceiptDisposition::Rejected);
    assert!(report.has_finding("receipt.candidate_binding_mismatch"));
    assert!(report.has_finding("receipt.base_binding_mismatch"));
    let rendered = serde_json::to_string(&report).unwrap();
    assert!(!rendered.contains(&"f".repeat(40)));
    assert!(!rendered.contains(&sha256('d')));
}

#[test]
fn current_development_or_an_unregistered_tuple_never_substitutes_for_a_frozen_candidate() {
    let mut development = valid_request();
    development["expected_binding"]["candidate_sha"] = Value::String("f".repeat(40));
    development["receipt"]["repository"]["candidate_sha"] = Value::String("f".repeat(40));
    let development_report = report(development);
    assert!(!development_report.document_valid);
    assert!(development_report.has_finding("receipt.candidate_binding_mismatch"));

    let mut wrong_tuple = valid_request();
    wrong_tuple["expected_binding"]["tuple"]["accelerator"] = Value::String("kvm".to_owned());
    wrong_tuple["receipt"]["tuple"]["accelerator"] = Value::String("kvm".to_owned());
    let tuple_report = report(wrong_tuple);
    assert!(!tuple_report.document_valid);
    assert!(tuple_report.has_finding("receipt.candidate_binding_mismatch"));
}

#[test]
fn preflight_and_non_pass_terminal_records_are_never_acceptance_passes() {
    let mut preflight = valid_request();
    preflight["receipt"]["result"] = Value::String("PREFLIGHT_PASS".to_owned());
    preflight["receipt"]["real_platform_acceptance"] = Value::String("pending".to_owned());
    preflight["receipt"]["run"]["mode"] = Value::String("observe-only-preflight".to_owned());
    preflight["receipt"]["run"]["authorization_evidence_id"] =
        Value::String("not_applicable".to_owned());
    preflight["receipt"]["run"]["cell_id"] = Value::Null;
    preflight["receipt"]["run"]["provider_object_evidence_id"] =
        Value::String("not_applicable".to_owned());
    preflight["receipt"]["run"]["runtime_receipt_evidence_id"] =
        Value::String("not_applicable".to_owned());
    preflight["receipt"]["run"]["lifecycle_evidence_id"] =
        Value::String("not_applicable".to_owned());
    preflight["receipt"]["run"]["guest_evidence_id"] = Value::String("not_applicable".to_owned());
    preflight["receipt"]["overlay"]["identity_evidence_id"] =
        Value::String("not_applicable".to_owned());
    preflight["receipt"]["overlay"]["cleanup_completed"] = Value::Bool(false);
    preflight["receipt"]["cleanup"]["evidence_id"] = Value::String("not_applicable".to_owned());

    let preflight_report = report(preflight);
    assert!(preflight_report.document_valid);
    assert_eq!(
        preflight_report.disposition,
        AcceptanceReceiptDisposition::PreflightOnly
    );
    assert!(preflight_report.has_finding("receipt.preflight_only"));

    let mut partial = valid_request();
    partial["receipt"]["result"] = Value::String("PARTIAL".to_owned());
    partial["receipt"]["real_platform_acceptance"] = Value::String("pending".to_owned());
    partial["receipt"]["cleanup"]["manual_review_retained"] = Value::Bool(true);
    let partial_report = report(partial);
    assert!(partial_report.document_valid);
    assert_eq!(
        partial_report.disposition,
        AcceptanceReceiptDisposition::TerminalNotPass
    );
    assert!(partial_report.has_finding("receipt.terminal_not_pass"));
}

#[test]
fn a_pass_cannot_claim_observe_only_preflight() {
    let mut request = valid_request();
    request["receipt"]["run"]["mode"] = Value::String("observe-only-preflight".to_owned());
    request["receipt"]["real_platform_acceptance"] = Value::String("pending".to_owned());

    let validation = report(request);
    assert!(!validation.document_valid);
    assert_eq!(
        validation.disposition,
        AcceptanceReceiptDisposition::Rejected
    );
    assert!(validation.has_finding("receipt.state_contradiction"));
}

#[test]
fn a_pass_cannot_replace_required_run_evidence_with_preflight_sentinels() {
    let mut request = valid_request();
    for pointer in [
        "/receipt/run/authorization_evidence_id",
        "/receipt/run/lifecycle_evidence_id",
        "/receipt/cleanup/evidence_id",
    ] {
        *request.pointer_mut(pointer).unwrap() = Value::String("not_applicable".to_owned());
    }

    let validation = report(request);
    assert!(!validation.document_valid);
    assert_eq!(
        validation.disposition,
        AcceptanceReceiptDisposition::Rejected
    );
    assert!(validation.has_finding("receipt.state_contradiction"));

    let opaque_evidence = [
        "/receipt/preflight/evidence_id",
        "/receipt/run/isolation_evidence_id",
        "/receipt/run/exact_owned_namespace",
        "/receipt/overlay/identity_evidence_id",
        "/receipt/run/authorization_evidence_id",
        "/receipt/run/provider_object_evidence_id",
        "/receipt/run/runtime_receipt_evidence_id",
        "/receipt/run/lifecycle_evidence_id",
        "/receipt/run/guest_evidence_id",
        "/receipt/cleanup/evidence_id",
    ];
    for sentinel in [
        "NOT-APPLICABLE",
        "NOT_EXECUTED",
        "PENDING_REAL_PLATFORM_GATE",
        "BLOCKED_EXTERNAL",
        "OWNER_DECISION_REQUIRED",
    ] {
        let mut forged = valid_request();
        for pointer in opaque_evidence {
            *forged.pointer_mut(pointer).unwrap() = Value::String(sentinel.to_owned());
        }
        let forged_validation = report(forged);
        assert!(!forged_validation.document_valid, "sentinel={sentinel}");
        assert!(
            forged_validation.has_finding("receipt.required_evidence_missing"),
            "sentinel={sentinel}"
        );
    }
}

#[test]
fn required_nullable_contract_fields_cannot_be_omitted() {
    for pointer in [
        "/receipt/immutable_base/backing_parent",
        "/receipt/run/cell_id",
    ] {
        let mut request = valid_request();
        let object_pointer = pointer.rsplit_once('/').unwrap().0;
        let key = pointer.rsplit_once('/').unwrap().1;
        request
            .pointer_mut(object_pointer)
            .unwrap()
            .as_object_mut()
            .unwrap()
            .remove(key);
        let validation = report(request);
        assert!(!validation.document_valid, "field={pointer}");
        assert!(
            validation.has_finding("receipt.invalid_document"),
            "field={pointer}"
        );
    }
}

#[test]
fn a_pass_requires_a_non_nil_real_run_cell_id() {
    let mut request = valid_request();
    request["receipt"]["run"]["cell_id"] =
        Value::String("00000000-0000-0000-0000-000000000000".to_owned());
    let validation = report(request);
    assert!(!validation.document_valid);
    assert_eq!(validation.disposition, AcceptanceReceiptDisposition::Rejected);
    assert!(validation.has_finding("receipt.required_evidence_missing"));
}

#[test]
fn malformed_duplicate_and_disclosing_input_fails_closed_without_echoing_it() {
    let duplicate = r#"{"schema_version":1,"schema_version":1,"contract":"vmcell.acceptance-receipt-validation-request.v1"}"#;
    let duplicate_report = validate_acceptance_receipt_bytes(duplicate.as_bytes());
    assert!(!duplicate_report.document_valid);
    assert!(duplicate_report.has_finding("receipt.duplicate_key"));

    let mut disclosure = valid_request();
    disclosure["receipt"]["run"]["provider_object_evidence_id"] =
        Value::String(r"C:\private\qemu-output.txt".to_owned());
    let disclosure_report = report(disclosure);
    assert!(!disclosure_report.document_valid);
    assert!(disclosure_report.has_finding("receipt.forbidden_disclosure"));
    assert!(
        !serde_json::to_string(&disclosure_report)
            .unwrap()
            .contains("qemu-output.txt")
    );
}

#[test]
fn oversized_and_unknown_field_documents_are_bounded_and_rejected() {
    let oversized = vec![b' '; 256 * 1024 + 1];
    let oversized_report = validate_acceptance_receipt_bytes(&oversized);
    assert!(!oversized_report.document_valid);
    assert!(oversized_report.has_finding("receipt.input_too_large"));

    let mut unknown = valid_request();
    unknown["receipt"]["ci_log"] = Value::String("untrusted prose".to_owned());
    let unknown_report = report(unknown);
    assert!(!unknown_report.document_valid);
    assert!(unknown_report.has_finding("receipt.invalid_document"));
}

#[test]
fn templates_and_support_promotion_claims_are_not_validated_as_acceptance_passes() {
    let template = include_bytes!("../docs/receipts/windows-whpx-acceptance-template.json");
    let template_report = validate_acceptance_receipt_bytes(template);
    assert!(!template_report.document_valid);
    assert_eq!(
        template_report.disposition,
        AcceptanceReceiptDisposition::Rejected
    );

    let mut promotion = valid_request();
    promotion["receipt"]["support_promotion"] = Value::String("supported".to_owned());
    let promotion_report = report(promotion);
    assert!(!promotion_report.document_valid);
    assert!(promotion_report.has_finding("receipt.support_promotion_claim"));
}
