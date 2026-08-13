# v0.4.1 consolidated corrective acceptance ledger

## Authority and candidate state

Contract: `vmcell.v0.4.1-corrective-acceptance-ledger.v1`.

Issue #61 records `OWNER_DECISION=SELECTED_B`: prepare one consolidated
corrective candidate with repository candidate version `0.4.1`. The frozen
candidate is
`release/v0.4.1@0e7fcf37f4310562d318f9d5c709ddf8e8ca1637`, tree
`18c2e81acc4db57e2275175b138d31049df000da`. Candidate-specific qualification
is recorded in [the frozen qualification report](v041-frozen-qualification.md)
and [release rehearsal](v041-release-rehearsal.json). This ledger remains
non-authorizing; R5 is `NOT_EXECUTED` and support remains `untested`.
The candidate disposition is `PROMOTION_ELIGIBLE_PENDING_R5`.

The authorization is repository-local candidate preparation and freeze only.
It does not authorize a tag, GitHub Release, package publication, `main` merge,
support promotion, provider or guest execution, or host/service/runner change.
Every real-platform result remains `NOT_EXECUTED`; the support status remains
`untested`.

## Consolidated correction floor

The candidate is the union below, not a cherry-pick of one row. Every named SHA
is provenance that must be an ancestor of the final candidate; it is not an
independent acceptance receipt.

| Floor | Exact provenance | Required behavior in the final tree |
| --- | --- | --- |
| Q — Windows QEMU descendant containment | safety clamp `f10ea52a9dddf62adf115225ae0f9d83b5f298da`; QEMU receipt ancestry through `2dd20814efa9cd5d43693f9cc774c8c7475508d8`; atomic Job Object PR #63 head `0217fa3d42addb95008e0190124d50ed4383d0ba`, merged as `f539cbc8aa0d4438df21256ebed3590c187824b1` | schema-2 Job receipt; pinned executable; exact argument digest; `PROC_THREAD_ATTRIBUTE_JOB_LIST` suspended launch; receipt persisted before resume; leader-absent descendant recovery; exact Job termination; zero-active/empty-PID-list terminal proof |
| S — state-root mutation binding | `06934a5b8ce93b80f5fb2b1fc7353a070751e784`, Windows follow-up `97e5054f4c7a7d231c2c97c9c6615574f7b83299` | exact state-root guard binding and pre/post revalidation across durable and destructive paths |
| A — artifact phase integrity | `e50e1759cb3a1c003230d1de45a5a64c6f6283ce` | only copy-out/artifact-collection operations may commit artifact state |
| Reliability A-G | Packet A `090fe32f7e8df1291e076efa8567c7f6b643d4ee` through Packet G merge `4a97888b2a964cabd2ba5d32a967674421c0ae2d` | fixed campaign, isolation, no replay, protocol/path hardening, receipt validation, correctness/performance separation, evidence topology, and residual-limit closeout |
| Frozen correction matrix | PR #60 merge `27dcc1c56db91f8c8ce34bcb8d7e3ed667962158` | frozen v0.1-v0.4 candidates stay `RETIRED_CORRECTION_REQUIRED`; no old evidence transfer |
| Compatibility floor | PR #62 merge `51a8fdfb2410c3ac00bc56ac30b3ff4bc61a77e1`; PR #64 head `390880e4d24c57da62d23ed958cd91e3ee77b1bb`, merged as `9da83b767659557f4a2438a5c63d31dacec34c95` | frozen v1/v2 read/reject/no-rewrite fixtures, manifest reference closure, package layout metadata, CLI/config/error taxonomy, and exact JobSpec SHA to non-authorizing plan/result provenance |

The final accepted candidate must also contain this ledger, its contract test,
the separately reviewed version/package alignment, and every later corrective
commit admitted before freeze. The final release ref—not this table—establishes
the complete immutable candidate tree.

## Historical candidate mapping

| Retired candidate | Historical tuple or overlay | Corrective floor | Disposition |
| --- | --- | --- | --- |
| `release/v0.1.0@32f4adad3881c5248c6c8c5d47982368b7b55799` | Windows Hyper-V / Windows guest / PowerShell Direct | S + A | exact historical candidate remains retired; renew package, CI, audit, and Hyper-V R5 on v0.4.1 |
| `release/v0.2.0@ed2ed31ae2f0182fc1626321b81e86d09db378c2` | repeated Hyper-V session/image/state behavior | S + A | same; no v0.1/v0.2 receipt or package hash transfers |
| `release/v0.3.0@d0af04b2e84cf2226628173d2ed0d295aed01f2b` | Windows QEMU/WHPX / Linux guest / QGA | Q + S + A | repeat on a dedicated Windows host with atomic Job and exact empty-tree evidence |
| same v0.3 source | native Linux QEMU/KVM / Linux guest / QGA | S + A | repeat on a dedicated native Linux host; WSL2/container/preflight cannot substitute |
| `release/v0.4.0@c741be99ef4632b436f394f1c53b71ed57d0d2d9` | JobSpec/result overlay on an accepted base tuple | S + A, and Q when the base is WHPX | repeat twice in the same authorized window as the exact accepted corrected base |

