# ADR 0006: M2 artifacts, TTL, and explicit garbage collection

- Status: Accepted
- Date: 2026-08-09

## Context

Guest execution needs durable recovery evidence and deterministic host artifact
storage. TTL cleanup must remain daemonless and must not weaken M1 destroy
authority.

## Decision

Every guest action writes a versioned, non-secret operation intent before a
transport session can begin. Records contain only operation identity, kind,
phase, timestamps, bounded result metadata, and a safe failure classification.
They never contain credentials, command arguments, output contents, or raw
transport errors.

Copy-out and artifact collection commit only beneath:

```text
state/artifacts/<CellId>/<operation-id>/
```

The state store pins ordinary artifact ancestors, rejects reparse points and
path traversal, writes operation-owned staging files, verifies size and
SHA-256, and commits files/manifests with same-directory atomic replacement.
Copy-out never writes an arbitrary caller-selected host destination. Copy-in
reads an ordinary non-reparse host file through a bounded Rust handle before
transport, so PowerShell never reopens an arbitrary host source path.

Guest paths are relative to a fixed CellId-scoped guest workspace. Rust rejects
absolute paths, traversal, device names, alternate data streams, trailing
dots/spaces, and ambiguous separators. The PowerShell guest shim independently
checks the resolved root and every existing ancestor for reparse points before
and after commit. An administrator-equivalent process inside the disposable
guest can still race its own filesystem and is not treated as a host security
boundary; observed ambiguity remains nonreplayable. Copy-in uses a sibling
temporary file and an explicit `deny` or `replace`
policy; interruption cannot expose a partially written destination.

`CellSpec.ttl_seconds` is bounded durable intent and `expires_at` is written in
the original M1 creation manifest. There is no timer or daemon. `vmcell gc`
uses an injected UTC cutoff, serializes against lifecycle and guest operations,
and routes each eligible cell through the existing exact-owned M1 destroy
authority. It never destroys by name, never adopts drift, and never treats an
in-flight or unknown guest operation as safe cleanup authority.

## Consequences

- A crash before operation intent has no authorized guest side effect.
- A crash after a session/action starts leaves a nonterminal record that is
  read-only detectable and never automatically replayed.
- Artifact staging can be cleaned only when its CellId/operation ownership and
  physical containment are proven.
- GC remains explicit and idempotent per cell; ambiguity fails closed and is
  reported without foreign mutation.
- M1 destroy crash recovery remains the rollback mechanism for expired cells.
