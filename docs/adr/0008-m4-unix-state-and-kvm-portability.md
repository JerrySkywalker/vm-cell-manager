# ADR 0008: Unix state identity and KVM portability

- Status: accepted
- Milestone: M4

## Decision

Linux uses the same provider-neutral engine and QEMU provider as Windows. KVM,
WHPX, and HVF remain accelerator capabilities rather than separate providers.
An accelerator being compiled into QEMU is not enough for selection: Linux
advertises KVM only when `/dev/kvm` is openable for read/write by the current
identity. TCG remains explicit opt-in.

Unix state roots and every vmcell-created state directory are private to the
effective user (`0700`); state, lock, artifact, and QEMU configuration files are
created as `0600`. Existing state with group/other permissions or a different
owner fails closed. Authority-bearing opens use `O_NOFOLLOW` and `O_CLOEXEC`;
directories additionally use `O_DIRECTORY`. Open path identity is revalidated
by device/inode before provider and guest actions.

Unix atomic state and QEMU configuration replacements fsync file content and
parent-directory metadata. Runtime deletion remains scoped to the exact
CellId directory and uses Rust's no-symlink-traversal recursive removal. A
same-user process able to rename entries inside the private state root remains
an excluded external writer; real-host acceptance must prove one exclusive
state root and no concurrent QEMU/QMP/QGA writer.

QEMU executables are resolved to canonical executable paths on Unix when found
on `PATH`. Linux process recovery binds `/proc` start time, executable, launch
digest, QMP UUID/name/runtime definition, and the durable vmcell receipt.
Unknown `/proc`, QMP, state, or socket identity never becomes absence proof.

## Consequences

- No Linux daemon, libvirt layer, or KVM-specific orchestration state machine is
  introduced.
- Windows handle and ACL behavior remains unchanged.
- WSL2 is useful for development but cannot prove real Linux/KVM acceptance.
- macOS/HVF stays fail-closed and deferred until a real macOS host is admitted.
