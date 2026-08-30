# VMCell one-week audit and release gates

Status: **OWNER APPROVED IMPLEMENTATION BASELINE**
Contract: `vmcell.one-week-audit-release-gates.v1`
Authorizing: **false**

## 1. Purpose

These gates prevent a fast owner-preview effort from becoming an accidental
support claim, unsafe host mutation, or relabeling of frozen `0.4.1` evidence.
Every gate is fail-closed. Absence of evidence is not PASS.

## 2. Evidence levels

| Level | Evidence | May prove real Hyper-V usability? |
| --- | --- | --- |
| E0 | Source inspection or documentation | No |
| E1 | Unit, fixture, static, AST, schema tests | No |
| E2 | Generic Hosted Windows/Linux CI | No |
| E3 | Exact candidate/package/binary identity | No |
| E4 | Non-authorizing Live preflight | No; ceiling is `PREFLIGHT_ELIGIBLE` |
| E5 | Owner-attended authorized real run | Yes, for the exact host/candidate/image tuple only |
| E6 | Independent closeout audit | May validate an E5 claim; cannot create missing evidence |

No lower level may substitute for a higher one.

## 3. Finding severity

| Severity | Definition | Required action |
| --- | --- | --- |
| P0 | Destructive host/provider/image/credential safety violation or uncontrolled foreign mutation risk | Stop immediately; no merge or real run |
| P1 | Authority escalation, false acceptance, secret/private-host leakage, candidate drift, unsafe receipt overwrite, ownership bypass, or unknown-effect replay risk | Stop; correct and independently re-audit |
| P2 | Bounded correctness, coverage, continuity, documentation, or operational-safety defect | Resolve before merge or real run |
| P3 | Nonblocking style or maintainability observation | Record; may defer with rationale |

Merge and real-run admission require P0=0, P1=0, and P2=0.

## 4. Gate sequence

### G0 — plan approval

- Owner approves the six-file planning bundle.
- Target is explicitly `OWNER_LOCAL_PREVIEW_PASS`.
- Apple/macOS/HVF and public support are nonblocking and out of scope.
- This gate grants no repository or real-platform authority by itself.

### G1 — PR #77 closeout

- Live base/head/diff/reviews/mergeability match the audited intent.
- All five historical P2 findings remain corrected.
- Exact-head required Hosted Windows/Linux checks are green.
- No unexpected commit, review request-changes, or incompatible `dev`
  divergence exists.
- Normal merge occurs without bypass.
- Exact merged-`dev` Hosted Windows/Linux checks pass at the merge SHA.

Self-hosted R4 checkout-network failure is nonpass evidence but may remain
nonblocking only when repository validation never executed and repository
policy explicitly assigns disposable Hosted CI as the correctness gate. A
source correctness failure is always blocking.

### G2 — tooling/docs source gate

- One branch and one PR from exact merged `dev`.
- Changes limited to preflight tooling, fixtures/static tests, narrow CI,
  owner-preview documentation, and directly related validation.
- Production Rust execution paths unchanged.
- `Cargo.lock` unchanged; package version remains `0.4.1`.
- Support rows and frozen release records unchanged.
- Path-derived operational storage and explicit runtime root implemented.
- Unconditional fixed-drive observations removed.
- Capacity and post-create/pre-start firmware evidence implemented.
- Evidence output cannot overwrite an existing receipt and uses ordinary
  non-reparse containment.
- Fixture mode performs no live observation.
- Mutation-deny AST/static tests cover Hyper-V, VM, VHD, feature, service,
  process, runner, network, disk, ACL, and host families.

### G3 — tooling/docs validation gate

- formatting passes;
- PowerShell static and AST checks pass;
- all Hyper-V fixtures pass deterministically;
- fixture-mode isolation passes;
- provenance/template tests pass;
- existing relevant WHPX/Linux/workflow fixtures remain green;
- locked Clippy, tests, doc-tests where applicable, and build pass;
- package determinism and layout pass where canonical;
- `git diff --check` passes;
- disclosure and private-host scans have zero high-confidence hits;
- complete diff review yields P0=0, P1=0, P2=0;
- exact-head Hosted Windows/Linux CI passes;
- fresh independent audit passes.

### G4 — tooling/docs integration gate

