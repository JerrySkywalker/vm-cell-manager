# Linux portable package

The repository-local GNU/Linux distribution contract produces exactly:

```text
vmcell-vX.Y.Z-linux-x86_64.tar.gz
SHA256SUMS.txt
```

The target is `x86_64-unknown-linux-gnu`. This is a glibc-linked portable
archive, not a musl or fully static build. The deterministic archive layout is:

```text
vmcell-vX.Y.Z-linux-x86_64/
  BUILD-PROVENANCE.json
  INSTALL.txt
  LICENSE.txt
  NOTICE.txt
  PACKAGE-CONTENTS.sha256
  PACKAGE-METADATA.json
  README.txt
  completions/
    _vmcell
    vmcell.bash
  vmcell
```

The sibling `SHA256SUMS.txt` binds the completed archive. The in-archive
`PACKAGE-CONTENTS.sha256` binds every other regular payload file. Bash and Zsh
completions are generated from the exact packaged binary's Clap command graph.

## Declared build baseline

The canonical package gate runs only through the manual exact-SHA Linux
validation lane. Its declared baseline is GitHub-hosted Ubuntu 24.04 x86_64,
Rust and Cargo 1.85.0, the `x86_64-unknown-linux-gnu` target, and the glibc
userspace supplied by that declared hosted OS label. GitHub's `ubuntu-24.04`
label is rolling rather than an immutable image revision, so the exact `ldd`
identity is recorded in each provenance document. The lane uses
`readelf --version-info` to determine the highest `GLIBC_X.Y`
symbol version required by the exact binary. That observed floor is recorded in
both package metadata and build provenance. No older glibc compatibility is
claimed.

`BUILD-PROVENANCE.json` also binds the source SHA, commit-derived timestamp,
release profile, Rust/Cargo/Python identities, binary SHA-256, target, archive
format, and exact ordered layout. The contract deliberately says binary
reproducibility is **not claimed**. It proves byte-identical package assembly
when the already-built binary, declared inputs, and baseline tools are
identical; it does not overstate cross-runner compiler/linker reproducibility.

## Repository-local rehearsal

The normal exact-SHA Linux gate runs:

```sh
sh tools/check-linux-package.sh
```

`CARGO_TARGET_DIR` must already be bound outside the checkout. The gate builds
the locked release binary, assembles the archive twice, compares both archives
and checksum manifests byte-for-byte, validates normalized ustar ownership,
modes, timestamps, ordering and gzip header, rejects unsafe/duplicate/link
entries, checks all hashes and JSON metadata, and performs an unprivileged
temporary-prefix install/remove smoke test.

The smoke test runs `--version`, `--help`, `doctor`, and `status` against an
isolated nonexistent state root. Doctor may perform the existing non-mutating
KVM usability probe, including a read/write open of `/dev/kvm`; it issues no KVM
ioctl, creates no VM, and repairs no host condition.

The assembler requires an existing current-user-owned output parent that is
not group/world writable, pins that directory identity, stages under a private
hidden directory, and commits the two-file output with Linux
`renameat2(RENAME_NOREPLACE)`. Ordinary pre-commit failures clean only the
identity-matched private stage. A process kill can retain a hidden staging
directory, and an interruption at the terminal publication boundary can leave
a complete visible candidate; inspect the exact parent and verify both checksum
layers before deciding whether to retain or remove either artifact. No output
path is overwritten.

Install, upgrade, rollback, and removal guidance is in
[`linux-install-upgrade-remove.md`](linux-install-upgrade-remove.md) and the
archive's `INSTALL.txt`.

## Publication and acceptance boundary

This contract creates no apt/rpm repository, tag, GitHub Release, package
manager submission, or automatic public-fork build. The manual Linux lane has
read-only repository permission and validates one explicitly supplied exact
source SHA. Ephemeral toolchain setup and portable package assembly are
repository evidence only; they do not establish real QEMU/KVM/QGA acceptance.
