# Windows Portable Package

The repository-local Windows distribution contract produces two files:

```text
vmcell-vX.Y.Z-windows-x86_64.zip
SHA256SUMS.txt
```

The archive layout is deterministic and intentionally small:

```text
vmcell-vX.Y.Z-windows-x86_64/
  vmcell.exe
  LICENSE.txt
  NOTICE.txt
  INSTALL.txt
  BUILD-PROVENANCE.json
```

`BUILD-PROVENANCE.json` records schema version, package/version, target triple,
exact source commit and commit-derived timestamp, release profile, Rust/Cargo
versions, the binary SHA-256, and the ordered archive layout. ZIP entry order,
timestamps, names, and attributes are normalized. `SHA256SUMS.txt` binds the
finished archive.

## Repository-local build

From an exact source checkout with the locked Rust graph:

```powershell
cargo build --locked --release --bin vmcell
$epoch = [long](git show -s --format=%ct HEAD)
.\tools\package-windows.ps1 `
  -BinaryPath .\target\release\vmcell.exe `
  -OutputDirectory .\dist `
  -Version 0.1.0 `
  -SourceCommit (git rev-parse HEAD) `
  -SourceDateEpoch $epoch
```

Verify the published checksum before extraction:

```powershell
$actual = (Get-FileHash .\vmcell-v0.1.0-windows-x86_64.zip -Algorithm SHA256).Hash.ToLowerInvariant()
$expected = ((Get-Content .\SHA256SUMS.txt -Raw).Split(' ', 2)[0]).Trim()
if ($actual -ne $expected) { throw 'vmcell archive checksum mismatch' }
```

The packaging script verifies `vmcell --version`, rejects reparse inputs,
writes only its exact archive/checksum names under the selected output
directory, and does not install software or touch provider state.

## CI and release boundary

`.github/workflows/package-windows.yml` is manual `workflow_dispatch` only and
runs on the existing trusted core runner. It has no `pull_request` trigger, no
release/tag/content-write permission, and only uploads the bounded package as a
short-retention workflow artifact. The normal CI workflow builds the package
twice and requires byte identity, exact layout, and provenance/hash agreement.

This machinery is repository-local evidence only. It does not create a release
tag, promote `main`, publish a GitHub Release, or establish real Hyper-V or
PowerShell Direct acceptance. A future frozen release branch must bind its own
declared acceptance evidence before publishing these outputs.
