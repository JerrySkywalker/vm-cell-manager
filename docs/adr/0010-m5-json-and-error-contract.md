# ADR 0010: Versioned JSON and deterministic error taxonomy

- Status: accepted
- Milestone: M5

## Decision

Machine-readable CLI responses use explicitly versioned schema families. The
current family is `schema_version: 1`. A version retains field meaning and
types; compatible releases may add fields, while removal, type changes, or
semantic reuse require a new schema version.

Runtime failures have one deterministic classification containing:

- a stable dotted `code`;
- a stable `category`;
- a redacted human `message`;
- a `retryable` hint;
- the exact process `exit_code`.

Human and JSON modes return the same taxonomy exit code. Unknown internal
errors fail closed as `vmcell.internal` with exit code 10. Timeout, partial-copy,
ownership, and integrity errors are never labelled retryable merely because an
automation client could issue the command again; their side effects or
authority must be reconciled first.

Clap syntax errors retain exit code 2. A later pre-alpha CLI migration may add
structured parse-error output, but it must not change the runtime taxonomy
without a versioned compatibility decision.

## Consequences

- Automation can branch on code/category/exit status without parsing prose.
- Existing success response shapes remain unchanged under schema version 1.
- Error messages remain diagnostic text, not a compatibility key.
- Provider-specific errors are mapped into provider-neutral automation classes.
