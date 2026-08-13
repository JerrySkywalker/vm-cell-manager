# Version-neutral reliability closeout

## Status and authority

Packets A-G are the repository-local reliability stream admitted by issue #48.
The stream is complete when this closeout is merged to `dev`, its exact candidate
and merged-`dev` hosted Windows/Linux qualifications are terminal green, and the
final review records P0=0/P1=0. The authoritative run and review identifiers are
recorded on issue #48 so this document does not turn an old run into evidence for
a later tree.

This closeout is version-neutral. It does not set version 0.8.0, create or move a
release branch or tag, publish a package, operate a provider or guest, accept a
real platform, or promote a support row. Frozen v0.1-v0.4 refs remain immutable.

| Packet | Repository-local status | Result | Merge |
| --- | --- | --- | --- |
| A | `COMPLETE_REPOSITORY_LOCAL` | fixed-seed lifecycle corpus and bounded minimizer | PR #51, `090fe32f7e8df1291e076efa8567c7f6b643d4ee` |
| B | `COMPLETE_REPOSITORY_LOCAL` | mutation isolation, receipt identity, reaping, and no-replay process fixtures | PRs #52-#53, through `79122f08543b5fbd25f6bca5c31514d9898f3212` |
| C | `COMPLETE_REPOSITORY_LOCAL` | deterministic run-selection, JobSpec/result, and durable-correlation matrices | PR #54, `2b3325e62c243747e34ca6922af8de8dda518f85` |
| D | `COMPLETE_REPOSITORY_LOCAL` | hostile protocol/path hardening, artifact phase integrity, and Windows process-receipt clamp | PRs #47, #55-#56, through `f4b89c5573604afcbf2ab0be5c5678fb8d4e2f41` |
| E | `COMPLETE_REPOSITORY_LOCAL` | strict sanitized acceptance-receipt validator | PR #57, `2193dfdeace4251344dfacb1adee10f9e46b10bb` |
| F | `COMPLETE_REPOSITORY_LOCAL` | bounded R3 campaign, R4 timing, and disposable Windows correctness split | PR #58, `35fc540cd47d0fca9d6f6ce34e202b191fe0a7e3` |
| G | `PENDING_TERMINAL_QUALIFICATION` in a candidate; `COMPLETE_REPOSITORY_LOCAL` only after merge and issue receipt | this reproducibility, limits, compatibility, topology, and closeout contract | this shallow closeout slice |

## Reproduction and campaign bounds

`tools/reliability-campaign.json` is the allowlist, not a suggestion. Its v1
contract names exactly five ignored tests and Rust 1.85.0. Two lifecycle cases
use SplitMix64 seed `6a09e667f3bcc909`; the model matrices use the literal
`fixed-v1` vector set. The committed manifest SHA-256 is
`78ea61cf3e47d6337e3f75ad5b5eb8c7d7e097214699926638f9ca55ba28b1da`.
The lifecycle generator is fully specified by source,
seed, and case index. Its v1 minimizer examines at most 31 non-canonical
transition cases and emits canonical JSON containing the contract, seed, source
case index, and normalized transition. A failure is reproduced with the exact
candidate checkout and the one target/test name printed by the campaign:

```text
cargo test --locked --offline --test <target> <test> -- --ignored --exact
```

Changing a seed, vector set, case name, count, minimization order, or bound is a
new campaign contract and requires review. Environment randomness, time, PID
reuse, provider state, guest state, and network results are not seeds or oracles.

The R3 bounds are exact: five cases; 120 seconds per registration or execution
subprocess; 600 seconds around the campaign; 15 minutes around the fresh hosted
job; and at most 1,048,576 captured bytes per subprocess. The capture reader
drains the complete stream and fails on overflow without imposing a file-size
limit on Cargo, rustc, or target artifacts. The no-replace, mode-0600 receipt is
kept under runner temporary storage, binds the exact source SHA and manifest
SHA-256, and is never uploaded. A missing, partial, overflowed, timed-out, or
nonzero case produces no PASS receipt.

