# CLI and JSON Contract

M1 CLI output is pre-stable but explicitly versioned. `--json` is global and causes successful commands to write JSON to stdout. Errors are written to stderr and return exit code 1.

## Commands

```text
vmcell doctor
vmcell provider list
vmcell image add --id IMAGE --path BASE.vhdx --guest-os windows
vmcell image list
vmcell image inspect IMAGE
vmcell create --image IMAGE [--cpu-count N] [--memory-mib N]
vmcell list
vmcell inspect CELL_ID
vmcell start CELL_ID
vmcell stop CELL_ID
vmcell destroy CELL_ID
vmcell reconcile [CELL_ID]
```

`--state-root PATH` is also global. It is intended for isolated development and acceptance roots; changing it does not authorize adoption of provider objects.

## Successful output

Single-object responses contain `schema_version: 1`. List responses use:

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

Inspection includes the durable cell record, provider-observed VM when present, and one reconciliation classification. Reconciliation itself is read-only.

## Error output

```json
{
  "schema_version": 1,
  "error": {
    "category": "operation_failed",
    "message": "ownership is not proven: ownership marker mismatch"
  }
}
```

M5 may refine categories and schema stability, but it must preserve the fail-closed ownership semantics introduced in M1.
