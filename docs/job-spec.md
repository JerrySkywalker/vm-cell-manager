# Job specification reference

`vmcell.job-spec.v1` is a strict, bounded TOML description of one prepared
disposable execution-cell workload. It is an input contract, not an authority
token: parsing it never acquires a state lock, probes a provider, creates a
cell, opens a guest transport, or changes the host.

## Commands

```text
vmcell job plan --spec vmcell.toml
vmcell run --spec vmcell.toml --plan-only
vmcell run --spec vmcell.toml [--username USER --password-stdin]
```

The first two commands are read-only. A non-plan `run --spec` resolves the same
provider-neutral plan and then uses the existing vmcell lifecycle and guest
authority. Windows PowerShell Direct requires the bounded stdin credential
flow. Linux QGA is credentialless and rejects username/password flags.

## Schema v1

```toml
schema_version = 1
image = "linux-dev"
cpu_count = 2
memory_mib = 2048
ttl_seconds = 3600
provider = "qemu"             # optional: hyperv | qemu
accelerator = "kvm"           # optional: whpx | kvm | hvf | tcg
allow_tcg = false
readiness_timeout_seconds = 30
action_timeout_seconds = 60
max_output_bytes = 65536

[command]
program = "/usr/bin/example"
args = ["--bounded"]

[cleanup]
keep = false
keep_on_failure = true

[[copy_in]]
source = "inputs/request.json"
destination = "inputs/request.json"
overwrite = "deny"            # optional: deny | replace
timeout_seconds = 60
max_bytes = 1048576

[artifacts]
sources = ["results/output.json"]
timeout_seconds = 60
max_bytes_per_file = 1048576
```

Unknown fields, unsupported schema versions, invalid resource/timeout bounds,
unsafe guest paths, invalid provider/accelerator combinations, and authority-
like or credential fields fail deterministically before lifecycle authority.
The source document itself is capped at 64 KiB. The exact accepted source bytes
are SHA-256 bound to planning and result metadata; the document is not persisted
in state or copied into result/error envelopes.

The specification describes execution-cell configuration only: image,
selection preference, bounded resources, command/arguments, timeouts,
copy-in, artifact requests, and cleanup policy. It does not describe package
installation, provisioning, cloud resources, networking, multi-machine
topology, scheduling, retries, an agent framework, or application automation.

## Input and artifact boundaries

Copy-in sources are lexically relative to the job file. Immediately before
guest transport, vmcell rechecks the ordinary, no-reparse source path, identity,
size, and SHA-256 binding; replacement or content drift fails before guest work.
Copy and artifact bounds are enforced by the existing guest-operation and
artifact subsystems. Artifact collection produces operation-bound manifests
beneath the configured state root; it does not publish files or contact a
provider during later inspection.

## Plans, results, and repeatability

`job plan` and `run --spec --plan-only` emit `vmcell.job-plan.v1` with
`authorizing: false`. The plan reports only safe resolved identity, resources,
timeouts, cleanup, declared action counts, and the source SHA-256. It omits
commands, paths, credentials, raw provider evidence, and state-root details.

Each admitted non-plan lifecycle run receives a fresh job ID and creates a fresh
cell, even when the same source SHA-256 is used again or a prior cell was
retained. The result uses `vmcell.job-result.v1`; known completed job actions
use `vmcell.job-operations.v1`. These expose safe IDs, timing, counts, and byte
totals only. They do not promise identical guest output, artifact bytes,
timestamps, or provider observations across runs.

An unknown or transport-active guest effect remains subject to the existing
no-replay recovery contract. A matching job ID or source digest never authorizes
adoption, cleanup, reuse, retry, or replay. Use `status`, `operation list`,
`operation inspect`, and `operation reconcile` as the explicit recovery
surfaces.

## Support boundary

Repository-local planning, tests, and package validation do not promote a
provider, accelerator, or guest transport to supported or experimental status.
Real Hyper-V/PowerShell Direct, WHPX/QGA, KVM/QGA, and release acceptance remain
separate host and evidence gates.
