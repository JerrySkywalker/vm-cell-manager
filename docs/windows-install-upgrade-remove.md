# Windows install, upgrade, and removal

This is the repository-local portable-package contract. No package manager,
tag, GitHub Release, Hyper-V feature, driver, service, switch, VM, or state root
is created by these instructions.

## Install

1. Obtain the ZIP and adjacent `SHA256SUMS.txt` from an accepted release or
   trusted candidate workflow.
2. Verify the ZIP SHA-256 against the exact checksum line before extraction.
3. Extract into a user-owned versioned parent such as
   `%LOCALAPPDATA%\Programs\vmcell\0.3.0`. The ZIP retains its deterministic
   top-level `vmcell-v0.3.0-windows-x86_64` directory.
4. Set `$installDir` to that nested directory, then run the exact binary's
   version, help, and doctor checks.
5. Add `$installDir` to the user PATH only after those checks.

```powershell
$extractDir = Join-Path $env:LOCALAPPDATA 'Programs\vmcell\0.3.0'
Expand-Archive -LiteralPath .\vmcell-v0.3.0-windows-x86_64.zip -DestinationPath $extractDir
$installDir = Join-Path $extractDir 'vmcell-v0.3.0-windows-x86_64'
$vmcell = Join-Path $installDir 'vmcell.exe'
& $vmcell --version
& $vmcell --help
& $vmcell doctor
```

PowerShell completion is optional and does not modify a profile automatically:

```powershell
. (Join-Path $installDir 'completions\vmcell.ps1')
```

For a durable profile entry, add that exact dot-source line yourself and remove
or update it during uninstall/upgrade. `vmcell completion powershell` prints the
same completion contract for custom layouts.

## Upgrade

1. Stop other vmcell commands that use the state root. Provider objects are not
   stopped or removed merely to upgrade the binary.
2. Keep the previous binary directory and a recoverable copy of the state root.
   Keep registered base images immutable and in place.
3. Verify and extract the new version into a different versioned directory.
4. With the new binary, run `vmcell --state-root PATH state check` before any
   mutation. The v0.1-to-v0.2 format-1 path is read in place without rewrite.
5. On `vmcell.state.upgrade_required`, integrity failure, or ambiguous state,
   stop. Do not repair JSON manually or run lifecycle commands with the new
   binary. Return to the binary that owns the state schema or follow a future
   explicit migrator.
6. Run `doctor`, then update PATH/completion to the new version.

Rollback means selecting the preserved old binary while leaving state and base
images untouched. It is not a state downgrade.

## Remove

1. Use `status`, `inspect`, and operation reconciliation to decide whether each
   retained cell can be safely destroyed. Never remove a VM by name alone.
2. Remove the vmcell program directory, PATH entry, and completion profile line.
3. Leave state roots, registered base images, retained cells, and artifacts in
   place unless a separate explicit cleanup decision authorizes them.

Deleting the portable program directory is intentionally not a data or provider
cleanup operation.

## Future package managers

`PACKAGE-METADATA.json` in the archive carries the stable package identifier,
version, portable binary/completion paths, and Scoop/WinGet installer-shape
fields. `publication_status` remains `candidate_only`; the file is input for a
later reviewed manifest/repository submission and does not publish anything.
