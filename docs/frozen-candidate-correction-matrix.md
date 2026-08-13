# Frozen v0.1-v0.4 candidate correction matrix

## Authority and recommendation

The release refs below remain immutable historical source snapshots. This audit
does not rewrite them, create a release branch, change `Cargo.toml`, publish a
package, accept a platform, or promote support.

All four frozen candidates are retired from any future promotion decision. This
does not allege a known compromise and does not erase their historical CI or
design value. It means their exact trees omit later fail-closed corrections, so
old package, CI, or real-platform evidence cannot be attached to a corrected
binary.

Recommended owner option: one consolidated corrective candidate from a freshly
qualified current `dev`, with a new version/ref only after a separate release
decision. This minimizes divergent manual ports, preserves the cumulative
compatibility fixes, and gives one truthful source/package/receipt identity. If
the first public candidate is intentionally v0.5 instead, the same correction
floor remains mandatory. Q, S, and A can each be semantically reimplemented on
an old line, but none is a clean evidence-preserving cherry-pick; Q and S are
high-risk cross-surface ports, while only A is a narrow bounded port. Separate
historical corrective candidates are not recommended.

## Frozen candidate and tuple inventory

| Candidate | Frozen source | Affected tuple or overlay | Promotion disposition |
| --- | --- | --- | --- |
| v0.1.0 | `32f4adad3881c5248c6c8c5d47982368b7b55799` | Windows/x86_64 Hyper-V + Windows guest + PowerShell Direct | retire exact candidate; correction rows S and A apply |
| v0.2.0 | `ed2ed31ae2f0182fc1626321b81e86d09db378c2` | v0.1 tuple plus repeated session/image/state behavior | retire exact candidate; correction rows S and A apply |
| v0.3.0 | `d0af04b2e84cf2226628173d2ed0d295aed01f2b` | Windows/x86_64 QEMU/WHPX + Linux guest + QGA | retire exact candidate; Q, S, and A apply; Job Object work remains missing |
| v0.3.0 | same source | Linux/x86_64 QEMU/KVM + Linux guest + QGA | retire exact candidate; S and A apply; Windows-only Q row does not apply |
| v0.4.0 | `c741be99ef4632b436f394f1c53b71ed57d0d2d9` | JobSpec/result overlay on an accepted v0.1/v0.3 tuple | retire exact candidate; S and A apply, plus Q when the base is Windows QEMU/WHPX |

## Authoritative correction rows

| ID | Exact defect in frozen source | Exact correction SHA and dependency | Affected frozen candidates | Backport feasibility | Minimum corrected source tree |
| --- | --- | --- | --- | --- | --- |
| Q | Windows `process_group_absence_proven` returned `true`; leader absence plus `CREATE_NEW_PROCESS_GROUP` was treated as descendant-tree absence without atomic containment | direct clamp `f10ea52a9dddf62adf115225ae0f9d83b5f298da`; functional QEMU receipt ancestry `834e06e4ae9c68ac0dc9a00c2c7674b00f192bd4`, `ab6bcfe306ba89a308043d9843a0472c15399058`, `225aea6886c8042d71ccd6dd67121828b53997e9`, `2dd20814efa9cd5d43693f9cc774c8c7475508d8` | v0.3 Windows QEMU/WHPX and v0.4 overlays using that base | no clean cherry-pick; high-risk manual port because receipt/recovery APIs evolved | frozen v0.3/v0.4 tree plus all named QEMU receipt ancestry and `f10ea52`; this only fails closed and still needs new atomic Windows Job Object plus empty-descendant-tree implementation before R5 |
| S | mutation guards validated a filesystem handle but were not bound and revalidated pre/post against the exact state root across durable/destructive call sites | behavior `06934a5b8ce93b80f5fb2b1fc7353a070751e784`; Windows test/cfg follow-up `97e5054f4c7a7d231c2c97c9c6615574f7b83299` | v0.1-v0.4; Hyper-V, QEMU/WHPX, QEMU/KVM, and JobSpec paths | technically possible only as a high-risk manual port; frozen APIs lack the state-root binding and converted call sites | exact frozen tree plus `MutationGuard::validate_for_state_root`, every engine/state pre/post call-site conversion, nested runtime/artifact/image/cell validation, and the cross-process replacement/lock regression suite from both named commits |
| A | `ArtifactCommitted` validated artifact fields but did not restrict the operation kind; `Exec` and `CopyIn` records could pass that phase | `e50e1759cb3a1c003230d1de45a5a64c6f6283ce` | v0.1-v0.4 and every artifact-capable tuple/overlay | small semantically portable manual port; do not mutate frozen refs and do not transfer old evidence | exact frozen tree plus the `CopyOut`/`ArtifactCollect` phase guard and valid/invalid/external-tamper regression tests; retain v0.4 JobSpec correlation behavior |

