# Linux install, upgrade, and removal

This is the unprivileged portable-package contract for the
`x86_64-unknown-linux-gnu` candidate. It does not install QEMU or system
packages, alter `/dev/kvm`, load modules, configure networking, create a VM, or
grant provider authority.

## Install

Verify the exact archive before extraction:

```sh
sha256sum --check --strict SHA256SUMS.txt
```

Install the complete layout into a new versioned, private, user-owned
directory. Fresh install is no-clobber: stop if the exact version directory
already exists. Create each absent component of `$HOME/.local/lib/vmcell` one
at a time with mode `0700`; every existing component must be an ordinary
non-symlink directory owned by the effective user with mode `0700`. Stop on any
mismatch.

```sh
set -eu
archive=vmcell-v0.2.0-linux-x86_64.tar.gz
staging=$(mktemp -d)
tar -xzf "$archive" -C "$staging"
layout="$staging/vmcell-v0.2.0-linux-x86_64"
install_parent="$HOME/.local/lib/vmcell"
install_root="$install_parent/vmcell-v0.2.0-linux-x86_64"

(cd "$layout" && sha256sum --check --strict PACKAGE-CONTENTS.sha256)
test ! -e "$install_root" && test ! -L "$install_root"
mkdir -m 0700 -- "$install_root"
cp -R -- "$layout/." "$install_root/"
(cd "$install_root" && sha256sum --check --strict PACKAGE-CONTENTS.sha256)

"$install_root/vmcell" --version
"$install_root/vmcell" --help
"$install_root/vmcell" doctor
```

Add that exact versioned directory to the user PATH only after these checks.
For the current shell, source `completions/vmcell.bash` in Bash or add the
versioned `completions` directory to Zsh `fpath`. No command overwrites a shared
binary or completion target. Retain the verified archive/checksum or a trusted
copy of its content manifest as the removal identity.

The example version follows the current repository candidate and is aligned to
`0.3.0` during v0.3 closeout. Do not use an archive whose version, checksum,
target, or recorded glibc floor does not match the intended installation.

## Upgrade and rollback

1. Stop other vmcell commands using the same state root. Do not stop provider
   objects merely to replace the program.
2. Retain the previous versioned program directory and a recoverable copy of
   the state root. Keep registered immutable base images in place.
3. Verify and install the new archive into a different, previously absent
   versioned directory. Never install over the old version.
4. Run `NEW_VMCELL --state-root PATH state check` before its first mutation.
   Stop on upgrade-required, integrity, or ambiguous state.
5. Switch PATH/completion selection only after version, help, state-check, and
   doctor results are understood.

Rollback selects the preserved old versioned directory while leaving durable
state and base images untouched. It is not a schema downgrade and never
authorizes manual state editing.

## Remove

Use `status`, cell inspection, and operation reconciliation to decide how to
handle retained runtime state. Never adopt or delete a QEMU process by name,
PID, or socket alone.

Before removal, require a quiescent install directory with no concurrent
writer. Prove the root and `completions` are ordinary non-symlink directories
owned by the effective user. Compare the installed `PACKAGE-CONTENTS.sha256`
to the retained verified package copy, require every declared payload to be an
ordinary non-symlink file owned by that user, run `sha256sum --check --strict`,
and reject any missing, additional, replaced, or mode-drifted entry. Stop
without deleting anything on any mismatch.

Only after those checks, remove the fixed package-owned file list and then the
two now-empty directories:

```sh
rm -- \
  "$install_root/BUILD-PROVENANCE.json" \
  "$install_root/INSTALL.txt" \
  "$install_root/LICENSE.txt" \
  "$install_root/NOTICE.txt" \
  "$install_root/PACKAGE-CONTENTS.sha256" \
  "$install_root/PACKAGE-METADATA.json" \
  "$install_root/README.txt" \
  "$install_root/completions/_vmcell" \
  "$install_root/completions/vmcell.bash" \
  "$install_root/vmcell"
rmdir -- "$install_root/completions" "$install_root"
```

Do not delete vmcell state roots, registered base images, retained cells, or
artifacts as part of program removal. Those require separate explicit cleanup
decisions through the ownership and reconciliation model.

