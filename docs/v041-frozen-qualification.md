# v0.4.1 frozen candidate qualification

Contract: `vmcell.v041-frozen-qualification.v1`.

This is the candidate-specific qualification and no-publication release
rehearsal for `release/v0.4.1`. It records repository and package evidence; it
does not authorize R5, a host or provider action, a tag, a GitHub Release,
package publication, `main` promotion, or a support-status change.

## Exact boundary and disposition

| Field | Binding |
| --- | --- |
| Goal | `V041-FROZEN-QUALIFICATION-RELEASE-READINESS-12H-001` |
| Window start | `2026-08-14T01:54:49.3585547+08:00` |
| Candidate | `release/v0.4.1@0e7fcf37f4310562d318f9d5c709ddf8e8ca1637` |
| Candidate tree | `18c2e81acc4db57e2275175b138d31049df000da` |
| Main observed | `c965d9f113fb3a9dca85161079738cda95411d2b` |
| Qualification tooling merge | PR #67, `dev@658a74f1441994381a58dedd90ee765a66892fbb` |
| R5 | `NOT_EXECUTED` for all four packets |
| Support | unchanged; all intended real-platform rows remain `untested` |
| Candidate disposition | `PROMOTION_ELIGIBLE_PENDING_R5` |

The qualification tooling is post-freeze repository evidence. It checked out
and bound the exact candidate where required, but its `dev` commits are not in
the candidate tree and must not be represented as candidate source.
R5 is `NOT_EXECUTED` and support remains `untested`.

## Reproducibility and packages

Two independent exports of the exact candidate contained 181 files and had an
identical normalized content-manifest digest
`b45b71e3ab13c164d0911da6620ff0b4c2a4442dd3106650d88710610162bd2f`.
The source archive digest was
`054a8a1821b96d304bc0c542ac5aee2d3709dd780c1c40da2ec30a0e243c6478`.

Windows clean builds on Rust 1.85 were not byte-reproducible. Three binaries
had equal size but different hashes:

- `46537173dce5104e993aceb7f1859398118c963cc4d62be782de15bcfc1e5336`;
- `275491656c1c0a3b8e8d7c58d00fa4fb141f8180fb28ac6c81b2818854e3a188`;
- `446942f4e4dce8b2df7ed610dc93177bc138d08bb436d41f7b791f1395503c1b`.

Object inspection found differing linker timestamps, and the binaries retained
absolute source/Cargo paths. A fixed input binary did produce byte-identical
ZIP and checksum output on repetition. This proves deterministic assembly at a
fixed binary/tool boundary; it does not prove Windows binary, cross-host, or
cross-toolchain determinism.

