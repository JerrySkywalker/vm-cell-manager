# Durable-state compatibility and recovery

## v0.4 job correlation and the narrow v2 fence

v0.4 reads legacy/direct durable records at version 1 and job-correlated
records at version 2. A cell created by `vmcell run --spec` carries one
immutable job correlation: a fresh job ID, the exact validated job-spec
SHA-256, and its start time. Operations dispatched by that job, and artifacts
committed from those operations, carry the matching job ID. Those correlated
cell, operation, and artifact records are version 2; a v1 record must not carry
job correlation, and a v2 record must carry it. A later direct command on a
retained job cell deliberately remains an uncorrelated v1 operation.

This narrow per-record fence prevents a v0.3 binary from silently dropping
job provenance: its exact v1 schema validation refuses each v2 record before
any mutation. v0.4 never upgrades, backfills, or rewrites a v1 record merely
because it reads it. `state check` reports the maximum readable durable record
format: `1` for legacy/direct-only state and `2` whenever readable correlated
records are present.

The correlation is bounded provenance only: it contains no specification bytes,
command, credentials, paths, guest streams, provider proof, ownership grant,
idempotency key, replay permission, or cleanup authority.

`state check`, normal operation reads, and artifact inspection validate the
cell-to-operation-to-artifact binding and reject inconsistent binding or a
duplicate job ID across cells. The StateStore mutation APIs also keep the
correlation immutable on managed writes. Direct commands on a retained job cell
remain intentionally uncorrelated. The existing cell and guest-operation state
machines remain authoritative for lifecycle, reconciliation, cleanup, and
non-replay. A field-omitting legacy operation whose old cell record is absent
remains readable as historical evidence, but never substitutes for the cell
authority required by recovery or mutation. Managed durable-record writes,
reconciliation, and public artifact pruning reject it without rewriting the
record until the exact parent cell is present and valid.

The fields remain additive when reading v1 historical state. Their absence means
legacy or direct state with no known correlation; vmcell never infers,
backfills, or rewrites them just because a newer binary reads the root. A v1
reader from before v0.4 therefore refuses a v2 record rather than dropping its
provenance. Use a feature-bearing v0.4-or-newer binary for any mutation of a
job-correlated root, or keep a recoverable copy before a deliberate downgrade
rehearsal. A v0.4 reader may accept field-omitting legacy records, but cannot
reconstruct provenance deliberately removed by a same-version external writer;
that loses observability only and never grants lifecycle, ownership, cleanup,
or replay authority.

## v0.1 to v0.2 contract

The frozen repository-local v0.1 candidate and v0.2 use durable-state format
version 1. Installation, image, cell, guest-operation, artifact, and ownership
records keep their existing schema version 1 layouts. The additive
`artifact_pruned_at` guest-operation field is optional when reading v0.1 state;
an absent field means that no artifact-prune tombstone was recorded. No v0.1
record is rewritten merely because a v0.2 binary reads it.

Format version 1 is a compatibility classification produced only after the
current binary validates readable durable records and their individual schema
and identity gates. It is not a new marker silently written into an old state
root. Malformed JSON, unknown schemas, identity mismatches, unsafe/reparse
paths, and invalid active artifact proofs remain fail-closed.

## Read-only upgrade preflight

Stop other vmcell commands that use the state root, keep the old binary and a
recoverable copy of the state root, then run:

```powershell
vmcell --state-root C:\path\to\state state check
vmcell --json --state-root C:\path\to\state state check
```

`state check` does not create a missing root, acquire a mutation lock, contact a
provider, open registered base-image content, replay guest work, reconcile an
operation, or rewrite JSON. It validates the ordinary state root and all
readable installation, image, cell, operation, and active operation-bound
artifact records. A compatible report uses contract
`vmcell.state-compatibility.v1`, durable format version `1` for
legacy/direct-only records or `2` when readable v0.4 job-correlated records are
present, and status `empty` or `compatible`.

An unsupported record schema returns `vmcell.state.upgrade_required` and exit
code 9. Stop: do not run a mutating command with that binary. Keep the state and
base images unchanged, return to the binary that owns the newer schema, or use a
future explicitly documented migrator. Other malformed or unsafe records return
the normal integrity classification and also require investigation before any
mutation. There is no automatic downgrade or repair path.

Tombstoned, partially pruned, or quarantined artifact subtrees are handled by
their existing recovery paths and are not counted as active compatibility
evidence. A compatibility PASS therefore proves that supported readable records
and active artifact proofs can be read safely; it is not a provider-health,
base-image-content, or real-platform acceptance result.

## Interruption and recovery matrix

`vmcell run` observes Windows Ctrl-C cooperatively at durable orchestration
stage boundaries. Readiness probes and guest commands remain bounded by their
configured deadlines and are not asynchronously replayed or detached by the
observer.

| Observed point | Durable meaning | Cleanup behavior |
| --- | --- | --- |
| Before a guest operation is recorded | No guest side effect was dispatched | Exact-owned cleanup follows `--keep` / `--keep-on-failure` policy. |
| After readiness transport becomes active but before command dispatch | Operation remains nonterminal because transport was active | Cell is retained; cleanup is refused as ambiguous. |
| During a bounded readiness probe or guest action | Ctrl-C is sampled at the next stage boundary; the bounded action's result or timeout classification wins | Unknown effects remain nonterminal, retained, and nonreplayed. |
| After a command completed | Exit/output metadata and operation ID are durable | Cleanup follows the requested policy; interruption is still reported. |
| During cleanup or after process crash | Existing cell/operation phase is authoritative | Use `status`, `inspect`, and `operation inspect/reconcile`; never destroy by provider name. |

`--keep` always retains a proven cell. `--keep-on-failure` retains known failed
runs; ambiguous guest/provider effects are retained regardless. Expired retained
cells are only considered by explicit `gc`, which skips nonterminal operations
and delegates eligible deletion to the exact-owned destroy path.

For a job, a known committed artifact remains bound to its durable operation and
can be inspected without guest contact. A copy, artifact collection, or guest
operation whose transport became active without a known terminal result remains
uncertain, retains the cell, and is never replayed merely because its job ID or
specification digest matches a later invocation.
