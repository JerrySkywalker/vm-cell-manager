# Platform Support Matrix

This document is rendered from the typed `SUPPORT_MATRIX` in `src/core/support.rs`.
Repository-local tests validate the source and require this file to match it byte for byte.

## Status vocabulary

| Status | Meaning |
|---|---|
| `supported` | Accepted for the named release by declared real-platform evidence. |
| `experimental` | Real-platform evidence exists, but the path is not a release guarantee. |
| `development-only` | Repository, mock, fake-protocol, or WSL2 evidence only; not real-platform acceptance. |
| `untested` | The path is intended, but its required real-platform acceptance is absent. |
| `unsupported` | The combination is rejected or not implemented and must not be selected. |

## Declared combinations

| Host OS | Host architecture | Provider | Accelerator | Guest OS | Guest architecture | Guest transport | Status | Acceptance evidence |
|---|---|---|---|---|---|---|---|---|
| windows | x86_64 | hyperv | hyper-v | windows | x86_64 | powershell-direct | `untested` | none |
| windows | x86_64 | hyperv | whpx | windows | x86_64 | powershell-direct | `unsupported` | none |
| windows | x86_64 | qemu | whpx | windows | x86_64 | qga | `unsupported` | none |
| windows | x86_64 | qemu | whpx | linux | x86_64 | qga | `untested` | none |
| windows | x86_64 | qemu | whpx | linux | x86_64 | ssh | `unsupported` | none |
| windows | x86_64 | qemu | tcg | linux | x86_64 | qga | `development-only` | none |
| linux | x86_64 | qemu | kvm | windows | x86_64 | qga | `unsupported` | none |
| linux | x86_64 | qemu | kvm | linux | x86_64 | qga | `untested` | none |
| linux | x86_64 | qemu | kvm | linux | x86_64 | ssh | `unsupported` | none |
| linux | x86_64 | qemu | tcg | linux | x86_64 | qga | `development-only` | none |
| macos | x86_64 | qemu | hvf | linux | x86_64 | qga | `untested` | none |
| macos | x86_64 | qemu | tcg | linux | x86_64 | qga | `development-only` | none |

An absent combination is undocumented and must fail closed; it never inherits support from a similar row.
No current row is `supported` or `experimental`. Repository CI, mocks, fake protocols, and WSL2 development evidence cannot promote a row; those statuses require declared real-platform acceptance evidence.
