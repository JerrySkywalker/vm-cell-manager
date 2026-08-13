# Release acceptance matrix and owner packets

This is the authoritative cross-release register for real-platform acceptance.
It complements the typed [support matrix](support-matrix.md), which remains the
authoritative product-status catalog. Neither document authorizes a host action,
promotes a support row, or replaces a separately admitted acceptance run.
This register does not create a `supported` or `experimental` support row.

The frozen v0.1-v0.4 source refs below are historical and immutable. A
cross-release audit retired their exact candidates from future promotion; see
[the frozen-candidate correction matrix](frozen-candidate-correction-matrix.md)
for exact fix provenance, minimum corrected trees, strategy options, and the
required renewed package/CI/real-platform receipts. Their rows remain here to
preserve the original tuple contracts, not to imply current promotability.

## Evidence rules

Every real-platform claim binds all of the following at once:

| Binding | Required exact evidence | Never substitute |
| --- | --- | --- |
| Candidate | Frozen release commit, clean checkout, binary version/hash, and receipt contract | A later `dev` build, merge parent, or an unbound package |
| Tuple | Host OS/architecture, provider, accelerator, guest OS/architecture, and transport | A related accelerator, another guest, Intel macOS for Apple Silicon, or explicit TCG |
| Host | Effective identity, private state/runtime root, canonical tools, capability, writer window, and foreign prestate | Core CI, WSL2, a shared workstation, or a host without exclusive control |
| Image and guest | Canonical ordinary base, format, content hash/size, provenance, backing chain, and transport expectation | A same-named image, a mutated base, or a different guest build |
| Run and cleanup | Exact-owned namespace, lifecycle/guest evidence, recovery result, cleanup proof, and foreign poststate | Name/PID/socket-only discovery or manual cleanup of ambiguous state |

Repository CI, mocks, fixture preflights, package contracts, and native Linux
compile/test evidence are reusable repository evidence only. Candidate, host,
image, guest, and real-run evidence are exact-binding evidence. A preflight
observes prerequisites; it does not authorize a lifecycle or replace the later
candidate run.

Live host evidence must stay outside Git. A future sanitized Markdown receipt
under `docs/receipts/` may refer to it by opaque evidence ID and digest; it must
not contain credentials, raw paths, raw provider output, guest commands/output,
or identifying host data. That form is compatible with the support-evidence
path policy while preserving the raw host record privately.

Use the [real-platform owner packet template](receipts/real-platform-owner-packet-template.md)
with the release-specific packet below. The template supplies the receipt shape;
the release-specific packet supplies the exact candidate, tuple, and sequence.

## Frozen-release register

