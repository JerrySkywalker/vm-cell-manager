# ADR 0011: Stable reconciliation and doctor classifications

- Status: accepted
- Milestone: M5

## Decision

Automation consumes provider-neutral classifications, not provider diagnostic
prose. Every cell inspection includes a stable reconciliation classification:

- `code` identifies the observed relationship;
- `ownership` is `proven`, `phase_proven`, `unproven`, `mismatch`, or
  `not_applicable`;
- `required_action` is `none`, `retry_lifecycle`, `recovery_required`, or
  `manual_review`.

The detailed reconciliation payload remains available for provider state and
diagnostic reasons, but those strings are not compatibility keys. The
classification never grants mutation authority; operation-specific ownership
proof remains in the engine/provider boundary.
`phase_proven` means only the narrower durable provisioning proof is satisfied;
it is never reported as full ready-cell ownership.

Guest-operation reconciliation uses the same `required_action` vocabulary.
Transport-active or unknown work requires manual review and is never replayed.

`doctor --json` identifies its contract as `vmcell.doctor.v1`, reports a stable
overall status, and gives each provider a typed probe status. Capability
objects carry the shared automation schema version. Provider `detail` remains
diagnostic prose and must not be parsed.

## Consequences

- Hyper-V and QEMU observations have one automation vocabulary.
- Callers can distinguish proven drift from absent or mismatched ownership.
- Doctor consumers do not infer readiness from English strings.
- New classifications require an additive or versioned compatibility decision.