No old receipt, archive checksum, hosted run, branch name, tag name, or support
row proves the corrected source. Historical material may describe a test or
layout, but terminal evidence must be renewed against the exact v0.4.1 tree.

## Repository and package acceptance slots

The freeze audit must fill these slots with terminal, exact-source evidence:

| Slot | Required final binding | Current disposition |
| --- | --- | --- |
| Candidate identity | release ref/SHA/tree, clean export, and Cargo/lock/CLI/docs version binding | `PASS`: exact SHA/tree above; version `0.4.1`; candidate remained immutable |
| Candidate hosted Windows | exact SHA, workflow SHA, Windows image, Rust 1.85, full tests/doc-tests, static/workflow safety, Windows package contract, read-only proof | `PASS`: run `31725027400` |
| Candidate hosted Linux | exact SHA, workflow SHA, Ubuntu image, Rust 1.85, full gate, Linux package contract, read-only proof | `PASS`: runs `31730194963` and retained package `31744783947` |
| Candidate R3 | exact SHA and five-case bounded campaign receipt | `PASS`: run `31725039915`; later counted provider-free confirmation also passed |
| Windows package | fresh `vmcell-v0.4.1-windows-x86_64.zip`, adjacent `SHA256SUMS.txt`, source SHA, archive SHA, binary SHA, deterministic layout, install/remove receipt | `PASS_WITH_LIMITS`: run `31730186128`; assembly fixed-input deterministic, binary non-reproducible and unsigned |
| Linux package | fresh `vmcell-v0.4.1-linux-x86_64.tar.gz`, adjacent `SHA256SUMS.txt`, source SHA, archive SHA, binary SHA, deterministic layout, install/remove receipt | `PASS_WITH_LIMITS`: run `31744783947`; assembly reproducible, binary reproducibility not claimed, GLIBC_2.39 floor |
| Independent audit | final source and release-ref/tree identity, release-critical defect count, immutable historical refs, no support/publication claim | `PASS_WITH_DISCLOSED_LIMITS`: release-critical defects 0; `cargo deny` automation technical failure; no tag, release, main, registry, R5, or support mutation |

R4 remains optional shared-host performance/diagnostic evidence under its
unchanged 30-minute contract. It is not a slot in repository correctness or R5.

## Renewed R5 packet register

Use the implementation-ready runbook in
[`v041-r5-dedicated-host-runbook.md`](receipts/v041-r5-dedicated-host-runbook.md).
Each row is a separate, later owner-authorized execution and sanitized receipt.

| Packet | Exact tuple | Floor | Packet status | Support result |
| --- | --- | --- | --- | --- |
| `V041-R5-HYPERV-PSD-V1` | dedicated Windows x64 / Hyper-V / Windows x64 / PowerShell Direct | S + A | `NOT_EXECUTED` | `untested` |
| `V041-R5-WHPX-QGA-V1` | dedicated Windows x64 / QEMU-WHPX / Linux x64 / QGA | Q + S + A | `NOT_EXECUTED` | `untested` |
| `V041-R5-KVM-QGA-V1` | dedicated native Linux x64 / QEMU-KVM / Linux x64 / QGA | S + A | `NOT_EXECUTED` | `untested` |
| `V041-R5-JOBSPEC-OVERLAY-V1` | v0.4 JobSpec overlay on one exact accepted base packet above | S + A; Q if WHPX base | `NOT_EXECUTED` | `untested`; cannot change base status |

A later filled packet must bind candidate ref/SHA/version, clean checkout,
package archive/manifest/binary hashes, operator authorization ID, isolated host
window, host/image/guest identity, state-root fingerprint, foreign pre/poststate,
exact-owned namespace, lifecycle and no-replay evidence, cleanup, and terminal
result. Only an authorized real run may report `PASS`; a preflight reports at
most `PREFLIGHT_PASS`.

## Freeze rule

`release/v0.4.1` is frozen at the exact SHA/tree above and must not absorb the
later qualification-tooling commits on `dev`. The repository/package gates are
qualified with the limits in the frozen qualification report. Promotion still
requires the declared R5 receipts and separate owner authority. The frozen
branch does not create a tag, publish anything, merge `main`, run R5, or
promote support.