## Evidence separation

| Tier or gate | Evidence meaning | Explicit non-claim |
| --- | --- | --- |
| R0 | pure fixed-vector/model contracts and deterministic unit fixtures | process, provider, guest, or host behavior |
| R1 | bounded subprocess, crash, lock, cancellation, and exact-reaping fixtures | real hypervisor/process-tree acceptance |
| R2 | hostile local parser, path, QMP/QGA stream, output, and redaction fixtures | real QMP/QGA interoperability |
| Disposable Windows/Linux correctness | exact-source compile, static safety, full tests/doc-tests, and package contract on a fresh hosted x64 image | R4 health, performance, or real provider support |
| R3 | the manual exact-source five-case hosted-Linux extended campaign | canonical correctness, soak, KVM, provider, guest, or support acceptance |
| R4 | outcome and sanitized timing on the shared self-hosted Windows environment under the unchanged 30-minute contract | repository failure when contention alone prevents completion |
| R5 | separately authorized, release-specific receipt from an isolated dedicated real host | publication or acceptance of any other candidate/tuple |

R0-R4 never promote support. Hosted correctness decides repository correctness;
R4 remains optional performance/diagnostic evidence and cannot override a hosted
failure or establish correctness after no hosted run. R5 is the only tier that
can establish the named real-platform tuple, and even R5 does not publish it.

## Residual equal-user race limits

The mutation guard, pinned handles, no-follow opens, object identity checks,
pre/post validation, provider snapshots, and exact-owned cleanup make
replacement observable and fail closed. They do not make a hostile process with
the same effective user and write access to the admitted state, runtime, image,
configuration, socket, or guest workspace namespace impossible.

In particular, an equal-user writer may race a pathname after one validation,
replace and restore an entry, mutate content through another hard link, consume
watch capacity, or change guest files with administrator-equivalent authority.
Detected identity, watch, receipt, or provider drift invalidates the operation;
unknown guest effects stay nonterminal, retained, and nonreplayed. It never
authorizes adoption, cleanup by name, automatic repair, or evidence promotion.
On Windows, a QEMU leader exit, PID/start-token/executable match, or
`CREATE_NEW_PROCESS_GROUP` membership does not prove descendant absence. Frozen
v0.3/v0.4 WHPX candidates still lack the atomic Windows Job Object binding and empty-tree proof.
Current corrected source supplies that repository mechanism,
but it becomes R5 evidence only after a newly authorized exact-candidate run on
a dedicated Windows host; no old receipt transfers.

Real acceptance therefore requires an exclusive, ACL- or mode-enforced writer
window and a private ordinary state/runtime root. A shared user identity or an
unattributed external writer is a failed admission condition, not a race that
repository tests can prove absent.

## Compatibility and upgrade matrix

