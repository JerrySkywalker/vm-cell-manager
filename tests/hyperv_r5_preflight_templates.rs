use serde_json::Value;

const HYPERV_R5_PROVENANCE_TEMPLATE: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/docs/receipts/hyperv-r5-image-provenance-template.json"
));

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

fn assert_no_disclosure_fields(value: &Value) {
    match value {
        Value::Array(values) => values.iter().for_each(assert_no_disclosure_fields),
        Value::Object(values) => {
            for (key, value) in values {
                let normalized: String = key
                    .chars()
                    .filter(char::is_ascii_alphanumeric)
                    .flat_map(char::to_lowercase)
                    .collect();
                assert!(
                    !["password", "credential", "secret", "command", "argv"]
                        .iter()
                        .any(|forbidden| normalized.contains(forbidden)),
                    "provenance template models a disclosure field: {key}"
                );
                assert_no_disclosure_fields(value);
            }
        }
        Value::String(value) => assert!(
            !value.starts_with("C:\\") && !value.starts_with("\\\\"),
            "provenance template embeds a raw host path: {value:?}"
        ),
        _ => {}
    }
}

#[test]
fn hyperv_r5_provenance_template_is_complete_and_non_authorizing() {
    let value: Value = serde_json::from_str(HYPERV_R5_PROVENANCE_TEMPLATE)
        .expect("Hyper-V R5 provenance template must be valid JSON");

    assert_eq!(required(&value, "/schema_version"), 1);
    assert_eq!(
        required_string(&value, "/contract"),
        "vmcell.hyperv-r5-image-provenance.v1"
    );
    assert_eq!(required(&value, "/authorizing"), false);
    assert_eq!(
        required_string(&value, "/real_platform_acceptance"),
        "not_started"
    );
    assert_eq!(
        required_string(&value, "/candidate/sha"),
        "REQUIRED_EXACT_40_HEX_SHA"
    );
    assert_eq!(required_string(&value, "/vhdx/vhd_type"), "fixed");
    assert!(required(&value, "/vhdx/parent_path").is_null());
    assert_eq!(required(&value, "/vhdx/attached"), false);
    for field in [
        "/package/archive_sha256",
        "/package/checksum_manifest_sha256",
        "/package/sha256",
        "/candidate_binary/sha256",
        "/windows/edition",
        "/windows/build",
        "/image_source/kind",
        "/image_source/source_sha256",
        "/vhdx/sha256",
        "/vhdx/generation",
        "/vhdx/secure_boot",
        "/vhdx/virtualization_based_security",
        "/creation/created_at_utc",
        "/immutability/declared",
        "/immutability/verification_evidence_sha256",
        "/admission_receipt/receipt_id",
        "/admission_receipt/issued_at_utc",
        "/admission_receipt/sha256",
        "/exclusive_window/eligible",
        "/exclusive_window/starts_at_utc",
        "/exclusive_window/ends_at_utc",
        "/exclusive_window/evidence_sha256",
    ] {
        assert!(
            required_string(&value, field).starts_with("REQUIRED_"),
            "template field {field} must remain a required placeholder"
        );
    }
    assert_no_disclosure_fields(&value);
}
