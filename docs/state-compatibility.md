# Durable-state compatibility and recovery

## v0.1 to v0.2 contract

The frozen repository-local v0.1 candidate and v0.2 use durable-state format
version 1. Installation, image, cell, guest-operation, artifact, and ownership
records keep their existing schema version 1 layouts. The additive
`artifact_pruned_at` guest-operation field is optional when reading v0.1 state;
an absent field means that no artifact-prune tombstone was recorded. No v0.1
record is rewritten merely because a v0.2 binary reads it.

Format version 1 is a compatibility classification produced only after the
current binary validates the active records and their individual schema and
identity gates. It is not a new marker silently written into an old state root.
Malformed JSON, unknown schemas, identity mismatches, unsafe/reparse paths, and
invalid active artifact proofs remain fail-closed.

## Read-only upgrade preflight

Stop other vmcell commands that use the state root, keep the old binary and a
recoverable copy of the state root, then run:

```powershell
vmcell --state-root C:\path\to\state state check
vmcell --json --state-root C:\path\to\state state check
```

`state check` does not create a missing root, acquire a mutation lock, contact a
provider, open registered base-image content, replay guest work, reconcile an
operation, or rewrite JSON. It validates the ordinary state root and all active
installation, image, cell, operation, and operation-bound artifact records. A
compatible report uses contract `vmcell.state-compatibility.v1`, durable format
version `1`, and status `empty` or `compatible`.

An unsupported record schema returns `vmcell.state.upgrade_required` and exit
code 9. Stop: do not run a mutating command with that binary. Keep the state and
base images unchanged, return to the binary that owns the newer schema, or use a
future explicitly documented migrator. Other malformed or unsafe records return
the normal integrity classification and also require investigation before any
mutation. There is no automatic downgrade or repair path.

Tombstoned, partially pruned, or quarantined artifact subtrees are handled by
their existing recovery paths and are not counted as active compatibility
evidence. A compatibility PASS therefore proves that the supported active
records can be read safely; it is not a provider-health, base-image-content, or
real-platform acceptance result.

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
