# CLI and JSON Contract

CLI output is pre-alpha but explicitly versioned. `--json` is global and causes successful commands to write JSON to stdout. Errors are written to stderr and use the deterministic exit-code taxonomy below.

Schema version 1 keeps existing field names, types, and meanings stable.
Compatible releases may add fields. Removing fields, changing their types, or
reusing their meaning requires a new schema version. Clients must ignore
unknown additive fields.

When `--json` is present, argument-validation failures also use the versioned
error envelope and exit `2`; raw arguments and parser diagnostics are not
echoed. Help and version output remain human-readable discovery surfaces.

## Commands

```text
vmcell doctor
vmcell status
vmcell state check
vmcell completion powershell
vmcell provider list
vmcell image add --id IMAGE --path BASE.vhdx --guest-os windows [--provider hyperv]
vmcell image add --id IMAGE --path BASE.qcow2 --guest-os linux --provider qemu
vmcell image validate --path BASE.vhdx --guest-os windows [--provider hyperv]
vmcell image validate --id IMAGE [--provider hyperv|qemu]
vmcell image list
vmcell image inspect IMAGE
vmcell image dependencies IMAGE
vmcell image unregister IMAGE
vmcell job plan --spec PATH
vmcell create --image IMAGE [--provider hyperv|qemu] [--cpu-count N] [--memory-mib N] [--ttl-seconds N]
              [--accelerator auto|whpx|kvm|hvf|tcg] [--allow-tcg]
vmcell run --image IMAGE [--provider hyperv|qemu] [--accelerator auto|whpx|kvm|hvf|tcg]
           [--allow-tcg] [--plan-only | -- PROGRAM [ARG...]]
vmcell run --spec PATH --plan-only
vmcell run --spec PATH [--username USER --password-stdin]
vmcell list
vmcell inspect CELL_ID
vmcell start CELL_ID
vmcell stop CELL_ID
vmcell destroy CELL_ID
vmcell reconcile [CELL_ID]
vmcell exec CELL_ID --username USER --password-stdin -- PROGRAM [ARG...]
vmcell shell CELL_ID --username USER --password-stdin
vmcell copy-in CELL_ID --source HOST_FILE --destination GUEST_PATH --username USER --password-stdin
vmcell copy-out CELL_ID --source GUEST_PATH --username USER --password-stdin
vmcell artifact collect CELL_ID --path GUEST_PATH [--path GUEST_PATH...] --username USER --password-stdin
vmcell artifact inspect CELL_ID OPERATION_ID
vmcell artifact prune [--older-than-seconds N] [--max-artifacts N] [--dry-run]
vmcell operation list [CELL_ID]
vmcell operation inspect OPERATION_ID
vmcell operation reconcile OPERATION_ID
vmcell gc
```

`--config PATH`, `--state-root PATH`, `--lock-timeout-ms N`, and
`--human-output normal|quiet` are global. The lock timeout is
bounded to 30 seconds per state-lock acquisition, defaults to fail-fast, and
never authorizes lock stealing. Commands that dispatch serially to multiple
provider engines may consume one bounded interval per acquisition.
Changing the state root does not authorize adoption of provider objects.

`state check` is read-only and provider-free. It emits
`vmcell.state-compatibility.v1` with schema version 1, one `checked_at`, durable
format version `1` for legacy/direct-only state or `2` when readable v0.4
job-correlated records are present, status `empty|compatible`, and counts for
active installation, image, cell, guest-operation, and operation-bound artifact
records. It does not create a missing root or rewrite compatible v0.1 JSON.
Unsupported durable schemas use `vmcell.state.upgrade_required`, integrity exit
9, and require the operator to stop mutation and follow
[`state-compatibility.md`](state-compatibility.md).

`completion powershell` is human-only shell integration. It is generated from
the exact binary's Clap command graph before configuration, state, or provider
access. Repeated runs of the same binary are byte-identical. `--json` is
rejected as invalid input because the output is executable PowerShell text,
not a JSON response contract.

Config selection and precedence are defined in
[`user-configuration.md`](user-configuration.md). Malformed/unsafe config uses
`vmcell.config.invalid` and exit `2`; a missing explicit config uses
`vmcell.config.not_found` and exit `2`; an unsupported config schema uses
`vmcell.config.unsupported_schema` and integrity exit `9`. Error envelopes do
not include config contents or paths. Configuration is loaded before state or
provider access and cannot contain credentials, accelerator/TCG policy, or
authority exceptions.