The retained trusted Windows candidate package from run
[`31730186128`](https://github.com/JerrySkywalker/vm-cell-manager/actions/runs/31730186128)
passed source/version/layout/checksum/provenance checks:

| Windows object | SHA-256 |
| --- | --- |
| `vmcell-v0.4.1-windows-x86_64.zip` | `3802a045148849c2dc7a385e2fee43865336dbd3d12ea64347503713230324b7` |
| adjacent `SHA256SUMS.txt` | `ad0825847013090138ddfd7ab899a13b3d5588e0b60c893760eb2c8f27804a03` |
| packaged `vmcell.exe` | `249db6841161d634449142584ad7924b26cbe7b31a41eca9b813dd2eb8acec1b` |

The binary reports `vmcell 0.4.1`, targets
`x86_64-pc-windows-msvc`, was built with Rust/Cargo 1.97.1, and is not
Authenticode-signed. Unsigned status is a disclosed release-plan limit, not a
signature claim.

The retained Linux candidate package from post-freeze workflow run
[`31744783947`](https://github.com/JerrySkywalker/vm-cell-manager/actions/runs/31744783947)
checked out the exact frozen source and passed both checksum layers, all 10
payload-manifest entries, layout/provenance validation, and unprivileged
install/remove smoke:

| Linux object | SHA-256 |
| --- | --- |
| `vmcell-v0.4.1-linux-x86_64.tar.gz` | `0a258f4f838f38ed632e80a2aec8e2ae6526de6656b21ddde77aeb13efa2999b` |
| adjacent `SHA256SUMS.txt` | `7ede5d30576da1ae36748f7add18d637a61c6697866c228026af0a134f64d42b` |
| packaged `vmcell` | `54d8371846edcc83bb2d9e49f8597a44d0abc003e5fedaabbcd1f21c0f80b575` |

Its provenance binds Rust/Cargo 1.85.0, Ubuntu 24.04 x86_64 glibc, target
`x86_64-unknown-linux-gnu`, and observed `GLIBC_2.39`. Repeated assembly was
byte-identical with the same binary and declared inputs; binary reproducibility
was explicitly false and is not claimed.

## Upgrade, rollback, and compatibility

`frozen_compatibility_contract` passed all eight v0.1-v0.4 fixture cases,
covering state/spec/job/package-layout read or reject behavior and no rewrite.
The exact candidate also passed `state check`, JobSpec/result/artifact
compatibility, and secret-bearing JobSpec rejection.

An actual frozen v0.1 package was built from
`release/v0.1.0`; its binary reported `0.1.0` and had SHA-256
`da4fc38565bffbed76e823019ef51ce3b05e95169a742e277027a3b7822690a9`.
The v0.1 package archive had SHA-256
`5557815dcc860b147ca84ef68e7b6ec96af84942f6552a2cd3b47c9739c028f9`.
The isolated v0.1 -> v0.4.1 -> v0.1 sequence passed no-clobber install,
upgrade, rollback, and recoverable remove while preserving user-state and
provider-object sentinels. A same-version Windows A -> B -> A package rehearsal
also preserved those sentinels.

The attempted clean historical binary build expansion hit its admitted
one-shot time cap after v0.1. Therefore fresh clean builds of v0.2-v0.4 are
`NOT_COMPLETED`; fixture compatibility is evidence for those versions, not a
substitute for unperformed historical binary builds.

## Provider-free reliability campaign

The long fixed-seed campaign executed 500 existing R3 cases and 125 extended
operation groups with no test failure, but its final summary object was lost to
a PowerShell boolean-literal reporting error. Its logs are corroborating
evidence only. A separate counted confirmation executed 50 R3 cases plus 50
extended lifecycle/state/job/image/receipt groups with zero failures in 29.55
seconds.

The counted confirmation observed the exact build target unchanged at 2,099
files and 1,020,575,090 bytes before and after, zero temporary objects before
and after, and an unchanged source manifest
`13de8289eb342992033c66152e29fd2de876716c39d231efde2a365506e841d`.
No real VM, provider, QGA, guest, or host action occurred.

## R5 contract rehearsal and owner handoff

[`v041-r5-contract-rehearsal.json`](receipts/v041-r5-contract-rehearsal.json)
and `tools/test-v041-r5-contract-rehearsal.ps1` validate all four packet types:

| Packet | Dry run | Real result | Support |
| --- | --- | --- | --- |
| `V041-R5-HYPERV-PSD-V1` | `PASS` | `NOT_EXECUTED` | `untested` |
| `V041-R5-WHPX-QGA-V1` | `PASS` | `NOT_EXECUTED` | `untested` |
| `V041-R5-KVM-QGA-V1` | `PASS` | `NOT_EXECUTED` | `untested` |
| `V041-R5-JOBSPEC-OVERLAY-V1` | `PASS` | `NOT_EXECUTED` | `untested` |

The adversarial rehearsal rejects candidate drift, cross-tuple substitution,
preflight represented as `PASS`, support promotion, an overlay without an exact
`PASS` base, and A4 input used as A5 authority. The JSON contains the minimum
dedicated-host prerequisites and exact A4/A5 one-command handoffs. A4 uses
`PROTECTED_PREFLIGHT_V2` and can produce at most `PREFLIGHT_PASS`; A5 uses
`PROTECTED_TRANSACTION_V2` and always requires a fresh owner goal.

## Security and supply chain

- `cargo audit 0.22.2` checked the exact lockfile against a 1,216-advisory
  RustSec database snapshot updated 2026-08-12: zero vulnerabilities and
  warnings. The database commit was not retained as a complete 40-character
  binding, so no exact database-commit claim is made.
- `Cargo.lock` SHA-256 is
  `fcf0545c48faa2f413d4560d073a3c6b5fbdb0c40858d1238ed56eb00296009c`.
  All 102 external packages are crates.io registry entries with checksums; no
  Git or unknown dependency source was found.
- The candidate scan found no private-key or token-prefix literal and no
  untrusted pull-request workflow using secrets, OIDC, or write permission.
- Archive traversal/path-confusion, package layout, no-clobber install/remove,
  and preservation contracts passed. Post-freeze workflows pin external
  actions by immutable SHA.
- Automated `cargo deny` installation exceeded the one-shot 20-minute cap.
  License-policy automation is `TECHNICAL_FAILURE`; manual metadata found the
  previously omitted `MPL-2.0` expression for `option-ext`, and PR #67 added it
  to the policy. No dependency-license approval beyond the recorded policy is
  inferred.
- GitHub Dependabot alerts were unavailable (`403`, feature disabled). No token
  scope was widened to compensate.

PR #67 also supplied the missing exact-source retained Linux packaging lane,
all-four-packet R5 contract rehearsal, immutable action pins, and a bounded
four-job self-hosted Cargo setting after observed commit-limit exhaustion.
These are post-freeze qualification/governance corrections. No proven defect
in the candidate runtime/package behavior was release-critical; a future
release-critical finding must retire this candidate as
`RETIRED_NEW_CORRECTION_REQUIRED`.

## No-publication release rehearsal

The machine-readable plan is
[`v041-release-rehearsal.json`](v041-release-rehearsal.json). It binds both
retained platform assets, a combined-checksum/provenance staging plan, release
notes, exact `main` comparison, immutable tag policy, rollback/patch policy,
unchanged support rendering SHA-256
`8017c6b5fa5b10c4d4041ef02844eb2902a94cc92faff1ea10804ad6afc75481`,
and terminal stop conditions.

At rehearsal time `main...release/v0.4.1` contained 127 changed files, 36,668
insertions, and 3,092 deletions. No tag points at the candidate, no GitHub
Release exists, and no registry or package-manager publication was performed.
Promotion must use the exact frozen candidate—not later qualification-only
`dev` commits—and still requires all declared R5 packets plus separate owner
authority.