| Release / candidate | Exact target tuple | Reusable repository evidence | Required packet and missing external prerequisites | Status |
| --- | --- | --- | --- | --- |
| v0.1.0 `32f4adad3881c5248c6c8c5d47982368b7b55799` | Windows/x86_64 + Hyper-V + Windows/x86_64 + PowerShell Direct | M1 ownership, provider, image/state, and static PowerShell contracts | Historical [v0.1 packet](#v01--dedicated-hyper-v-and-powershell-direct); a future corrected tree must rebind and repeat its dedicated-host prerequisites | `RETIRED_CORRECTION_REQUIRED` |
| v0.2.0 `ed2ed31ae2f0182fc1626321b81e86d09db378c2` | Windows/x86_64 + Hyper-V + Windows/x86_64 + PowerShell Direct | v0.1 foundation plus repeated-session, image lifecycle, compatibility, package, and recovery contracts | Historical [v0.2 packet](#v02--repeated-session-image-and-state); a future corrected tree must rebind and repeat its full lifecycle/recovery prerequisites | `RETIRED_CORRECTION_REQUIRED` |
| v0.3.0 `d0af04b2e84cf2226628173d2ed0d295aed01f2b` | Windows/x86_64 + QEMU/WHPX + Linux/x86_64 + credentialless QGA | Windows CI, fixture-only WHPX preflight, fake QMP/QGA, portable-package contract | Historical [Windows QEMU/WHPX walkthrough](windows-qemu-whpx.md); a corrected tree also requires atomic Job Object/empty-tree proof before new dedicated-host evidence | `RETIRED_CORRECTION_REQUIRED` |
| v0.3.0 `d0af04b2e84cf2226628173d2ed0d295aed01f2b` | Native Linux/x86_64 + QEMU/KVM + Linux/x86_64 + credentialless QGA | Exact-SHA hosted Linux compile/test/package lane, shell fixture preflight, fake QMP/QGA, Unix safety contracts | Historical [native Linux walkthrough](linux-kvm-qga.md); a future corrected tree must rebind and repeat its dedicated-host KVM/QGA prerequisites | `RETIRED_CORRECTION_REQUIRED` |
| v0.4.0 `c741be99ef4632b436f394f1c53b71ed57d0d2d9` | Overlay on an independently accepted v0.1/v0.3 tuple; TOML JobSpec for prepared workload | v0.4 exact-dev Windows/Linux CI, package contracts, JobSpec/plan/result/correlation/compatibility tests | Historical [v0.4 overlay](#v04--job-result-overlay); a future corrected tree must rebind and repeat it against an independently accepted corrected base | `RETIRED_CORRECTION_REQUIRED` |
| v0.5 planning base `c741be99ef4632b436f394f1c53b71ed57d0d2d9` | macOS/Apple Silicon/aarch64 + QEMU/HVF + Linux/aarch64 + credentialless QGA | Fail-closed repository safety contracts only; no macOS lifecycle evidence | [v0.5 preflight handoff](#v05--apple-silicon-observe-only-preflight); dedicated Apple-Silicon host and fresh observe-only preflight | `BLOCKED_EXTERNAL` |

`Windows QEMU/WHPX` and `native Linux QEMU/KVM` are separate tuple rows. WSL2,
fixture success, package validation, or a capability probe cannot satisfy either
row. v0.4 is an overlay: its JobSpec resolves through existing image/provider/
accelerator/guest/lifecycle authority and never carries image bytes,
provisioning, credentials, or a second lifecycle state machine.

## Tooling and receipt ownership

Share only the receipt core: exact candidate/tuple, non-authorizing state,
preflight binding, immutable-base identity, exact-owned cleanup, and
conservative support status. The repository verifies that shared core in
`tests/acceptance_receipt_templates.rs`.

Do not merge the host collectors. Windows ACL/reparse/process semantics, Linux
UID/mode/device-inode/atomic-publication semantics, Hyper-V VM/switch/VHDX
inventory, and future macOS/HVF admission are distinct safety boundaries.

| Path | Repository-local owner | Boundary |
| --- | --- | --- |
| Hyper-V + PowerShell Direct | [M1 gate](m1-hyperv-acceptance.md) and the v0.1/v0.2 packets below | Manual owner packet only; no script claims to preflight Hyper-V acceptance. |
| Windows QEMU/WHPX + QGA | `tools/windows-whpx-preflight.ps1`, its fixture test, and [`windows-whpx-acceptance-template.json`](receipts/windows-whpx-acceptance-template.json) | Fixture preflight is non-mutating and never starts QEMU or enables WHPX. |
| Native Linux QEMU/KVM + QGA | `tools/linux-kvm-preflight.sh`, its fixture/race test, and [`linux-kvm-acceptance-template.json`](receipts/linux-kvm-acceptance-template.json) | Fixture preflight is non-mutating and never creates a VM, issues KVM ioctls, or repairs KVM. |
| macOS QEMU/HVF + QGA | [issue #43](https://github.com/JerrySkywalker/vm-cell-manager/issues/43) and the v0.5 packet below | No repository collector is admitted before the dedicated Apple-Silicon host preflight. |

Each current JSON acceptance template is a *pending*, `authorizing: false`
template. A future filled acceptance record must still have a separate operator
authorization and a sanitized Markdown receipt; neither a template nor a
preflight grants support-promotion authority.

Issue #61 selected one consolidated v0.4.1 corrective-candidate strategy. The
repository-local [corrective ledger](v041-corrective-acceptance-ledger.md) and
[dedicated-host R5 runbook](receipts/v041-r5-dedicated-host-runbook.md) define
its exact correction floor and renewed packet slots. They remain pending and
non-authorizing until the final candidate ref/SHA and fresh evidence are bound;
no historical row or receipt transfers to v0.4.1.

The offline [`acceptance-receipt validator`](acceptance-receipt-validator.md)
adds a strict, sanitized JSON *validation request* for the two v0.3 QEMU
tuples. It validates supplied binding consistency only: it does not replace the
Markdown receipt, contact a host, authorize a lifecycle, or promote support.

## Owner packets

All packets bind one accountable operator, clean exact candidate, isolated time
window, private state root, and fail-closed stop on mismatch.

### v0.1 — dedicated Hyper-V and PowerShell Direct

Use the M1 gate. Before mutation bind Windows build/architecture, effective
identity, ACL-exclusive state root, ordinary immutable VHDX hash/size/parent,
Hyper-V capability prestate, foreign VM/switch inventory, and one exact-owned
namespace. Prove image registration, one stopped networkless cell, one
differencing VHDX with the exact parent, start/inspect/PowerShell Direct
readiness and bounded command, stop/reconcile, idempotent exact-owned destroy,
and unchanged base/foreign state. Name-only, pre-ID, or expected-state mismatch
must be quarantined for manual review.

### v0.2 — repeated session, image, and state

Repeat the full v0.1 sequence on the v0.2 candidate; a v0.1 host run is not a
replacement. Then prove at least two fresh cells, exact-owned retained-session
behavior, image validate/add/inspect/remove semantics, state preflight over a
recoverable copy, interruption/reconciliation, package install/upgrade/remove,
and exact cleanup. Bind every run to its CellId and provider ID; never adopt a
historical cell.

### v0.3 — Windows QEMU/WHPX and native Linux KVM

Use one packet per tuple: the [Windows WHPX walkthrough](windows-qemu-whpx.md)
and template or the [native Linux walkthrough](linux-kvm-qga.md) and template.
Both bind canonical `qemu-system`/`qemu-img`, immutable QCOW2, one overlay,
process/receipt/QMP identity, credentialless QGA readiness/exec/copy/artifact,
unknown-effect no-replay, crash/reconciliation, and exact-owned cleanup. The
Windows path proves WHPX without enabling a feature. The Linux path proves a
native host and usable unmodified `/dev/kvm`; WSL2 cannot substitute.

### v0.4 — Job/result overlay

Use only inside the same authorized window as an exact accepted provider tuple.
Bind a non-secret `vmcell.job-spec.v1` source SHA-256 and logical image
reference, record its resolved non-authorizing plan, then run it twice to prove
fresh job/cell identities. Capture safe `vmcell.job-result.v1` plus
operation/artifact correlation, bounds, cleanup disposition, and retained
unknown-effect state. A job ID or source digest never authorizes replay, reuse,
image import, or cleanup.

### v0.5 — Apple-Silicon observe-only preflight

Keep issue #43 open and do not start v0.5 slices. On the future dedicated
Apple-Silicon host, observe macOS version/build, native arm64 identity, private
state root, canonical `qemu-system-aarch64`/`qemu-img` identities/hashes, HVF
capability, immutable Ubuntu 24.04 LTS aarch64 QCOW2/QGA provenance, bounded
socket namespace, foreign QEMU/socket/writer prestate, and writer exclusivity.
Report zero VM launches, QMP/QGA connections, guest operations, image writes,
package installs, driver/network/service/ACL changes, and reboots. A passing
preflight is only input to a later admitted implementation/acceptance goal.

## Ordering and correction rule

1. A frozen release branch is immutable evidence. A receipt always applies only
   to its named candidate and tuple; it does not amend the branch or `main`.
2. A repository defect is corrected on `dev` through a fresh short-lived PR,
   then receives a fresh candidate and real-platform receipt. An old receipt is
   historical evidence, never behavior proof for the correction.
3. A host/image/guest prerequisite failure stays attached to its exact packet.
   It may be corrected only in a new owner-authorized host window, never from
   CI or by editing support status.
4. A later release does not retroactively accept an older frozen candidate.
   A backport or publication needs an explicit release decision and its own
   exact candidate/evidence chain.
5. `main`, tags, GitHub Releases, and package publication remain separate
   promotion decisions after the declared real-platform gate.