`image validate` is read-only. Candidate-path mode returns a schema-versioned
validation report without registering the image. Registered-image mode repeats
the provider metadata and immutable-content proof and compares the canonical
path, format, file size, and SHA-256 with the durable record. `status` is
`usable` only when `issues` is empty; an unusable report is still emitted and
the command exits with the integrity code `9`. Issue values are stable
snake-case identifiers. Human `image inspect` performs the registered proof;
JSON `image inspect` keeps the existing `ImageRecord` response unchanged, and
automation can request the proof explicitly with `image validate --id`.

`image dependencies` is provider-neutral and read-only. It returns contract
`vmcell.image-dependencies.v1`, the image ID, a deterministic CellId-sorted
array of durable references (`cell_id`, `state`, `phase`, `blocking`), and
`can_unregister`. A reference is nonblocking only when both durable cell state
and phase are `destroyed`; any inconsistent cell/image binding is an integrity
failure.
The dependency report applies the same metadata-removal predicate as
`unregister`: malformed, reparse, device-ambiguous, or same-file-alias variant
metadata fails integrity and never yields `can_unregister: true`.

`image unregister` rechecks that dependency set under the global mutation lock
and atomically retires only the exact validated `images/IMAGE.json` entry to a
non-JSON receipt in the same pinned directory. The active manifest name is
removed from image lookup, but its bytes are not unlinked. Its
`vmcell.image-unregister.v1` report contains `metadata_removed`, the invariant
`bytes_deleted: false`, and any retained destroyed-cell references. The command
does not require provider availability and never probes a provider or accesses
registered base-image contents. It performs only a bounded read-only/no-follow
file-identity check for existing variant paths; a missing variant does not
block metadata removal. A non-destroyed dependency fails as
`vmcell.image.in_use` / conflict exit `4`. Repeating a completed unregister is
successful with `metadata_removed: false`.

PowerShell Direct guest commands require an exact-owned, running Windows cell.
Its password is read as one bounded line from stdin; there is deliberately no
password argument or environment-variable option. Guest paths are relative to
vmcell's fixed workspace and reject traversal, absolute paths, alternate data
streams, device names, and ambiguous trailing dot/space forms.

`vmcell shell` is human-only and rejects `--json`. It reads the bounded password
from stdin first and then reads commands from the attached Windows console, so
secret and command streams are not multiplexed. Each nonempty line is one
independent bounded `powershell.exe -Command` operation with a fresh provider
and installation authority proof. It is not a PTY and offers no guest stdin,
`Read-Host`, full-screen controls, or persistent cwd/environment/process state.
`.exit`, EOF, or cooperative Ctrl-C leaves the cell running. Any timeout,
transport/session failure, ownership drift, or existing nonterminal guest
operation stops the console without replay or automatic cleanup; the durable
operation ID is printed when transport intent was recorded.

`run` resolves a read-only `vmcell.run-plan.v1` before credentials or mutation.
Its precedence is explicit CLI provider/accelerator, then an explicitly present
non-authorizing config provider preference, then deterministic compatible
native/default resolution. The plan records provider, exact accelerator,
transport, guest identity, support status, selection source, and
`authorizing: false`; it omits paths, hashes, commands, credentials, and raw
probe detail. `run --plan-only` returns that plan directly. Actual run success
and failure objects carry it as an additive `plan` field when resolution
completed, preserving the one-JSON-document rule.

`job plan --spec PATH` loads the strict TOML `vmcell.job-spec.v1` document and
returns a non-authorizing `vmcell.job-plan.v1`. `run --spec PATH --plan-only`
returns the same job-plan contract. Both paths resolve current read-only image
and provider evidence but do not acquire a mutation lock, create state, issue
provider authority, read credentials, or contact a guest. A spec is
self-contained for provider selection: it never inherits a mutable configured
provider preference.

For QEMU, the selected image variant and accelerator must be compatible. TCG
requires both `--accelerator tcg` and `--allow-tcg`; `auto` never falls back to
TCG, even when `--allow-tcg` is present. Linux QGA guest commands are
credentialless, reject username/password flags, and use a POSIX CellId-scoped
workspace. Windows QGA is not advertised.

## Successful output

Automation reports, list envelopes, and error envelopes use their documented
`schema_version: 1` contracts. Durable records serialized directly have their
own schema: legacy/direct cell, guest-operation, and artifact records are
version `1`, while v0.4 job-correlated records are version `2`. Thus `operation
inspect --json` and `artifact inspect --json` may return a v2 durable record;
`operation list --json` and `status --json` retain their v1 outer envelope but
may contain v2 records. List responses use:

