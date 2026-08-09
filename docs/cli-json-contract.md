# CLI and JSON Contract

CLI output is pre-alpha but explicitly versioned. `--json` is global and causes successful commands to write JSON to stdout. Errors are written to stderr and use the deterministic exit-code taxonomy below.

Schema version 1 keeps existing field names, types, and meanings stable.
Compatible releases may add fields. Removing fields, changing their types, or
reusing their meaning requires a new schema version. Clients must ignore
unknown additive fields.

## Commands

```text
vmcell doctor
vmcell provider list
vmcell image add --id IMAGE --path BASE.vhdx --guest-os windows [--provider hyperv]
vmcell image add --id IMAGE --path BASE.qcow2 --guest-os linux --provider qemu
vmcell image list
vmcell image inspect IMAGE
vmcell create --image IMAGE [--provider hyperv|qemu] [--cpu-count N] [--memory-mib N] [--ttl-seconds N]
              [--accelerator auto|whpx|kvm|hvf|tcg] [--allow-tcg]
vmcell list
vmcell inspect CELL_ID
vmcell start CELL_ID
vmcell stop CELL_ID
vmcell destroy CELL_ID
vmcell reconcile [CELL_ID]
vmcell exec CELL_ID --username USER --password-stdin -- PROGRAM [ARG...]
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

`--state-root PATH` and `--lock-timeout-ms N` are global. The lock timeout is
bounded to 30 seconds per state-lock acquisition, defaults to fail-fast, and
never authorizes lock stealing. Commands that dispatch serially to multiple
provider engines may consume one bounded interval per acquisition.
Changing the state root does not authorize adoption of provider objects.

PowerShell Direct guest commands require an exact-owned, running Windows cell.
Its password is read as one bounded line from stdin; there is deliberately no
password argument or environment-variable option. Guest paths are relative to
vmcell's fixed workspace and reject traversal, absolute paths, alternate data
streams, device names, and ambiguous trailing dot/space forms.

For QEMU, the image and create provider must be explicit and compatible. TCG
requires both `--accelerator tcg` and `--allow-tcg`; `auto` never silently
falls back unless `--allow-tcg` is present. Linux QGA guest commands are
credentialless, reject username/password flags, and use a POSIX CellId-scoped
workspace. Windows QGA is not advertised by M3.

## Successful output

Single-object responses contain `schema_version: 1`. List responses use:

`doctor --json` additionally reports `contract: "vmcell.doctor.v1"`, overall
`status` (`ready` or `unavailable`), and typed provider probe status (`ready`,
`unsupported_host`, `unavailable`, or `probe_failed`). Each capability object
carries its own `schema_version`. Provider `detail` is diagnostic prose and
must not be parsed. `provider list --json` uses the same typed provider objects.
`ready` means the bounded provider/capability probe completed; callers must
still inspect the requested accelerator, guest, and transport capabilities.

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
mutation. Reconciliation itself is read-only.

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
