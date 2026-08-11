# Real-platform owner packet template

This template is a planning/checklist artifact. It is **not** an authorization,
does not create a VM, and must not be filled with credentials, raw host paths,
raw provider output, guest commands/output, or identifying host data.

Create a new sanitized packet for each candidate/tuple/window. Never overwrite
this template or use an earlier packet as proof for a changed candidate, image,
guest, host, or provider tuple.

```text
contract: vmcell.real-platform-owner-packet.v1
schema_version: 1
authorizing: false
real_platform_acceptance: pending
result: NOT_EXECUTED

repository: JerrySkywalker/vm-cell-manager
candidate_sha: REQUIRED_EXACT_40_HEX_SHA
candidate_binary_version: REQUIRED_VERSION
candidate_binary_sha256: REQUIRED_SHA256
candidate_checkout_clean: REQUIRED_BOOLEAN
release_ref: REQUIRED_FROZEN_RELEASE_OR_CANDIDATE_REF

execution_window:
  mode: observe-only-preflight | authorized-real-run
  operator_identity_fingerprint: REQUIRED_SANITIZED_HASH_OR_ID
  authorization_evidence_id: REQUIRED_OPAQUE_ID_OR_NOT_APPLICABLE
  isolation_evidence_id: REQUIRED_OPAQUE_ID
  started_at_utc: REQUIRED_RFC3339
  ended_at_utc: REQUIRED_RFC3339_OR_NOT_EXECUTED

tuple:
  host_os: REQUIRED_VALUE
  host_architecture: REQUIRED_VALUE
  provider: REQUIRED_VALUE
  accelerator: REQUIRED_VALUE_OR_NONE
  guest_os: REQUIRED_VALUE
  guest_architecture: REQUIRED_VALUE
  guest_transport: REQUIRED_VALUE

host_evidence:
  effective_identity_fingerprint: REQUIRED_SANITIZED_HASH_OR_ID
  state_root_identity_fingerprint: REQUIRED_SANITIZED_HASH_OR_ID
  tool_identity_fingerprints: REQUIRED_SANITIZED_HASH_OR_ID
  capability_preflight_receipt_sha256: REQUIRED_SHA256
  foreign_prestate_fingerprint_sha256: REQUIRED_SHA256
  writer_exclusivity_evidence_id: REQUIRED_OPAQUE_ID

image_and_guest:
  immutable_base_sha256_before: REQUIRED_SHA256
  immutable_base_size_before: REQUIRED_BYTES
  immutable_base_provenance_id: REQUIRED_OPAQUE_ID
  guest_transport_expectation: REQUIRED_VALUE

run_and_recovery:
  exact_owned_namespace: REQUIRED_OPAQUE_ID
  cell_id: REQUIRED_CELL_ID_OR_NOT_APPLICABLE
  provider_object_identity: REQUIRED_SANITIZED_PROVIDER_ID_OR_NOT_APPLICABLE
  runtime_receipt_identity: REQUIRED_SANITIZED_HASH_OR_ID_OR_NOT_APPLICABLE
  lifecycle_evidence_id: REQUIRED_OPAQUE_ID
  guest_operation_evidence_id: REQUIRED_OPAQUE_ID_OR_NOT_APPLICABLE
  unknown_guest_effect_replayed: false
  manual_review_retained: REQUIRED_BOOLEAN

cleanup_and_poststate:
  cleanup_policy: exact-owned-only
  cleanup_evidence_id: REQUIRED_OPAQUE_ID
  immutable_base_sha256_after: REQUIRED_SHA256
  foreign_poststate_fingerprint_sha256: REQUIRED_SHA256
  foreign_state_unchanged: REQUIRED_BOOLEAN
```

## Required packet statements

1. State whether this is an observe-only preflight or an authorized real run.
   A preflight must record zero VM launches, QMP/QGA or guest connections,
   guest operations, image writes, package/driver/network/service/ACL changes,
   and reboots.
2. State the exact release-specific overlay used:
   - v0.1 baseline lifecycle and PowerShell Direct;
   - v0.2 repeated session/image/state behavior;
   - v0.3 Windows WHPX or native Linux KVM QGA path;
   - v0.4 JobSpec/result correlation on an already accepted base tuple; or
   - v0.5 Apple-Silicon observe-only preflight.
3. State every missing or mismatched prerequisite as a bounded terminal result.
   Do not repair host configuration, install packages, enable features, change
   permissions/groups, load modules, alter networking, or adopt foreign state.
4. State that a failure leaves ambiguous resources for manual review. Exact-owned
   cleanup is permitted only after the repository's established ownership proof.
5. State whether a sanitized Markdown receipt is eligible to be referenced by a
   future support row. Eligibility is not promotion; a separate reviewed
   decision must still bind the exact candidate and result.
6. A filled receipt replaces `NOT_EXECUTED` with exactly one terminal result:
   `PREFLIGHT_PASS`, `PASS`, `PARTIAL`, `BLOCKED_EXTERNAL`, or
   `OWNER_DECISION_REQUIRED`. It replaces `real_platform_acceptance: pending`
   with `completed` only for a completed authorized run. `PASS` still does not
   promote a support row; the separate reviewed promotion decision remains
   required.
7. Only `authorized-real-run` may report `PASS`, and it must bind one operator,
   authorization evidence, isolated time window, exact CellId, and exact
   provider-object/runtime-receipt identity. A clean
   `observe-only-preflight` reports `PREFLIGHT_PASS`, uses `NOT_APPLICABLE`
   resource values, keeps `real_platform_acceptance: pending`, and can never
   report `PASS`.
