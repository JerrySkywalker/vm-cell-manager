# v0.4.1 dedicated-host R5 runbook

## Scope and stop rule

Contract: `vmcell.v0.4.1-r5-dedicated-host-runbook.v1`.

This is an implementation-ready packet definition, not authority to execute
it. Every packet begins `authorizing: false`, `real_platform_acceptance: pending`,
`result: NOT_EXECUTED`, and `support_status: untested`. A later owner
goal must name exactly one candidate SHA, package/binary hashes, tuple, host,
operator, and bounded window before any provider or guest action.

Stop before mutation on a dirty checkout, source/version/hash mismatch,
unapproved or shared host, concurrent writer, non-private/reparse state root,
unexpected foreign state, noncanonical tool or image, or missing rollback
capacity. Preserve evidence and return `OWNER_DECISION_REQUIRED` or
`BLOCKED_EXTERNAL`; never repair, adopt, replay, or clean ambiguous state.

Raw host evidence stays outside Git. Repository receipts use opaque evidence
IDs and digests and never include credentials, private paths, raw commands,
guest output, or identifying host data.

## Common packet envelope

Before tuple-specific work, record and independently verify:

1. `release/v0.4.1` and clean checkout both resolve to the exact frozen
   candidate SHA; `vmcell --version` is `0.4.1`.
2. The package archive, `SHA256SUMS.txt`, unpacked `vmcell` binary, source SHA,
   Cargo version, target triple, and install/remove layout agree. Build or
   install receipts from v0.1-v0.4 are rejected.
3. Operator authorization, host/isolation identity, execution-window identity,
   private state/runtime root, single writer, and foreign prestate are bound by
   opaque evidence IDs and SHA-256 fingerprints.
4. The ordinary immutable base image has canonical identity, format, size,
   content hash, provenance, and expected parent. Capture the same identity
   after cleanup.
5. Allocate one collision-free exact-owned CellId/provider namespace. Name,
   PID, socket, Job, process group, JobSpec SHA, or prior receipt alone never
   grants ownership.
6. Record each durable transition, interruption/unknown state, no-replay
   decision, reconciliation, exact-owned cleanup, foreign poststate, and any
   retained manual-review object.

Allowed terminal packet results are `PREFLIGHT_PASS`, `PASS`, `PARTIAL`,
`BLOCKED_EXTERNAL`, or `OWNER_DECISION_REQUIRED`. Only a completed
`authorized-real-run` may report `PASS`. Repository CI and fixture preflights
remain R0-R3 evidence and cannot fill an R5 result.

## Packet V041-R5-HYPERV-PSD-V1

Tuple: dedicated Windows x64, Hyper-V, Windows x64 guest, PowerShell Direct.
Correction floor: S + A. Use the general
[`real-platform-owner-packet-template.md`](real-platform-owner-packet-template.md)
and [`m1-hyperv-acceptance.md`](../m1-hyperv-acceptance.md).

Admission additionally binds the Windows build/architecture, effective
identity, Hyper-V capability, ordinary immutable VHDX, ACL-exclusive state and
runtime roots, canonical package/binary, foreign VM/switch inventory, and
exclusive provider writer window.

Authorized sequence:

1. register and validate the exact immutable image;
2. create one stopped networkless cell and prove one differencing VHDX has the
   exact parent;
3. start and inspect the exact provider object, prove PowerShell Direct
   readiness, and execute only the bounded admitted guest action;
4. stop, reconcile, destroy, then repeat exact-owned destroy to prove
   idempotency;
5. prove provider ID/runtime absence and unchanged base hash, host features,
   switches, and foreign inventory.

An expected-state, ownership, disk, networking, identity, or cleanup mismatch
is terminal fail-closed evidence. Name-only or pre-ID objects are quarantined,
not adopted or removed.

## Packet V041-R5-WHPX-QGA-V1

Tuple: dedicated Windows x64, QEMU/WHPX, Linux x64 guest, credentialless QGA.
Correction floor: Q + S + A. Use
[`windows-whpx-acceptance-template.json`](windows-whpx-acceptance-template.json)
and [`windows-qemu-whpx.md`](../windows-qemu-whpx.md).