`doctor --json` additionally reports `contract: "vmcell.doctor.v1"`, overall
`status` (`ready` or `unavailable`), and typed provider probe status (`ready`,
`unsupported_host`, `unavailable`, or `probe_failed`). Each capability object
carries its own `schema_version`. Provider `detail` is diagnostic prose and
must not be parsed. `provider list --json` uses the same typed provider objects.
`ready` means the bounded provider/capability probe completed; callers must
still inspect the requested accelerator, guest, and transport capabilities.
The shared provider boundary derives `available` from typed `status` and the
versioned full-system/COW lifecycle capability minimum. Contradictory ready
responses fail closed as `probe_failed`, and non-ready responses expose no
positive capabilities.

`status --json` returns `contract: "vmcell.status.v1"`, one `evaluated_at`
instant, and deterministic provider, cell, image, and guest-operation arrays.
It is read-only and provider tolerant: unavailable providers preserve durable
local records while their observations become `provider_unavailable`; they are
never reported as exact-owned or safe to mutate. Cell entries derive retention
(`manual`, `active_until_expiry`, `expired`, or `none`), pending and uncertain
operation counts, and non-authorizing cleanup guidance. A
`transport_active` guest operation is uncertain and requires `manual_review`;
status never replays or reconciles it. Image validation is attempted only when
the recorded provider probe is ready, otherwise the durable image remains
visible with typed provider status.

Cells created by `run --spec` expose an optional
`cells[].cell.job = { job_id, job_spec_sha256, started_at }`; job-dispatched
guest operations and artifact records expose an optional `job_id`. `operation
inspect --json` returns that same optional operation field. Human `status` and
`operation` output render `job=<UUID|none>`. `none` means no durable correlation
is known (including direct work on a retained job cell or legacy state); it
never means that replay, cleanup, or ownership is safe.

```json
{
  "schema_version": 1,
  "items": []
}
```

Lifecycle operations return:

```json
{
  "schema_version": 1,
  "cell_id": "00000000-0000-0000-0000-000000000000",
  "state": "stopped",
  "changed": true
}
```

Inspection includes the durable cell record, provider-observed VM when present,
the detailed reconciliation payload, and a provider-neutral `classification`:

```json
{
  "code": "state_drift",
  "ownership": "proven",
  "required_action": "retry_lifecycle"
}
```

Stable codes are `exact_owned`, `manifest_only`, `provider_missing`,
`unproven_provider_object`, `ownership_mismatch`, `state_drift`, `provisioning`,
and `destroyed`. Ownership is `proven`, `phase_proven`, `unproven`, `mismatch`,
or `not_applicable`; `phase_proven` is the narrower durable provisioning proof.
Required action is `none`, `retry_lifecycle`, `recovery_required`, or
`manual_review`. This classification is descriptive and never authorizes
mutation. Reconciliation may persist only the safe state transitions described
below.

Guest exec returns a generated `operation_id`, exact `cell_id`, exit code,
UTF-8 stdout/stderr, byte counts, encoding, and truncation status. Output is
bounded by `--max-output-bytes`. The durable operation record intentionally
does not contain credentials, command arguments, output, or raw transport
errors. A timeout, partial copy, invalid response, or unknown transport failure
remains nonterminal and is never replayed automatically.

`operation reconcile` is also nonreplaying. It marks an intent interrupted only
when transport was never recorded as active, completes an `artifact_committed`
record only after revalidating the bound manifest and every file hash/size, and
reports `recovery_required` without mutation for transport-active operations.
Incomplete artifact staging without a committed manifest remains quarantined;
M2 provides no broad or automatic staging deletion.
The report uses the same `required_action` vocabulary; transport-active work is
`manual_review`.

Copy-out and artifact collection write only beneath the deterministic state
artifact root. Each committed manifest binds CellId, operation ID, guest path,
state-relative host path, SHA-256, and size. `artifact inspect` reads that
manifest; it does not contact the guest. One collection is capped at 16 files,
64 MiB per file, and a 1 GiB declared aggregate maximum.

`artifact prune` is the only M5 retention mutation. It selects completed
artifacts at one age cutoff, processes at most 256 records, and supports
`--dry-run`. Dry run does not alter operation or artifact records, although it
may initialize lock infrastructure when the state root is new. The engine saves
the additive `artifact_pruned_at` tombstone on the completed operation before
removing the exact CellId/operation artifact subtree. A crash after that save is
resumed by a later prune, including from a partially removed exact subtree;
guest work is never replayed. Missing or integrity-drifted committed artifacts
fail closed before the tombstone is written. There is no background retention
daemon.

`vmcell gc` is an explicit foreground operation. It evaluates durable TTLs at
one timestamp, skips cells with nonterminal guest operations, and delegates
eligible cells to the M1 exact-ownership destroy path. It is not a daemon or
background timer.

## Run workflow output

