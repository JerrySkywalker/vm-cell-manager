# M1 Hyper-V Acceptance Gate

Implementation does not authorize real Hyper-V mutation. Real acceptance requires a separately approved, dedicated Hyper-V-capable host and must not run on the existing GitHub Actions runner labeled `core` and `trusted`.

## Admission

- exact feature-branch SHA and clean checkout;
- dedicated integration identity and isolated state/runtime root;
- disposable ordinary non-reparse base VHDX with recorded SHA-256;
- no VM or switch name collision;
- inventory of foreign VMs and switches captured read-only;
- explicit operator authorization for this exact run.

## Required sequence

1. register and inspect the immutable image;
2. create one stopped networkless cell;
3. verify one differencing VHDX and the exact parent;
4. start, inspect, stop, and reconcile exact ownership;
5. destroy the owned cell twice to prove idempotency;
6. verify the VM id and CellId-scoped runtime are absent;
7. verify the base hash, foreign VM inventory, host features, and switch inventory are unchanged.

Any ownership mismatch, unexpected provider object, image drift, extra disk, network adapter, or state ambiguity is a terminal fail-closed result. The acceptance process must not adopt, repair, or delete ambiguous provider state.
