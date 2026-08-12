//! Offline validation for sanitized real-platform acceptance receipts.
//!
//! This module deliberately validates only a caller-supplied binding and the
//! receipt's internal consistency. It never accesses a host, Git, a binary,
//! state, a provider, or the support matrix; a successful result is not an
//! authorization or a support-status promotion.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize, de};
use serde_json::Value;
use sha2::{Digest, Sha256};
use uuid::Uuid;

pub const ACCEPTANCE_RECEIPT_VALIDATION_CONTRACT: &str = "vmcell.acceptance-receipt-validation.v1";
const REQUEST_CONTRACT: &str = "vmcell.acceptance-receipt-validation-request.v1";
const RECEIPT_CONTRACT: &str = "vmcell.real-platform-acceptance-receipt.v1";
const SCHEMA_VERSION: u32 = 1;
pub const MAX_ACCEPTANCE_RECEIPT_BYTES: usize = 256 * 1024;
const MAX_VALUE_BYTES: usize = 256;
const MAX_JSON_DEPTH: usize = 16;
const MAX_JSON_OBJECT_FIELDS: usize = 32;
const MAX_JSON_ARRAY_ITEMS: usize = 16;
const V03_RELEASE_REF: &str = "release/v0.3.0";
const V03_CANDIDATE_SHA: &str = "d0af04b2e84cf2226628173d2ed0d295aed01f2b";
const V03_CANDIDATE_VERSION: &str = "0.3.0";
const NOT_APPLICABLE: &str = "not_applicable";
const DUPLICATE_KEY_SENTINEL: &str = "vmcell_duplicate_json_key";
/// Normalized template/status words which can never identify evidence.
///
/// Keep these separate from the human-facing template contracts: accepting one
/// as an opaque identifier would let a placeholder or terminal state disguise
/// itself as completed-run evidence.
const RESERVED_NON_EVIDENCE_IDS: &[&str] = &[
    "blockedexternal",
    "developmentonly",
    "experimental",
    "notapplicable",
    "notevaluated",
    "notexecuted",
    "ownerdecisionrequired",
    "partial",
    "pass",
    "pendingrealplatformgate",
    "preflightpass",
    "supported",
    "untested",
    "unsupported",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AcceptanceReceiptDisposition {
    Pass,
    PreflightOnly,
    TerminalNotPass,
    Rejected,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AcceptanceReceiptFinding {
    pub code: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AcceptanceReceiptValidationReport {
    pub schema_version: u32,
    pub contract: &'static str,
    pub authorizing: bool,
    pub support_promotion: &'static str,
    pub document_sha256: String,
    pub document_valid: bool,
    pub disposition: AcceptanceReceiptDisposition,
    pub findings: Vec<AcceptanceReceiptFinding>,
}

impl AcceptanceReceiptValidationReport {
    #[must_use]
    pub fn has_finding(&self, code: &str) -> bool {
        self.findings.iter().any(|finding| finding.code == code)
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ValidationRequest {
    schema_version: u32,
    contract: String,
    expected_binding: ExpectedBinding,
    receipt: FilledReceipt,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ExpectedBinding {
    release_ref: String,
    candidate_sha: String,
    candidate_version: String,
    candidate_binary_sha256: String,
    preflight_receipt_sha256: String,
    tuple: ReceiptTuple,
    immutable_base: ExpectedImmutableBase,
}

#[derive(Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct ExpectedImmutableBase {
    format: String,
    size_bytes: u64,
    sha256: String,
}

#[derive(Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct ReceiptTuple {
    host_os: String,
    host_architecture: String,
    provider: String,
    accelerator: String,
    guest_os: String,
    guest_architecture: String,
    guest_transport: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct FilledReceipt {
    schema_version: u32,
    contract: String,
    authorizing: bool,
    real_platform_acceptance: RealPlatformAcceptance,
    result: TerminalResult,
    repository: ReceiptRepository,
    candidate_binary: CandidateBinary,
    tuple: ReceiptTuple,
    preflight: PreflightBinding,
    immutable_base: ReceiptImmutableBase,
    overlay: OverlayBinding,
    run: RunEvidence,
    cleanup: CleanupEvidence,
    support_status: String,
    support_promotion: String,
}

#[derive(Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
enum RealPlatformAcceptance {
    Pending,
    Completed,
}

#[derive(Debug, Deserialize, PartialEq, Eq)]
enum TerminalResult {
    #[serde(rename = "PREFLIGHT_PASS")]
    PreflightPass,
    #[serde(rename = "PASS")]
    Pass,
    #[serde(rename = "PARTIAL")]
    Partial,
    #[serde(rename = "BLOCKED_EXTERNAL")]
    BlockedExternal,
    #[serde(rename = "OWNER_DECISION_REQUIRED")]
    OwnerDecisionRequired,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ReceiptRepository {
    slug: String,
    release_ref: String,
    candidate_sha: String,
    checkout_clean: bool,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CandidateBinary {
    version: String,
    sha256: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct PreflightBinding {
    receipt_sha256: String,
    evidence_id: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ReceiptImmutableBase {
    format: String,
    size_bytes: u64,
    sha256_before: String,
    sha256_after: String,
    backing_parent: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct OverlayBinding {
    base_sha256: String,
    identity_evidence_id: String,
    cleanup_completed: bool,
}

#[derive(Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
enum ReceiptRunMode {
    ObserveOnlyPreflight,
    AuthorizedRealRun,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RunEvidence {
    mode: ReceiptRunMode,
    authorization_evidence_id: String,
    isolation_evidence_id: String,
    exact_owned_namespace: String,
    cell_id: Option<String>,
    provider_object_evidence_id: String,
    runtime_receipt_evidence_id: String,
    lifecycle_evidence_id: String,
    guest_evidence_id: String,
    unknown_guest_effect_replayed: bool,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CleanupEvidence {
    policy: String,
    evidence_id: String,
    immutable_base_sha256_after: String,
    foreign_state_unchanged: bool,
    manual_review_retained: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InputScanError {
    Disclosure,
    Limit,
}

/// Validate one bounded, sanitized receipt-validation request from already-read bytes.
///
/// The caller provides the expected frozen-candidate/binary/base binding in the
/// same request. This is intentionally a structural offline check: it cannot
/// authenticate an operator, inspect a host, calculate a binary hash, or
/// promote support.
#[must_use]
pub fn validate_acceptance_receipt_bytes(bytes: &[u8]) -> AcceptanceReceiptValidationReport {
    let document_sha256 = hex_sha256(bytes);
    if bytes.len() > MAX_ACCEPTANCE_RECEIPT_BYTES {
        return rejected(document_sha256, "receipt.input_too_large");
    }
    if std::str::from_utf8(bytes).is_err() {
        return rejected(document_sha256, "receipt.invalid_utf8");
    }
    if let Err(error) = reject_duplicate_json_keys(bytes) {
        return rejected(
            document_sha256,
            if error {
                "receipt.duplicate_key"
            } else {
                "receipt.invalid_json"
            },
        );
    }
    let value: Value = match serde_json::from_slice(bytes) {
        Ok(value) => value,
        Err(_) => return rejected(document_sha256, "receipt.invalid_json"),
    };
    if let Err(error) = scan_sanitized_json(&value, 0) {
        return rejected(
            document_sha256,
            match error {
                InputScanError::Disclosure => "receipt.forbidden_disclosure",
                InputScanError::Limit => "receipt.input_limit_exceeded",
            },
        );
    }
    if !has_required_nullable_fields(&value) {
        return rejected(document_sha256, "receipt.invalid_document");
    }
    let request: ValidationRequest = match serde_json::from_value(value) {
        Ok(request) => request,
        Err(_) => return rejected(document_sha256, "receipt.invalid_document"),
    };

    let mut findings = Vec::new();
    let mut valid = true;
    let mut reject = |code: &'static str| {
        valid = false;
        push_finding(&mut findings, code);
    };

    if request.schema_version != SCHEMA_VERSION {
        reject("receipt.unsupported_schema");
    }
    if request.contract != REQUEST_CONTRACT
        || request.receipt.schema_version != SCHEMA_VERSION
        || request.receipt.contract != RECEIPT_CONTRACT
    {
        reject("receipt.unsupported_contract");
    }
    if request.receipt.authorizing {
        reject("receipt.authorizing_claim");
    }
    if request.receipt.support_promotion != "not_evaluated" {
        reject("receipt.support_promotion_claim");
    }
    if request.receipt.support_status != "untested" {
        reject("receipt.support_status_mismatch");
    }

    validate_binding(&request.expected_binding, &mut reject);
    validate_receipt_ids(&request.receipt, &mut reject);
    validate_cross_fields(&request, &mut reject);

    if !valid {
        return AcceptanceReceiptValidationReport {
            schema_version: SCHEMA_VERSION,
            contract: ACCEPTANCE_RECEIPT_VALIDATION_CONTRACT,
            authorizing: false,
            support_promotion: "not_evaluated",
            document_sha256,
            document_valid: false,
            disposition: AcceptanceReceiptDisposition::Rejected,
            findings,
        };
    }

    let disposition = match request.receipt.result {
        TerminalResult::Pass => {
            if validate_pass_terminal_state(&request.receipt, &mut findings) {
                AcceptanceReceiptDisposition::Pass
            } else {
                return AcceptanceReceiptValidationReport {
                    schema_version: SCHEMA_VERSION,
                    contract: ACCEPTANCE_RECEIPT_VALIDATION_CONTRACT,
                    authorizing: false,
                    support_promotion: "not_evaluated",
                    document_sha256,
                    document_valid: false,
                    disposition: AcceptanceReceiptDisposition::Rejected,
                    findings,
                };
            }
        }
        TerminalResult::PreflightPass => {
            if validate_preflight_terminal_state(&request.receipt, &mut findings) {
                push_finding(&mut findings, "receipt.preflight_only");
                AcceptanceReceiptDisposition::PreflightOnly
            } else {
                return AcceptanceReceiptValidationReport {
                    schema_version: SCHEMA_VERSION,
                    contract: ACCEPTANCE_RECEIPT_VALIDATION_CONTRACT,
                    authorizing: false,
                    support_promotion: "not_evaluated",
                    document_sha256,
                    document_valid: false,
                    disposition: AcceptanceReceiptDisposition::Rejected,
                    findings,
                };
            }
        }
        TerminalResult::Partial
        | TerminalResult::BlockedExternal
        | TerminalResult::OwnerDecisionRequired => {
            if request.receipt.real_platform_acceptance != RealPlatformAcceptance::Pending {
                push_finding(&mut findings, "receipt.state_contradiction");
                return AcceptanceReceiptValidationReport {
                    schema_version: SCHEMA_VERSION,
                    contract: ACCEPTANCE_RECEIPT_VALIDATION_CONTRACT,
                    authorizing: false,
                    support_promotion: "not_evaluated",
                    document_sha256,
                    document_valid: false,
                    disposition: AcceptanceReceiptDisposition::Rejected,
                    findings,
                };
            }
            push_finding(&mut findings, "receipt.terminal_not_pass");
            AcceptanceReceiptDisposition::TerminalNotPass
        }
    };

    AcceptanceReceiptValidationReport {
        schema_version: SCHEMA_VERSION,
        contract: ACCEPTANCE_RECEIPT_VALIDATION_CONTRACT,
        authorizing: false,
        support_promotion: "not_evaluated",
        document_sha256,
        document_valid: true,
        disposition,
        findings,
    }
}

/// Serde treats omitted `Option<T>` fields like explicit JSON `null`. The
/// receipt contract needs both nullable fields to be spelled out, so check
/// their object-key presence before typed deserialization.
fn has_required_nullable_fields(value: &Value) -> bool {
    let Some(receipt) = value.get("receipt").and_then(Value::as_object) else {
        return false;
    };
    let Some(immutable_base) = receipt.get("immutable_base").and_then(Value::as_object) else {
        return false;
    };
    let Some(run) = receipt.get("run").and_then(Value::as_object) else {
        return false;
    };
    immutable_base.contains_key("backing_parent") && run.contains_key("cell_id")
}

fn validate_binding(binding: &ExpectedBinding, reject: &mut impl FnMut(&'static str)) {
    if binding.release_ref != V03_RELEASE_REF
        || binding.candidate_sha != V03_CANDIDATE_SHA
        || binding.candidate_version != V03_CANDIDATE_VERSION
        || !is_registered_v03_qemu_tuple(&binding.tuple)
    {
        reject("receipt.candidate_binding_mismatch");
    }
    if !is_lower_hex(&binding.candidate_binary_sha256, 64)
        || !is_lower_hex(&binding.preflight_receipt_sha256, 64)
        || binding.immutable_base.format != "qcow2"
        || binding.immutable_base.size_bytes == 0
        || !is_lower_hex(&binding.immutable_base.sha256, 64)
    {
        reject("receipt.invalid_binding");
    }
}

fn validate_receipt_ids(receipt: &FilledReceipt, reject: &mut impl FnMut(&'static str)) {
    let required = [
        &receipt.preflight.evidence_id,
        &receipt.run.isolation_evidence_id,
        &receipt.run.exact_owned_namespace,
    ];
    if required.into_iter().any(|value| !is_opaque_id(value))
        || receipt
            .run
            .cell_id
            .as_deref()
            .is_some_and(|value| !is_non_nil_uuid(value))
    {
        reject("receipt.required_evidence_missing");
    }
    let possibly_not_applicable = [
        &receipt.overlay.identity_evidence_id,
        &receipt.run.authorization_evidence_id,
        &receipt.run.provider_object_evidence_id,
        &receipt.run.runtime_receipt_evidence_id,
        &receipt.run.lifecycle_evidence_id,
        &receipt.run.guest_evidence_id,
        &receipt.cleanup.evidence_id,
    ];
    if possibly_not_applicable
        .into_iter()
        .any(|value| !is_opaque_or_not_applicable(value))
    {
        reject("receipt.required_evidence_missing");
    }
}

fn is_non_nil_uuid(value: &str) -> bool {
    Uuid::parse_str(value).is_ok_and(|id| !id.is_nil())
}

fn validate_cross_fields(request: &ValidationRequest, reject: &mut impl FnMut(&'static str)) {
    let expected = &request.expected_binding;
    let receipt = &request.receipt;
    if receipt.repository.slug != "JerrySkywalker/vm-cell-manager"
        || receipt.repository.release_ref != expected.release_ref
        || receipt.repository.candidate_sha != expected.candidate_sha
    {
        reject("receipt.candidate_binding_mismatch");
    }
    if !receipt.repository.checkout_clean {
        reject("receipt.checkout_not_clean");
    }
    if receipt.candidate_binary.version != expected.candidate_version
        || receipt.candidate_binary.sha256 != expected.candidate_binary_sha256
        || !is_lower_hex(&receipt.candidate_binary.sha256, 64)
    {
        reject("receipt.binary_binding_mismatch");
    }
    if receipt.tuple != expected.tuple {
        reject("receipt.tuple_binding_mismatch");
    }
    if receipt.preflight.receipt_sha256 != expected.preflight_receipt_sha256
        || !is_lower_hex(&receipt.preflight.receipt_sha256, 64)
    {
        reject("receipt.preflight_binding_mismatch");
    }
    let base = &receipt.immutable_base;
    if base.format != "qcow2"
        || base.size_bytes != expected.immutable_base.size_bytes
        || base.sha256_before != expected.immutable_base.sha256
        || base.sha256_after != expected.immutable_base.sha256
        || base.backing_parent.is_some()
        || receipt.overlay.base_sha256 != expected.immutable_base.sha256
        || receipt.cleanup.immutable_base_sha256_after != expected.immutable_base.sha256
        || !is_lower_hex(&base.sha256_before, 64)
        || !is_lower_hex(&base.sha256_after, 64)
        || !is_lower_hex(&receipt.overlay.base_sha256, 64)
        || !is_lower_hex(&receipt.cleanup.immutable_base_sha256_after, 64)
    {
        reject("receipt.base_binding_mismatch");
    }
    if receipt.cleanup.policy != "exact-owned-only"
        || !receipt.cleanup.foreign_state_unchanged
        || receipt.run.unknown_guest_effect_replayed
    {
        reject("receipt.state_contradiction");
    }
}

fn validate_pass_terminal_state(
    receipt: &FilledReceipt,
    findings: &mut Vec<AcceptanceReceiptFinding>,
) -> bool {
    let valid = receipt.real_platform_acceptance == RealPlatformAcceptance::Completed
        && receipt.run.mode == ReceiptRunMode::AuthorizedRealRun
        && is_opaque_id(&receipt.run.authorization_evidence_id)
        && receipt.run.cell_id.is_some()
        && [
            &receipt.overlay.identity_evidence_id,
            &receipt.run.provider_object_evidence_id,
            &receipt.run.runtime_receipt_evidence_id,
            &receipt.run.lifecycle_evidence_id,
            &receipt.run.guest_evidence_id,
            &receipt.cleanup.evidence_id,
        ]
        .into_iter()
        .all(|value| is_opaque_id(value))
        && receipt.overlay.cleanup_completed
        && !receipt.cleanup.manual_review_retained;
    if !valid {
        push_finding(findings, "receipt.state_contradiction");
    }
    valid
}

fn validate_preflight_terminal_state(
    receipt: &FilledReceipt,
    findings: &mut Vec<AcceptanceReceiptFinding>,
) -> bool {
    let valid = receipt.real_platform_acceptance == RealPlatformAcceptance::Pending
        && receipt.run.mode == ReceiptRunMode::ObserveOnlyPreflight
        && receipt.run.authorization_evidence_id == NOT_APPLICABLE
        && receipt.run.cell_id.is_none()
        && [
            &receipt.overlay.identity_evidence_id,
            &receipt.run.provider_object_evidence_id,
            &receipt.run.runtime_receipt_evidence_id,
            &receipt.run.lifecycle_evidence_id,
            &receipt.run.guest_evidence_id,
            &receipt.cleanup.evidence_id,
        ]
        .into_iter()
        .all(|value| *value == NOT_APPLICABLE)
        && !receipt.overlay.cleanup_completed
        && !receipt.cleanup.manual_review_retained;
    if !valid {
        push_finding(findings, "receipt.state_contradiction");
    }
    valid
}

fn is_registered_v03_qemu_tuple(tuple: &ReceiptTuple) -> bool {
    tuple.host_architecture == "x86_64"
        && tuple.provider == "qemu"
        && tuple.guest_os == "linux"
        && tuple.guest_architecture == "x86_64"
        && tuple.guest_transport == "qga"
        && matches!(
            (tuple.host_os.as_str(), tuple.accelerator.as_str()),
            ("windows", "whpx") | ("linux", "kvm")
        )
}

fn is_lower_hex(value: &str, length: usize) -> bool {
    value.len() == length
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn is_opaque_id(value: &str) -> bool {
    if value.len() < 12 || value.len() > 128 {
        return false;
    }
    let lower = value.to_ascii_lowercase();
    let collapsed: String = lower
        .bytes()
        .filter(u8::is_ascii_alphanumeric)
        .map(char::from)
        .collect();
    if RESERVED_NON_EVIDENCE_IDS.contains(&collapsed.as_str()) || collapsed.starts_with("required")
    {
        return false;
    }
    if [
        "required_",
        "password",
        "credential",
        "secret",
        "command",
        "guestoutput",
    ]
    .iter()
    .any(|forbidden| lower.contains(forbidden))
    {
        return false;
    }
    value
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
}

fn is_opaque_or_not_applicable(value: &str) -> bool {
    value == NOT_APPLICABLE || is_opaque_id(value)
}

fn rejected(document_sha256: String, code: &'static str) -> AcceptanceReceiptValidationReport {
    AcceptanceReceiptValidationReport {
        schema_version: SCHEMA_VERSION,
        contract: ACCEPTANCE_RECEIPT_VALIDATION_CONTRACT,
        authorizing: false,
        support_promotion: "not_evaluated",
        document_sha256,
        document_valid: false,
        disposition: AcceptanceReceiptDisposition::Rejected,
        findings: vec![AcceptanceReceiptFinding { code }],
    }
}

fn push_finding(findings: &mut Vec<AcceptanceReceiptFinding>, code: &'static str) {
    if !findings.iter().any(|finding| finding.code == code) {
        findings.push(AcceptanceReceiptFinding { code });
    }
}

fn hex_sha256(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut rendered = String::with_capacity(64);
    for byte in digest {
        use std::fmt::Write;
        write!(&mut rendered, "{byte:02x}").expect("writing to String must succeed");
    }
    rendered
}

fn reject_duplicate_json_keys(bytes: &[u8]) -> Result<(), bool> {
    let mut deserializer = serde_json::Deserializer::from_slice(bytes);
    if let Err(error) = DuplicateFreeJson::deserialize(&mut deserializer) {
        return Err(error.to_string().contains(DUPLICATE_KEY_SENTINEL));
    }
    deserializer.end().map_err(|_| false)
}

struct DuplicateFreeJson;

impl<'de> Deserialize<'de> for DuplicateFreeJson {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: de::Deserializer<'de>,
    {
        struct DuplicateFreeVisitor;

        impl<'de> de::Visitor<'de> for DuplicateFreeVisitor {
            type Value = DuplicateFreeJson;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("a JSON value without duplicate object keys")
            }

            fn visit_bool<E>(self, _: bool) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                Ok(DuplicateFreeJson)
            }

            fn visit_i64<E>(self, _: i64) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                Ok(DuplicateFreeJson)
            }

            fn visit_u64<E>(self, _: u64) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                Ok(DuplicateFreeJson)
            }

            fn visit_f64<E>(self, _: f64) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                Ok(DuplicateFreeJson)
            }

            fn visit_str<E>(self, _: &str) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                Ok(DuplicateFreeJson)
            }

            fn visit_string<E>(self, _: String) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                Ok(DuplicateFreeJson)
            }

            fn visit_none<E>(self) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                Ok(DuplicateFreeJson)
            }

            fn visit_unit<E>(self) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                Ok(DuplicateFreeJson)
            }

            fn visit_seq<A>(self, mut sequence: A) -> Result<Self::Value, A::Error>
            where
                A: de::SeqAccess<'de>,
            {
                while sequence.next_element::<DuplicateFreeJson>()?.is_some() {}
                Ok(DuplicateFreeJson)
            }

            fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
            where
                A: de::MapAccess<'de>,
            {
                let mut keys = BTreeSet::new();
                while let Some(key) = map.next_key::<String>()? {
                    if !keys.insert(key) {
                        return Err(de::Error::custom(DUPLICATE_KEY_SENTINEL));
                    }
                    map.next_value::<DuplicateFreeJson>()?;
                }
                Ok(DuplicateFreeJson)
            }
        }

        deserializer.deserialize_any(DuplicateFreeVisitor)
    }
}

fn scan_sanitized_json(value: &Value, depth: usize) -> Result<(), InputScanError> {
    if depth > MAX_JSON_DEPTH {
        return Err(InputScanError::Limit);
    }
    match value {
        Value::Null | Value::Bool(_) | Value::Number(_) => Ok(()),
        Value::String(value) => {
            if value.len() > MAX_VALUE_BYTES {
                return Err(InputScanError::Limit);
            }
            if string_discloses_host_data(value) {
                return Err(InputScanError::Disclosure);
            }
            Ok(())
        }
        Value::Array(values) => {
            if values.len() > MAX_JSON_ARRAY_ITEMS {
                return Err(InputScanError::Limit);
            }
            for value in values {
                scan_sanitized_json(value, depth + 1)?;
            }
            Ok(())
        }
        Value::Object(values) => {
            if values.len() > MAX_JSON_OBJECT_FIELDS {
                return Err(InputScanError::Limit);
            }
            for (key, value) in values {
                if key_discloses_forbidden_data(key) {
                    return Err(InputScanError::Disclosure);
                }
                scan_sanitized_json(value, depth + 1)?;
            }
            Ok(())
        }
    }
}

fn key_discloses_forbidden_data(key: &str) -> bool {
    let normalized: String = key
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .flat_map(char::to_lowercase)
        .collect();
    [
        "password",
        "credential",
        "secret",
        "command",
        "argv",
        "guestoutput",
        "hostpath",
        "canonicalpath",
    ]
    .iter()
    .any(|forbidden| normalized.contains(forbidden))
}

fn string_discloses_host_data(value: &str) -> bool {
    value.chars().any(|character| {
        character.is_control()
            || matches!(
                character,
                '\u{061c}'
                    | '\u{200e}'
                    | '\u{200f}'
                    | '\u{202a}'
                    | '\u{202b}'
                    | '\u{202c}'
                    | '\u{202d}'
                    | '\u{202e}'
                    | '\u{2066}'
                    | '\u{2067}'
                    | '\u{2068}'
                    | '\u{2069}'
            )
    }) || value.starts_with('/')
        || value.starts_with("\\\\")
        || value.starts_with("file:")
        || value.contains("://")
        || value.as_bytes().get(1).is_some_and(|byte| *byte == b':')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognized_v03_tuples_are_limited_to_windows_whpx_or_native_linux_kvm() {
        let windows = ReceiptTuple {
            host_os: "windows".to_owned(),
            host_architecture: "x86_64".to_owned(),
            provider: "qemu".to_owned(),
            accelerator: "whpx".to_owned(),
            guest_os: "linux".to_owned(),
            guest_architecture: "x86_64".to_owned(),
            guest_transport: "qga".to_owned(),
        };
        assert!(is_registered_v03_qemu_tuple(&windows));
        let linux = ReceiptTuple {
            host_os: "linux".to_owned(),
            accelerator: "kvm".to_owned(),
            ..windows
        };
        assert!(is_registered_v03_qemu_tuple(&linux));
    }

    #[test]
    fn opaque_evidence_identifiers_are_bounded_and_non_disclosing() {
        assert!(is_opaque_id("evidence-cleanup-123"));
        assert!(!is_opaque_id(NOT_APPLICABLE));
        assert!(!is_opaque_id("NOT-APPLICABLE"));
        assert!(!is_opaque_id("NOT_EXECUTED"));
        assert!(!is_opaque_id("PENDING_REAL_PLATFORM_GATE"));
        assert!(!is_opaque_id("BLOCKED_EXTERNAL"));
        assert!(!is_opaque_id("OWNER_DECISION_REQUIRED"));
        assert!(!is_opaque_id("REQUIRED_EVIDENCE"));
        assert!(!is_opaque_id("secret-evidence-123"));
        assert!(!is_opaque_id("C:\\private\\host"));
        assert!(is_non_nil_uuid("00000000-0000-0000-0000-000000000001"));
        assert!(!is_non_nil_uuid("00000000-0000-0000-0000-000000000000"));
        assert!(string_discloses_host_data("evidence\u{2066}hidden"));
    }
}
