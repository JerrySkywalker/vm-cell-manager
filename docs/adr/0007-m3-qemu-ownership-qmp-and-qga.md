# ADR 0007: QEMU ownership, QMP lifecycle, and QGA guest control

- Status: Accepted
- Date: 2026-08-09

## Context

M3 must prove that the M1/M2 lifecycle and guest-control abstractions are
portable without weakening their ownership boundary. QEMU exposes a process,
QMP and QGA endpoints, and a QCOW2 backing chain rather than a Hyper-V object
with a provider marker.

## Decision

Rust remains the only orchestration state machine. A QEMU cell is authorized by
the current installation, CellId-derived UUID and name, an engine-issued
mutation authority, the exact provider configuration directory, one overlay,
its immutable registered parent, CPU and memory, an explicit accelerator
policy, a canonical executable-content hash, and a hash of the complete launch
argument vector. The versioned QEMU
runtime receipt additionally records QMP/QGA endpoints and, after spawn, the
process id plus platform process-start token. On Unix that process id is also
the owned process-group id: the leader binding is revalidated while live and
the group must be empty before lifecycle completion or runtime removal.

Any live QMP inspection and every `cont` or `quit` mutation first revalidate
that durable PID, start token, launch digest, canonical executable, and
executable-content hash. A matching QMP UUID/name/shape alone cannot substitute
for the process receipt.

No destructive operation is authorized by process id, process name, socket
path, or QEMU name alone. Start and stop negotiate QMP capabilities, correlate
request ids, bound asynchronous events while waiting for their response, and
prove the configured UUID/name, CPU, memory, networkless state, and attached
overlay. QMP/QGA messages, host child output, and host protocol waits are
bounded. Unknown protocol or process identity fails closed.

Registered QEMU images are ordinary immutable QCOW2 files with no backing
image. Unix opens reject symbolic links and bind the opened device/inode to the
current registered pathname before and after provider use. Linux also retains
mutation watches for every canonical pathname component and for the base inode
across `qemu-img` consumption. Exact component changes, inode mutation through
an alternate hard link, watch overflow, or watched-object drift are rejected,
including rename-replace-restore ABA. Creation produces a private uniquely
named QCOW2 overlay, validates its full backing path, and publishes it to the
exact CellId destination with a no-clobber hard link. Rust pins and re-verifies
the base while `qemu-img` or QEMU consumes it. The parent is never converted,
flattened, or mutated.

Acceleration is explicit. The native mapping is KVM on Linux, HVF on macOS,
and WHPX on Windows. TCG is accepted only when both the persisted cell policy
and the request opt in; there is no silent emulation fallback. QEMU launches
with `-nic none` and no interactive monitor, display, serial console, shell, or
daemonization.

QGA is a guest transport, not a lifecycle provider. M3 implements
credentialless Linux QGA exec and CellId-scoped copy using typed protocol
requests; it does not invoke a guest shell. Unknown guest actions retain the M2
nonreplayable recovery classification. QGA exposes no portable process-kill
primitive, so a timed-out `guest-exec` process may remain live inside the
disposable guest and requires operator recovery; vmcell never replays it. QGA
filesystem protection does not
treat an administrator-equivalent process inside the disposable guest as a
security boundary.

## Consequences

- Hyper-V retains its provider-specific proof adapter while the engine and
  opaque authority carry portable lifecycle intent.
- A process that exists without a durable, exact QMP/runtime receipt is
  quarantined; it is never killed or adopted by name or pid.
- A durable pre-spawn intent prevents an unavailable QMP endpoint from being
  misclassified as an off VM during the spawn/receipt crash window.
- Configuration replacements sync both file content and Unix directory
  metadata before the associated provider action begins.
- QEMU option-bearing host paths reject comma and control-character ambiguity
  before launch rather than relying on post-launch drift detection.
- A QMP failure after spawn can require manual recovery, but cannot authorize a
  broad host process kill or base-image deletion. Stale or foreign QMP/QGA
  endpoint paths and surviving owned-process-group members are retained rather
  than inferred safe for deletion.
- QEMU/KVM, WHPX, and HVF real acceptance remain separate host gates. Missing
  QEMU is not a reason to weaken repository-local validation.
- No daemon, host-network mutation, virtualization installation, driver
  change, reboot, or QEMU mutation from the core CI runner is introduced.
