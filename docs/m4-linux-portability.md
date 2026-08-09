# M4 Linux portability foundation

M4 makes the M3 QEMU provider host-portable without creating a separate KVM
provider. This document is a repository-local validation contract, not a real
Linux acceptance receipt.

## Linux host admission

A real acceptance host must prove all of the following before mutation:

- native Linux, not WSL2 or a container presented as the host;
- an explicitly dedicated acceptance identity and ordinary private state root;
- no external state, QEMU, QMP, QGA, or image writer during the run;
- an ordinary immutable QCOW2 base with bound size and SHA-256;
- a usable `/dev/kvm` opened by the acceptance identity, unless the test is an
  explicitly bounded TCG-only experiment;
- read-only pre-state for foreign QEMU processes, sockets, network devices, and
  the base image.

Never change KVM modules, permissions, groups, packages, host networking, or
system virtualization settings automatically.

## Repository-local checks

Run `tools/check-linux.sh` with the declared Rust toolchain. Tests cover the
provider-neutral engine, accelerator no-fallback behavior, QMP/QGA framing,
Unix process groups, state identity, schema gates, and crash recovery.

The Linux state contract is `0700` directories and `0600` files owned by the
effective user. Symlinked final components are rejected with `O_NOFOLLOW`, and
open directory/file device and inode identities are rechecked before authority
use. A different same-user process with write access to the admitted root is
still an external writer and invalidates acceptance.

## Deferred acceptance

- WSL2 validation is development evidence only.
- Real Linux/KVM lifecycle, QCOW2, QMP, and QGA acceptance requires a dedicated
  host and disposable image.
- Real macOS/HVF acceptance is deferred; M4 keeps the abstraction fail-closed.