`vmcell run --image IMAGE [options] -- PROGRAM [ARG...]` composes the existing
create, start, guest-exec, and exact-owned destroy operations. `vmcell run
--spec PATH` resolves the same lifecycle authority from a validated,
self-contained job specification. In default human mode, lifecycle progress
and the final cleanup disposition are written to stderr. Bounded guest stdout
is written to stdout and bounded guest stderr is written to stderr. Progress
records contain only image/cell identifiers, stages, exit status, and cleanup
classification; they never echo credentials, the guest command, host paths, or
raw provider diagnostics.

With `--json`, progress is suppressed and success emits one schema-v1
`RunCellReport` to stdout. It contains `cell_id`, `operation_id`, `outcome`, the
bounded `result`, and `cleanup`. A run failure uses the normal error body plus a
`run` object containing the safe stage, cell/operation identifiers, durable
error codes, cleanup disposition, and optional result metadata. Guest stdout
and stderr content are never included in an error envelope.

Direct-run reports leave `job` and `job_operations` absent rather than encoding
`null`. A spec-backed success adds `job` with contract
`vmcell.job-result.v1` (fresh job ID, source SHA-256, and timing) and
`job_operations` with contract `vmcell.job-operations.v1`. The latter contains
only copy operation IDs and byte counts, an optional command operation ID, and
artifact operation IDs, file counts, and byte totals. Failures after a
`JobRunRequest` exists, including bounded interruption-handler setup, carry the
same safe `job` data under `run`; `job_operations` is absent before the manifest
is initialized, present but empty after that initialization before an action
finishes, and populated only with known completed actions. Invalid or unreadable specification
input and pre-request preparation failures such as a missing or invalid bound
copy source use the generic redacted error envelope even when a read-only plan
was resolved. None of these job fields contains
specification contents, command text, credentials, host or guest paths, raw
provider diagnostics, or guest stream contents. The compact terminal human job
result has counts and safe IDs; use `status` and `operation list|inspect` for
durable recovery correlation.

After a completed guest command, `vmcell run` returns the guest exit code when
it is in `1..=255`; an out-of-range nonzero guest code maps to `1`. This
run-specific success-path rule can numerically overlap the vmcell error table,
so automation that needs to distinguish guest failure from orchestration
failure must use `--json` and inspect `outcome` or `error`. Orchestration
failures always use the stable taxonomy below.

On Windows, Ctrl-C during `run` is sampled cooperatively at durable stage
boundaries. A bounded readiness probe or guest action is allowed to return its
known result/timeout rather than being asynchronously replayed or abandoned.
Cancellation after transport becomes active remains nonterminal and refuses
automatic cleanup; cancellation after a completed command preserves result and
operation metadata and applies the requested cleanup policy.

## Error output

```json
{
  "schema_version": 1,
  "error": {
    "code": "vmcell.ownership.not_proven",
    "category": "ownership",
    "message": "ownership proof failed",
    "retryable": false,
    "exit_code": 6
  }
}
```

`code`, `category`, and `exit_code` are compatibility fields. `message` is
bounded redacted category prose and must not be parsed. Raw provider stderr,
paths, credentials, arguments, and guest output are never placed in the error
envelope or durable cell failure summary. `retryable` means the same
request may be attempted after its stated precondition changes; it never
authorizes replay of a timeout, partial copy, unknown guest operation, or
ownership failure.

| Exit | Category | Meaning |
| ---: | --- | --- |
| 0 | success | Command completed successfully. |
| 2 | invalid_input | CLI or request input is invalid. |
| 3 | not_found | Required local/provider object is absent. |
| 4 | conflict | Existing state or lifecycle state conflicts. |
| 5 | unavailable | Provider or guest transport is unavailable. |
| 6 | ownership | Ownership is absent, changed, or drifted. |
| 7 | contention | Another local mutation owns the lock; bounded wait expired. |
| 8 | timeout | Deadline expired; side effects may be unknown. |
| 9 | integrity | Schema, identity, response, or artifact proof failed. |
| 10 | internal | Unclassified internal or state I/O failure. |
| 11 | recovery_required | Partial/unknown work requires reconciliation. |
| 12 | resource_limit | A configured size/output bound was reached. |
| 13 | authentication | Guest authentication failed. |
| 14 | unsupported | Provider, transport, or operation is unsupported. |

## Pre-alpha command migration

The former top-level `provider-list` spelling is removed. Automation must use
`vmcell provider list`. The legacy spelling fails deterministically as
`vmcell.invalid_input` / exit `2`; under `--json` it receives the same redacted
schema-v1 error envelope as every other argument-validation failure. This is a
pre-alpha migration, not a compatibility alias, so scripts cannot silently
continue on an obsolete surface.
