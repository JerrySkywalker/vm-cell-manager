# ADR 0013: Automation CLI and provider probe invariants

- Status: accepted
- Milestone: M5

## Decision

Automation-facing argument failures honor global `--json` even when full CLI
parsing cannot complete. They return the schema-v1 error envelope, stable
`vmcell.invalid_input` code, and exit `2` without echoing raw arguments. Help
and version remain human-readable. The removed pre-alpha `provider-list`
spelling is a deterministic error with an explicit migration to
`vmcell provider list`; there is no hidden compatibility alias.

Provider readiness is normalized once at the shared provider boundary before
doctor, provider-list, or lifecycle admission consumes it. `available` is a
derived compatibility field. A ready provider must also report the current
capability schema plus full-system VM and copy-on-write overlay support.
Contradictory ready responses become `probe_failed`; non-ready responses have
their capabilities cleared. Diagnostic prose never becomes an automation key.

The final M5 contract suite uses deterministic cross-products instead of
host-specific provider mutation: Hyper-V and QEMU names, every typed probe
status, contradictory compatibility booleans, invalid capability versions,
CLI parse faults, and provider/state/guest error categories. Real provider and
guest acceptance remains a separate platform gate.

## Consequences

- Provider list, doctor, and lifecycle admission cannot disagree about
  readiness.
- Invalid automation invocations remain parseable without exposing secrets or
  argument contents.
- Removed pre-alpha spellings fail loudly instead of creating an indefinite
  compatibility surface.
- Repository-local contract tests do not imply real Hyper-V, PowerShell Direct,
  QEMU, KVM, WHPX, HVF, or macOS acceptance.
