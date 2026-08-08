# M2 Windows Guest Control Threat Model

## Protected assets

- M1 installation, cell, provider, runtime, and immutable-image authority;
- guest credentials and authentication material;
- host files outside the configured state/artifact root;
- guest files outside the CellId-scoped workspace;
- command output and collected artifacts;
- foreign Hyper-V VMs, switches, features, and services.

## Trust boundaries

```text
CLI/library caller
  -> Rust CellEngine and StateStore
  -> opaque GuestActionAuthority
  -> bounded PowerShell Direct host shim
  -> exact-owned running Windows guest
```

The caller controls command text, relative guest paths, copy-in source files,
timeouts, and credentials. The guest may be unready, compromised, or actively
changing files. Hyper-V and the host filesystem may drift between observations.
Only Rust state plus exact provider identity can authorize an action.

## Threats and controls

| Threat | Required control |
| --- | --- |
| Foreign/name-reused VM | Current installation plus provider GUID, marker, configuration, disk, network, CPU, memory, and running-state proof; never mutate by name. |
| Provider drift after Rust proof | Shared provider mutex and a fresh complete PowerShell snapshot immediately before session creation. |
| Credential disclosure | Password from stdin only; separate length-framed process-memory channel; non-serializable/zeroing values; categorized errors; no raw stderr persistence. |
| Command injection | Typed program and argument array; Windows command-line quoting; no generated PowerShell interpolation. |
| Unbounded readiness or child process | Rust deadline/poll policy, owned child timeout/kill, bounded output and copy sizes. |
| Host path escape or replacement | Ordinary non-reparse source handle; pinned state/artifact ancestors; operation-scoped staging; atomic commit. |
| Guest traversal, ADS, device, or reparse escape | Strict relative `GuestPath`; fixed CellId root; independent in-guest full-path and reparse checks. |
| Partial or overwritten copy | Explicit overwrite policy and sibling temporary file; atomic destination replacement; interrupted operations never auto-replay. |
| Artifact substitution | Deterministic root, size/SHA-256 manifest, schema/filename identity gates, atomic file and manifest writes. |
| Crash after unknown guest side effect | Durable intent and nonterminal recovery classification; no automatic retry of exec/copy. |
| TTL race or foreign cleanup | Durable expiry cutoff, local serialization, exact-owned M1 destroy only, and fail-closed per-cell GC reporting. |
| Scope expansion through transport | QGA/SSH/QEMU mutation, daemon scheduling, feature/switch/reboot operations, and real provider acceptance remain excluded. |

## Crash classifications

| Window | Classification | Automatic action |
| --- | --- | --- |
| Before operation intent | No guest action authorized | Safe caller retry with fresh proof |
| Intent before session/action | Interrupted, no action confirmed | Fresh explicit retry only after proof |
| Session or exec started | Unknown guest side effects | Never auto-replay |
| Copy-in temporary write | Partial copy staging | Destination remains old/absent; no implicit replay |
| Copy-out before host commit | Partial host staging | Explicit owned-staging cleanup only |
| Artifact file commit before manifest | Orphan/incomplete artifact | Read-only detection; no adoption |
| GC before/during M1 destroy | Existing M1 destroying phase | Exact-owned idempotent destroy retry |
| Installation/provider drift | Ownership failure | No guest or provider verb |

## Repository-local acceptance boundary

M2 tests use mock providers/transports plus static PowerShell checks. They do not
authorize real Hyper-V, VHDX, PowerShell Direct, service, switch, host-feature,
or runner mutation. Real guest acceptance remains a separately admitted
dedicated-host gate layered on the still-pending M1 real-provider acceptance.