| Frozen source | Durable/spec/package surface | Current-dev rehearsal contract | Promotion consequence |
| --- | --- | --- | --- |
| v0.1.0 `32f4adad3881c5248c6c8c5d47982368b7b55799` | format-1 installation/image/cell/operation/artifact state; no JobSpec and no required `artifact_pruned_at` | validate and read supported v1 records without creating, backfilling, or rewriting them; absent optional prune time means no recorded prune; reject unknown schema before mutation | frozen package and old CI remain historical only; any corrected candidate needs renewed package, CI, and R5 receipts |
| v0.2.0 `ed2ed31ae2f0182fc1626321b81e86d09db378c2` | format-1 state with optional `artifact_pruned_at`, plus state-check/upgrade guidance | same no-rewrite v1 behavior; malformed, unsafe, identity-mismatched, reparse, or future schemas fail closed | old evidence cannot be backported; requalify the corrected tree |
| v0.3.0 `d0af04b2e84cf2226628173d2ed0d295aed01f2b` | format-1 QEMU/QMP/QGA state and Windows/Linux portable packages | read supported v1 state without rewrite; no current field may infer ownership, containment, or terminal guest effects | Windows WHPX promotion also requires atomic Job Object/empty-tree proof on a corrected tree |
| v0.4.0 `c741be99ef4632b436f394f1c53b71ed57d0d2d9` | legacy/direct v1 plus job-correlated v2 cell/operation/artifact records and JobSpec/result contracts | preserve v1 records; require correlation for v2; reject inconsistent correlation or unsupported schema; never rewrite on `state check` | overlay acceptance must be repeated against an independently accepted corrected base tuple |
| v0.3 reader against v0.4 v2 state | v0.3 has no job-correlation schema | return `vmcell.state.upgrade_required` before mutation; never drop v2 provenance | use v0.4-or-newer for the root; downgrade is unsupported |
| direct command on a retained v0.4 job cell | cell remains correlated v2; later direct operation is deliberately uncorrelated v1 | read both without inferring or backfilling correlation | direct operation does not inherit job authority or replay rights |

`state check` is the provider-free read/reject rehearsal: compatible v1/v2 state
is read in place, unsupported schema returns `vmcell.state.upgrade_required`, and
integrity ambiguity stops mutation. It is not a migrator or package-upgrade PASS.
The frozen fixtures and corrected-candidate impact matrix are separate follow-up
artifacts; this table neither mutates frozen refs nor declares them promotable.

## CI topology and future dedicated option

The long-term topology has three mechanically distinct purposes:

1. GitHub-hosted Windows/Linux x64 jobs are disposable repository-correctness
   gates. They use exact source, read-only contents, no persistent credentials,
   no trusted cache, no secret/OIDC input, and no automatic fork execution.
2. The current shared self-hosted Windows runner is R4-only, optional for merge
   correctness, and useful for performance/diagnostic observation. Its existing
   30-minute contract remains unchanged.
3. Dedicated real Windows/Linux/macOS hosts are R5 provider-acceptance systems.
   They require explicit owner authority, a release-specific tuple and receipt,
   exclusive host/state-root control, and pre/post foreign-state proof.

The repository dispatcher keys concurrency by selected lane plus exact source
SHA. Cross-lane evidence may run or wait independently; a new pending request
cannot evict another lane. Same-lane, same-SHA requests remain deduplicated and
`cancel-in-progress: false` protects an active qualification.
Each dispatch selects one lane; non-selected reusable bridge jobs are expected
to be skipped. Skipped jobs and runs canceled before a terminal selected job are
never PASS evidence. The hosted Linux correctness lane owns normal format,
check, Clippy, full test/doc-test, and package coverage; R3 owns only its fixed
five-case campaign. Reusable source input must remain equal to caller, job
workflow, and checked-out SHAs.

A future dedicated `vmcell` self-hosted R4 runner is implementation-ready but
not authorized here. Its owner packet should provision a separate Windows x64
host or VM and OS identity with no co-resident runner, build, or scheduled-task
work; register it only to this repository with exclusive labels such as
`self-hosted`, `windows`, `x64`, `vmcell`, `r4-dedicated`; use a private runner
work root and separately bound Rust/Cargo state; retain credential isolation and
the existing command/timeout contract; and prove listener/service ownership,
zero foreign worker trees, clean residue, and bounded idle/load samples before
one exact-SHA trial. Only a later shallow workflow PR may select those labels.
That packet must not repurpose the current shared runner or use R4 as R5.

## Closed and deferred

Repository-local A-G reliability work closes with the issue #48 terminal
receipt. Remaining work is deliberately outside this stream: frozen-candidate
correction strategy; version-neutral compatibility fixtures and contract gaps;
dedicated-host R5 execution; runner provisioning; v0.5-v0.7 product features;
any v0.8 version or branch; release publication; and support promotion.
