# ADR 0015: v0.2 interruption and durable-state compatibility

## Status

Accepted for repository-local v0.2.

## Decision

The v0.1-to-v0.2 durable format remains version 1. Existing schema-1 records
are read in place, additive defaulted fields retain their documented legacy
meaning, and no read performs an implicit migration. `vmcell state check` is a
provider-free, mutation-free compatibility preflight. It reports format 1 only
after active records pass their existing schema, identity, path, and artifact
proofs. An unknown schema becomes the stable
`vmcell.state.upgrade_required` integrity result; no automatic downgrade,
rewrite, or best-effort reinterpretation is allowed.

Windows Ctrl-C for `vmcell run` is cooperative. The console handler records an
interruption request; the orchestration observer samples it at bounded durable
stage boundaries. Before guest transport, exact-owned cleanup may proceed. Once
guest transport is active, uncertainty remains a nonterminal operation and
automatic cleanup is refused. A command already known to have completed keeps
its durable result and follows the requested cleanup policy. No guest action is
automatically replayed.

## Consequences

- v0.1 state requires no migration to be read by v0.2, but operators have an
  explicit read-only preflight before using a new binary.
- Compatibility PASS is not provider, image-content, or real-platform
  acceptance.
- Readiness and guest action cancellation is bounded/cooperative rather than an
  unsafe asynchronous thread abort. A Ctrl-C during one of those actions may be
  visible only at the next boundary; timeout or unknown transport effects stay
  retained and nonreplayed.
- Future incompatible schemas require a separately designed, bounded migrator
  or the binary version that owns them.
