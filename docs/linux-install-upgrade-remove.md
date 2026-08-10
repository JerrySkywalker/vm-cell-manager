# Linux install, upgrade, and removal

This is the unprivileged portable-package contract for the
`x86_64-unknown-linux-gnu` candidate. It does not install QEMU or system
packages, alter `/dev/kvm`, load modules, configure networking, create a VM, or
grant provider authority.

## Install

Verify the exact archive before extraction:

```sh
sha256sum --check SHA256SUMS.txt
```

Inspect the versioned top-level directory, then install into an ordinary
user-owned prefix. This example uses `$HOME/.local`; choose another explicit
user-owned prefix if appropriate.

```sh
archive=vmcell-v0.2.0-linux-x86_64.tar.gz
staging=$(mktemp -d)
tar -xzf "$archive" -C "$staging"
layout="$staging/vmcell-v0.2.0-linux-x86_64"

install -d "$HOME/.local/bin"
install -m 0755 "$layout/vmcell" "$HOME/.local/bin/vmcell"
install -d "$HOME/.local/share/bash-completion/completions"
install -m 0644 "$layout/completions/vmcell.bash" \
  "$HOME/.local/share/bash-completion/completions/vmcell"
install -d "$HOME/.local/share/zsh/site-functions"
install -m 0644 "$layout/completions/_vmcell" \
  "$HOME/.local/share/zsh/site-functions/_vmcell"

"$HOME/.local/bin/vmcell" --version
"$HOME/.local/bin/vmcell" --help
"$HOME/.local/bin/vmcell" doctor
```

The example version follows the current repository candidate and is aligned to
`0.3.0` during v0.3 closeout. Do not use an archive whose version, checksum,
target, or recorded glibc floor does not match the intended installation.

## Upgrade and rollback

1. Stop other vmcell commands using the same state root. Do not stop provider
   objects merely to replace the program.
2. Retain the previous binary and a recoverable copy of the state root. Keep
   registered immutable base images in place.
3. Verify and stage the new archive separately.
4. Run `NEW_VMCELL --state-root PATH state check` before its first mutation.
   Stop on upgrade-required, integrity, or ambiguous state.
5. Replace the user-prefix binary and completion files only after version,
   help, state-check, and doctor results are understood.

Rollback selects the preserved old binary while leaving durable state and base
images untouched. It is not a schema downgrade and never authorizes manual
state editing.

## Remove

Use `status`, cell inspection, and operation reconciliation to decide how to
handle retained runtime state. Never adopt or delete a QEMU process by name,
PID, or socket alone.

Remove only the files installed by this portable package:

```sh
rm -- "$HOME/.local/bin/vmcell"
rm -- "$HOME/.local/share/bash-completion/completions/vmcell"
rm -- "$HOME/.local/share/zsh/site-functions/_vmcell"
```

Remove now-empty program directories only after verifying their exact paths.
Do not delete vmcell state roots, registered base images, retained cells, or
artifacts as part of program removal. Those require separate explicit cleanup
decisions through the ownership and reconciliation model.

