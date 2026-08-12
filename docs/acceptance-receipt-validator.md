# Offline acceptance-receipt validator

`vmcell receipt validate` is a bounded, offline consistency check for a
**sanitized** acceptance-receipt validation request supplied on standard input.
It does not contact a host, GitHub, a provider, or a guest; load configuration,
open a state root, inspect a binary, or change a support row. It is not an
operator authorization, a real-platform acceptance run, or publication
evidence.

```text
vmcell --json receipt validate < sanitized-validation-request.json
```

The command reads at most 256 KiB. It accepts only strict UTF-8 JSON with no
duplicate keys, unknown fields, raw host paths, URLs, credentials, commands,
guest output, control characters, or bidirectional controls. Evidence values
are bounded opaque identifiers or SHA-256 digests; a completed run also needs
a non-nil UUID cell ID. The validator never echoes input values in its report.

## Scope and binding

Version 1 deliberately covers only the two frozen v0.3 QEMU tuples at
`release/v0.3.0@d0af04b2e84cf2226628173d2ed0d295aed01f2b`:

| Tuple | Accepted only as a structural binding |
| --- | --- |
| Windows/x86_64 + QEMU/WHPX + Linux/x86_64 + credentialless QGA | The exact v0.3 candidate and a caller-supplied binary/base/preflight binding. |
| Native Linux/x86_64 + QEMU/KVM + Linux/x86_64 + credentialless QGA | The exact v0.3 candidate and a caller-supplied binary/base/preflight binding. |

It rejects current `dev`, templates, preflight collector output, CI logs,
Markdown packets, v0.1/v0.2 manual packets, the v0.4 overlay, and v0.5 host
planning as unsupported input. Those records have distinct contracts and must
not be inferred from a related candidate or tuple.

The request has an outer
`vmcell.acceptance-receipt-validation-request.v1` contract and exactly two
objects:

- `expected_binding`: the independently supplied frozen release ref/candidate
  SHA/version, binary SHA-256, preflight SHA-256, exact tuple, and immutable
  QCOW2 size/SHA-256;
- `receipt`: one filled
  `vmcell.real-platform-acceptance-receipt.v1` record with the same fields,
  opaque evidence IDs, base/overlay binding, terminal result, cleanup facts,
  `authorizing: false`, `support_status: "untested"`, and
  `support_promotion: "not_evaluated"`.

The expected binding is an input assertion, not a substitute for independently
calculating a binary hash or observing a host. A successful check proves only
that this request is internally consistent with the supplied binding and the
compiled frozen v0.3 tuple registry. It never verifies an actual machine or
changes the separate [release acceptance matrix](release-acceptance-matrix.md).

The immutable base must be one standalone QCOW2 with a nonzero exact expected
size, `backing_parent: null`, and matching before/after SHA-256. The overlay
must bind that exact base SHA-256 and show exact-owned cleanup. The receipt
must show an unchanged foreign poststate, no replay of an unknown guest effect,
and no support-promotion claim.

## Terminal-state rules

| Receipt terminal state | Required state | Validator disposition / exit |
| --- | --- | --- |
| `PASS` | `authorized-real-run`, `real_platform_acceptance: "completed"`, clean checkout, actual opaque run/cleanup evidence, exact-owned cleanup, and no manual-review retention | `pass`, exit 0 |
| `PREFLIGHT_PASS` | `observe-only-preflight`, `real_platform_acceptance: "pending"`, no cell/provider/runtime/lifecycle/guest/cleanup evidence, and no overlay cleanup | `preflight_only`, exit 9 |
| `PARTIAL`, `BLOCKED_EXTERNAL`, `OWNER_DECISION_REQUIRED` | `real_platform_acceptance: "pending"` | `terminal_not_pass`, exit 9 |
| Template sentinel, malformed input, unsupported contract, binding drift, disclosure, authorization claim, or support-promotion claim | none | `rejected`, exit 9 |

The inner receipt remains `authorizing: false` even for a structurally valid
`PASS`: the external owner authorization is represented only by a sanitized
opaque evidence ID. A preflight can never be relabelled as `PASS`; CI-looking
prose and a package/test result are not a receipt contract.

For this JSON contract, the preflight sentinel is exactly lowercase
`not_applicable`. Case, punctuation, or spelling variants—and template or
terminal-status words such as `PENDING_REAL_PLATFORM_GATE`, `NOT_EXECUTED`,
or `BLOCKED_EXTERNAL`—are never opaque evidence and are rejected. The
uppercase sentinel in the separate Markdown owner-packet template is not an
input to this validator.

## Output contract

Successful JSON output uses
`vmcell.acceptance-receipt-validation.v1` with:

- `document_sha256`: the SHA-256 of the exact supplied request bytes;
- `document_valid`: whether syntax, disclosure rules, schema, and supplied
  cross-field bindings passed;
- `disposition`: `pass`, `preflight_only`, `terminal_not_pass`, or `rejected`;
- stable, value-free finding `code`s; and
- `authorizing: false` plus `support_promotion: "not_evaluated"`.

The report intentionally has no raw host information, evidence values, paths,
commands, guest output, credentials, or parser diagnostics. A `pass` result
does not mark a support row supported or experimental, publish anything, or
override the real-host packet and reviewed promotion decision required by the
release acceptance matrix.
