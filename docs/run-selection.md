# Provider-Neutral Run Selection

`vmcell run --image IMAGE -- COMMAND...` resolves one descriptive execution
plan before reading guest credentials or beginning lifecycle mutation. A
provider flag is not required when exactly one deterministic compatible local
path survives the declared image variants, host probes, accelerator evidence,
guest identity, transport requirements, and the
[support matrix](support-matrix.md).

Use `vmcell run --image IMAGE --plan-only` to stop after that read-only
resolution. With `--json`, it emits one `vmcell.run-plan.v1` document. Normal
run success and failure JSON retain the same plan as an additive `plan` field.

## Precedence

Selection applies this order:

1. explicit CLI provider or accelerator;
2. an explicitly configured provider preference;
3. deterministic compatible native/default resolution.

The Windows native order prefers Hyper-V when the image, PowerShell Direct,
and current capability evidence are compatible. Windows QEMU uses WHPX only
when selected and compatible. Linux QEMU uses KVM. WHPX and KVM are
accelerators under the QEMU provider, never providers.

A configured provider is a preference, not authority. If that preferred path
is unavailable or incompatible, planning fails instead of silently choosing a
different provider. Ambiguous paths and contradictory probes also fail.

## TCG boundary

TCG is never implicit, never configuration-authorized, and never a fallback.
It requires both `--accelerator tcg` and `--allow-tcg`. `--accelerator auto`
resolves to the exact native hardware accelerator or fails. After resolution,
the exact accelerator—not `auto`—is bound into the existing cell request.

## Plan contract

The safe human plan line contains the logical image, provider, accelerator,
guest transport, guest OS/architecture, support status, and selection source.
The JSON plan adds schema and contract versions plus `authorizing: false`. It
contains no credentials, command arguments, host paths, hashes, or raw probe
diagnostics.

The plan grants no provider or lifecycle authority. Immediately before the
existing engine begins mutation, it reloads the logical image and re-probes
the selected provider, then revalidates the exact accelerator, architecture,
transport, support row, and plan/spec binding. The engine still performs its
normal mutation-boundary provider probe, immutable-image verification,
ownership checks, and opaque provider/guest authority issuance. Drift fails
before mutation and never falls back to TCG.

`untested` and `development-only` remain visible as exactly those statuses in
plans. Repository tests prove selection behavior but do not promote any real
platform combination to `supported`; the real Windows, WHPX, KVM, QGA, and
PowerShell Direct gates remain deferred.
