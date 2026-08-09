# CLI and JSON Contract

M2 CLI output is pre-stable but explicitly versioned. `--json` is global and causes successful commands to write JSON to stdout. Errors are written to stderr and return exit code 1.

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
vmcell operation list [CELL_ID]
vmcell operation inspect OPERATION_ID
vmcell operation reconcile OPERATION_ID
vmcell gc
```

`--state-root PATH` is also global. It is intended for isolated development and acceptance roots; changing it does not authorize adoption of provider objects.

Guest commands require an exact-owned, running Windows cell. The password is
read as one bounded line from stdin; there is deliberately no password argument
or environment-variable option. Guest paths are relative to vmcell's fixed
workspace and reject traversal, absolute paths, alternate data streams, device
names, and ambiguous trailing dot/space forms.

For QEMU, the image and create provider must be explicit and compatible. TCG
requires both `--accelerator tcg` and `--allow-tcg`; `auto` never silently
falls back unless `--allow-tcg` is present. Linux QGA guest commands are
credentialless, reject username/password flags, and use a POSIX CellId-scoped
workspace. Windows QGA is not advertised by M3.

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

Copy-out and artifact collection write only beneath the deterministic state
artifact root. Each committed manifest binds CellId, operation ID, guest path,
state-relative host path, SHA-256, and size. `artifact inspect` reads that
manifest; it does not contact the guest. One collection is capped at 16 files,
64 MiB per file, and a 1 GiB declared aggregate maximum.

`vmcell gc` is an explicit foreground operation. It evaluates durable TTLs at
one timestamp, skips cells with nonterminal guest operations, and delegates
eligible cells to the M1 exact-ownership destroy path. It is not a daemon or
background timer.

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

M5 may refine categories and schema stability, but it must preserve the
fail-closed ownership semantics introduced in M1 and the non-secret,
nonreplayable guest-operation semantics introduced in M2.