- PR head and tested SHA still match.
- Required reviews do not request changes.
- `dev` remains compatible.
- Normal merge succeeds without bypass.
- Exact merged-`dev` Hosted Windows/Linux CI passes.
- `main` and `release/v0.4.1` remain unchanged.

### G5 — image and path gate

- Windows Server 2022 x86_64 image is legitimate and owner-controlled.
- VHDX is fixed, parentless, detached, read-only, ordinary, non-reparse, and
  exactly hash-bound.
- Sanitized provenance is complete and contains no fabricated evidence.
- Guest local profile prerequisite is owner-attested.
- State, runtime, image, package, and binary paths independently pass the
  path-derived local NTFS/backing/capacity policy.
- Evidence location is separately classified and does not become operational
  storage authority.

### G6 — Live preflight gate

- Exact tooling, candidate, package, binary, image, and provenance are rebound.
- Owner-attended elevated read context and exclusive window are proven.
- Every required observation is available and PASS.
- Receipt is fresh, non-overwriting, sanitized, and digest-bound.
- Result is exactly `PREFLIGHT_ELIGIBLE`; authority remains none and acceptance
  remains false.

### G7 — real-run authorization gate

- New owner authorization names one exact host/candidate/image/path/window.
- Foreign prestate and rollback capacity are freshly recorded.
- No active competing writer exists.
- Credentials are available through interactive bounded stdin.
- Mutation list and stop conditions match
  `owner-attended-acceptance-plan.md` exactly.
- No authority is inferred from G0–G6.

### G8 — owner-preview completion gate

- Stopped cell proves Generation 2, Secure Boot, networkless configuration,
  exact disk parent, exact ownership, and expected resources.
- PowerShell Direct readiness and bounded guest action pass.
- stdout, stderr, zero/nonzero exit, copy, and artifacts pass.
- three fresh-identity sequential runs pass.
- one bounded recovery/reconciliation case passes without blind replay.
- exact-owned cleanup is idempotent and leaves no residue.
- immutable base hash and foreign poststate are unchanged.
- independent closeout review yields P0=0, P1=0, P2=0.

G8 permits only `OWNER_LOCAL_PREVIEW_PASS` for the exact tuple.

## 5. Candidate policy

The frozen `0.4.1` candidate may be used only while the production binary needs
no source correction and all identity hashes remain exact.

If a production defect is proven:

1. stop and preserve evidence;
2. do not amend or relabel the frozen `0.4.1` binary or receipts;
3. create one bounded correction PR;
4. produce a new candidate version, SHA, archive, binary hash, and receipt;
5. run the complete applicable local, Hosted, independent-audit, and real-run
   qualification for that new candidate;
6. stop the one-week effort on a second production defect or timebox expiry.

## 6. Support and release policy

An owner-local preview does not automatically authorize:

- changing `untested` to `experimental` or `supported`;
- moving changes to `main`;
- tagging or publishing a version;
- creating or uploading release assets;
- changing historical v0.4.1 acceptance rows;
- claiming Windows 11, Linux, QEMU, WHPX, KVM, macOS, or HVF coverage.

Any support promotion is a later decision that cites the exact E5/E6 packet and
its limitations. Public release qualification is separately planned.

## 7. Merge decision template

```text
GATE=
EXACT_BASE_SHA=
EXACT_HEAD_SHA=
EXACT_TESTED_SHA=
WINDOWS_CI=
LINUX_CI=
INDEPENDENT_AUDIT=
P0_FINDINGS=
P1_FINDINGS=
P2_FINDINGS=
P3_FINDINGS=
CARGO_LOCK_CHANGED=
PACKAGE_VERSION=
SUPPORT_STATUS_CHANGED=
MAIN_CHANGED=
RELEASE_V041_CHANGED=
MERGE_ADMISSIBLE=
BLOCKER=
```

## 8. Real-run decision template

```text
CANDIDATE_SHA=
BINARY_SHA256=
IMAGE_SHA256=
PROVENANCE_SHA256=
PREFLIGHT_RECEIPT_SHA256=
HOST_ID_OPAQUE=
WINDOW_ID=
FOREIGN_PRESTATE_DIGEST=
EXCLUSIVE_WINDOW=
OWNER_AUTHORIZATION=
REAL_RUN_ADMISSIBLE=
BLOCKER=
```

Every unexecuted field remains `NOT_RUN` or `NOT_PROVEN`, never PASS.
