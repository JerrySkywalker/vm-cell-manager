# ADR 0012: Bounded contention, retention, and redacted diagnostics

- Status: accepted
- Milestone: M5

## Decision

State mutation remains serialized by one process-local plus filesystem lock per
canonical state root. The default is fail-fast. Automation may request a
bounded wait with global `--lock-timeout-ms`; the value is capped at 30 seconds
per state-lock acquisition and never steals, replaces, or infers staleness of
another process's lock. A command that dispatches serially to multiple provider
engines may therefore consume one bounded interval per acquisition.
Exhausting the wait returns the stable retryable `vmcell.state.contention`
classification.

Artifact retention is explicit and foreground-only. `artifact prune` selects
completed artifacts at or before one cutoff, supports a dry run that does not
change operation or artifact records (but may initialize the state lock
infrastructure on a new root),
and processes at most 256 records per invocation. No daemon or implicit
age-based cleanup is introduced. A collection remains bounded to 16 files,
64 MiB per file, and 1 GiB total; the same limits are revalidated when a
persisted manifest is loaded.

Before deleting artifact bytes, the engine durably sets the additive
`artifact_pruned_at` tombstone on the bound completed operation. Deletion is
restricted to the exact
CellId/operation subtree under the pinned state artifact root. A crash after the
transition is retryable: a later prune completes the exact same removal without
replaying guest work. Corrupt, missing, reparse, or identity-mismatched committed
artifacts fail closed before a new prune transition.
The small non-secret completed operation record remains as the durable
audit tombstone; M5 does not introduce automatic operation-record deletion.

CLI error prose and persisted cell failure summaries never contain raw provider
stderr, paths, credentials, command arguments, or guest output. Automation uses
the stable error code/category/exit fields; user-facing prose is a bounded
category message. Cell manifests retain only a stable failure code.
Legacy free-form cell failure text is redacted in memory before inspection and
is replaced by the redacted code on the next manifest save.

## Consequences

- Lock contention has deterministic fail-fast and bounded-wait behavior.
- Artifact cleanup is bounded, auditable, dry-runnable, and crash-retryable.
- Artifact limits cannot be bypassed by editing a manifest after collection.
- Detailed provider diagnostics remain transient implementation data, not a
  durable or machine-readable contract.