The named fix SHAs are code provenance, not evidence that each change can be
applied independently or that the resulting tree is release-ready. A corrected
candidate must contain the union of every applicable row and all prerequisite
tests/contracts from current `dev`.

## Option comparison

| Option | Engineering cost and risk | Compatibility and version truth | Evidence renewal | Decision |
| --- | --- | --- | --- | --- |
| A — separate historical backports | four manually corrected candidate trees with different provider/schema surfaces; S and Q are conflict-prone and invite behavioral drift | keeps old feature labels but creates new binaries that are no longer the frozen versions; requires distinct corrective versions | package, Windows/Linux CI, audit, and every applicable R5 tuple per corrected tree | not recommended |
| B — one consolidated corrective candidate from current `dev` | one maintained tree containing the cumulative reliability train and compatibility checks | clearest truthful identity; preserves current v1/v2 read/reject/no-rewrite behavior | one candidate package/CI/audit set, then only explicitly claimed R5 tuples | **recommended** |
| C — defer the first public corrective candidate until v0.5 | lowest immediate release cost but delays a promotable candidate and couples correction availability to the externally blocked v0.5 lifecycle | truthful if v0.1-v0.4 remain explicitly retired and v0.5 includes the full floor | v0.5 package/CI/audit/R5 packet after its separate admission | acceptable fallback, not current recommendation |

Option B does not authorize version 0.8, v0.5, or any other version. An owner
release decision must choose the candidate version/ref, claimed tuples, and
publication policy. A corrected current-dev candidate should not masquerade as
the byte-identical frozen v0.1-v0.4 source.

## Required renewed receipts

For any later corrected candidate, bind every receipt to its new exact commit,
version, clean checkout, binary SHA-256, package manifest/hash, and tuple:

1. Rust 1.85 locked format, check, Clippy, full all-target tests, doc-tests, and
   workflow/static safety at exact candidate and exact merged `dev`.
2. Disposable hosted Windows x64 and Linux x64 repository-correctness PASS at
   both exact candidate and exact merged `dev`; R3 when the candidate changes a
   reliability contract. R4 remains optional performance/diagnostic evidence.
3. Rebuilt Windows and Linux portable packages with their source/version/layout,
   install/remove, and deterministic-assembly contracts. A frozen package hash
   cannot identify the corrected source.
4. A fresh P0=0/P1=0 independent audit of the exact candidate and exact merged
   tree, including the correction and compatibility matrices.
5. New R5 receipts for each tuple actually proposed: dedicated host, image and
   guest identity, base/overlay dependency, exclusive writer window, lifecycle,
   no-replay, artifact, cleanup, and terminal-state proof. Windows QEMU/WHPX
   additionally requires the not-yet-implemented atomic Job Object and empty
   descendant-tree proof.

No old receipt, CI run, package, PR merge, or release-ref name is backportable
as evidence. It remains historical context only.

## Owner decision gate

The owner-decision issue comparing A/B/C must link this matrix and record one of
`SELECTED`, `DEFERRED`, or `REJECTED`. Until a separate release decision is
`SELECTED`, the implementation-ready next action is limited to version-neutral
compatibility fixtures and contract hardening. It must not bump a version,
create a release branch/tag, publish, or execute a real provider.
