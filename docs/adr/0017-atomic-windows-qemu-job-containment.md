# ADR 0017: Atomic Windows QEMU Job containment

## Status

Accepted for repository-local corrective-candidate preparation. This decision
does not establish WHPX, guest, provider, package-publication, or support
acceptance.

## Context

`CREATE_NEW_PROCESS_GROUP` identifies a console-control group, not a durable
descendant tree. The frozen Windows QEMU candidates could prove an exact leader
by executable hash, PID, creation token, and launch digest, but could not prove
that no descendant retained QMP/QGA/runtime resources after leader exit.

Post-spawn `AssignProcessToJobObject` is also insufficient: a child could run or
the launcher could fail between process creation and containment. A daemonless
launcher additionally cannot keep a private in-memory handle as its only Job
identity.

## Decision

- Provider config schema 2 persists a random `Local\\vmcell-qemu-<uuid>` Job
  identity before launch. Collision is terminal; names are never reused.
- The resolved QEMU executable is held through creation by an ordinary-file
  handle that denies write and delete sharing. Its pinned bytes supply the
  executable hash checked against the suspended process image.
- Windows creates an anonymous kill-on-close launch guard and the named durable
  Job, then supplies both through `PROC_THREAD_ATTRIBUTE_JOB_LIST` to
  `CreateProcessW` with `CREATE_SUSPENDED`. No QEMU instruction runs outside
  containment.
- Existing parent Job chains use Windows nested-Job semantics. Any unsupported
  or incompatible assignment makes `CreateProcessW` fail; there is no broker,
  environment-triggered internal mode, or post-spawn fallback.
- The exact executable path/hash, argument digest, PID, creation token, and Job
  membership are checked while suspended. The durable argument digest must
  equal the exact argument vector used to render the `CreateProcessW` UTF-16
  command buffer. The complete receipt is atomically persisted before
  `ResumeThread`.
- Kill-on-close remains armed across every pre-persistence failure. After
  resume, it is cleared only after a process reaper exists. A query-only
  inheritable Job handle keeps the named object live across launcher exit.
- Cleanup accepts no PID or name alone. It reopens the persisted Job, proves
  the complete live receipt before graceful QMP shutdown, and may terminate
  only that exact Job if the owned tree remains. It then requires
  `ActiveProcesses == 0` and an empty `JobObjectBasicProcessIdList`; ambiguous
  state is retained.
- After a launcher crash, an absent exact leader plus a queryable nonempty
  receipt Job is surfaced as cleanup-required running state. Stop/destroy may
  terminate that exact Job without reconnecting QMP or replaying `quit`; an
  inaccessible or otherwise unprovable Job remains blocked.
- Schema-1 configurations remain readable. A live or ambiguous legacy Windows
  receipt never receives inferred containment and cannot authorize cleanup.

## Consequences

Windows 10 / Server 2016 or newer is required for
`PROC_THREAD_ATTRIBUTE_JOB_LIST`; unsupported or restricted nesting fails
closed. The random named-object namespace still shares the existing residual
equal-user race boundary: real acceptance requires an exclusive ACL-enforced
writer window. Repository tests use only the current test binary and a bounded
descendant fixture; they do not start QEMU or a VM and cannot substitute for a
fresh dedicated-host R5 packet.