Admission additionally binds canonical `qemu-system-x86_64` and `qemu-img`
versions/hashes, WHPX advertised preflight, parentless immutable QCOW2,
prepared QGA guest, private endpoint namespace, foreign QEMU/runtime/network
prestate, and exclusive provider/state writers. Preflight never enables WHPX or
starts QEMU.

Authorized sequence:

1. register the base and create exactly one overlay;
2. atomically create QEMU suspended with
   `PROC_THREAD_ATTRIBUTE_JOB_LIST`, persist the schema-2 receipt before
   resume, and record executable hash, PID, start token, exact argument digest,
   Job identity, overlay, QMP identity, and WHPX functional evidence;
3. start/inspect, prove fresh QMP and credentialless QGA readiness, then perform
   the bounded exec/copy/artifact sequence;
4. exercise the admitted crash/timeout/unknown-state case without replay,
   reconcile from the durable receipt, and retain ambiguity for manual review;
5. stop/destroy exact-owned state, terminate only the receipt Job if recovery
   requires it, and prove `ActiveProcesses == 0` plus an empty process-ID list;
6. prove endpoint/process/overlay removal and unchanged base and foreign state.

Leader exit, process-group membership, PID/path match, fixture success, or a
zero child snapshot outside the exact Job does not prove empty descendants.

## Packet V041-R5-KVM-QGA-V1

Tuple: dedicated native Linux x64, QEMU/KVM, Linux x64 guest, credentialless
QGA. Correction floor: S + A. Use
[`linux-kvm-acceptance-template.json`](linux-kvm-acceptance-template.json) and
[`linux-kvm-qga.md`](../linux-kvm-qga.md).

Admission additionally proves a native non-WSL2, non-container host, effective
UID, private mode-0700 state root, canonical QEMU tool hashes, parentless
immutable QCOW2, prepared QGA guest, foreign process/socket/network prestate,
and the exact `/dev/kvm` character-device/inode/major/minor identity. The
observe-only preflight may open `/dev/kvm` read/write but performs no ioctl, VM
creation, QEMU launch, module load, package install, or host repair.

Authorized sequence:

1. register the base, create one overlay, and bind process-group, launch digest,
   QMP, overlay, and KVM functional identities;
2. start/inspect, prove fresh QMP and credentialless QGA readiness, then perform
   bounded exec/copy/artifact operations;
3. exercise the admitted crash/timeout/unknown-state case without replay and
   reconcile using exact process/QMP/QGA state;
4. stop/destroy exact-owned resources and prove process group, sockets, overlay,
   and runtime are absent;
5. prove unchanged base hash, `/dev/kvm` identity, network state, and foreign
   process/runtime inventory.

WSL2, a container, TCG, preflight success, or hosted Linux CI cannot substitute
for this tuple.

## Packet V041-R5-JOBSPEC-OVERLAY-V1

This overlay is not a standalone provider packet. It runs in the same approved
window as one exact `PASS` candidate/tuple above and cites that base packet's
opaque receipt ID and digest. Floor: S + A, plus Q when the base is WHPX.

1. Bind non-secret `vmcell.job-spec.v1` source bytes and SHA-256 plus one
   logical registered image ID. The JobSpec contains no credentials, image
   bytes, provisioning, or host/provider authority.
2. Record the `vmcell.job-plan.v1` projection with `authorizing: false`, exact
   source SHA, selected base tuple, bounded resources/timeouts, copy/artifact
   counts, and no command, path, credential, or raw probe evidence.
3. Execute the identical spec twice through the already accepted base tuple.
   Require the same source SHA but fresh job, cell, operation, and artifact IDs.
4. Record bounded `vmcell.job-result.v1` and `vmcell.job-operations.v1`
   correlation, nonzero/timeout/unknown-effect handling, no replay, retention,
   and cleanup disposition.
5. Prove the base tuple and foreign poststate remain unchanged. The overlay
   cannot promote or alter the base support status.

Job ID and source digest are correlation identities only; neither authorizes
reuse, replay, provider adoption, image import, or cleanup.

## Closeout

One packet cannot satisfy another tuple. A Windows WHPX receipt does not prove
Hyper-V or KVM; a provider packet does not prove the JobSpec overlay; and an
overlay does not upgrade its base. Each filled receipt must be audited against
the exact frozen candidate and kept `untested` in the support matrix until a
separate support-promotion decision. R5 completion still does not authorize a
tag, publication, `main` merge, or GitHub Release.
