# M3 QEMU Acceptance

Repository-local tests and fake QMP/QGA peers prove protocol and ownership
logic; they do not constitute real QEMU acceptance.

The Windows-specific operator sequence and non-mutating receipt preflight are
documented in [Windows QEMU/WHPX Human MVP](windows-qemu-whpx.md). The generated
preflight is evidence input only; it does not authorize the real transaction.

## Admission

Real-provider mutation is allowed only in an explicitly isolated context with:

- an existing QEMU system binary and `qemu-img`; no installation or driver
  changes by vmcell;
- a dedicated ordinary, non-reparse state/runtime root controlled exclusively
  by the acceptance identity;
- one disposable ordinary QCOW2 base with no backing image, recorded SHA-256,
  size, format, and physical identity;
- no concurrent external QEMU, QMP, QGA, or state-root writer;
- read-only pre-state of foreign QEMU processes, control endpoints, host
  acceleration capability, and relevant host networking;
- an available native accelerator, or an explicit owner-approved TCG test;
- no reuse of the core/trusted GitHub Actions runner as a privileged
  virtualization runner.

Absence of any prerequisite is a soft external gate. It does not authorize a
fallback, an installation, or mutation of a foreign process or image.

## Canonical bounded sequence

1. Register and inspect the disposable QCOW2 base; record its hash.
2. Create a stopped, networkless cell with the admitted accelerator.
3. Prove exactly one writable QCOW2 overlay and exactly one immutable backing
   parent.
4. Prove the runtime receipt, launch digest, QMP UUID/name, CPU, memory,
   accelerator, and absence of networking.
5. Start, inspect, stop, and reconcile through QMP.
6. When an admitted Linux guest image includes QGA, prove readiness, bounded
   exec, copy-in, and copy-out beneath the CellId workspace.
7. Destroy twice and prove QEMU process, QMP/QGA endpoints, configuration, and
   runtime/overlay absence.
8. Re-hash the base and prove foreign process/network/host inventories
   unchanged.

Any mismatch stops the run. Cleanup is allowed only for artifacts whose exact
CellId, QMP UUID, configuration, overlay/backing chain, and process instance
remain proven. An ambiguous process or endpoint is preserved for manual
recovery; it is never killed by pid, name, or socket alone.

The admitted state/configuration and image paths must be ordinary and free of
QEMU option-separator ambiguity. A stop is complete only after both the QMP
endpoint and the exact persisted process instance are proven absent.

The no-external-writer window is also the check-to-connect boundary for QGA:
vmcell takes a fresh complete QMP snapshot before every guest action, but QEMU
does not provide an atomic transaction spanning QMP proof and the separate QGA
connection. A timed-out QGA `guest-exec` is nonreplayable and its guest process
may require operator cleanup inside the disposable cell.

## Repository-local gates

- Rust 1.85 locked metadata, check, clippy, tests, and doc-tests;
- fake QMP greeting/capability/id/event/malformed/oversize/timeout coverage;
- fake QGA sync/exec/file/error/output-bound coverage;
- accelerator discovery and explicit-TCG negative/positive tests;
- QCOW2 no-backing base and exactly-one-overlay chain tests;
- launch digest, `-nic none`, UUID/name, and process-receipt drift tests;
- canonical executable identity and executable-content hash binding on Windows;
- non-mutating Windows WHPX preflight fixture and receipt-contract tests;
- crash/retry tests for intent, overlay, definition, spawn, handshake, start,
  stop, runtime deletion, and tombstone boundaries;
- Windows and Linux process-tree timeout coverage;
- independent exact-head audit with P0=0/P1=0.

Real Linux/KVM acceptance requires a real Linux host. WSL2 may validate builds,
Unix compilation, state permissions, symlink containment, and fake protocol
tests, but is not final KVM acceptance. macOS/HVF acceptance is deferred.
