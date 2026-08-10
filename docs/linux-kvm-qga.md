# Native Linux QEMU/KVM/QGA Human MVP

This is the canonical native-Linux path for a prepared Linux x86_64 QCOW2
guest with QGA. QEMU is the provider, KVM is its accelerator, and QGA is the
credentialless Linux guest transport. KVM is not a provider, and this path
uses the same run plan, QEMU/QMP lifecycle, ownership model, recovery model,
and artifact surface as the Windows QEMU path.

> **Acceptance gate:** native-Linux compile/test CI, mocks, fake QMP/QGA peers,
> WSL2, and a preflight receipt are repository evidence only. The mutating
> commands below require a separately authorized dedicated native-Linux host.
> Until that real KVM/QGA gate passes, the support-matrix row remains
> `untested`.

VM Cell Manager does not install QEMU, load KVM modules, alter `/dev/kvm`
permissions or groups, repair packages, create host networking, or silently
fall back to TCG.

## 1. Read-only host and state diagnostics

Use an already admitted, effective-user-owned `0700` state root:

```sh
vmcell --state-root /var/tmp/vmcell-acceptance/state state check
vmcell --state-root /var/tmp/vmcell-acceptance/state doctor
vmcell --state-root /var/tmp/vmcell-acceptance/state status
```

The QEMU probe binds canonical executable identities, requires recognizable
bounded `qemu-system-x86_64` and `qemu-img` version output, and keeps KVM out of
the capability set unless QEMU advertises it and the current identity can open
an ordinary `/dev/kvm` character device read/write with stable device/inode
identity. The open issues no ioctl and creates no VM. Missing and
permission-denied KVM are distinct diagnostics; neither authorizes a repair.

## 2. Validate and register a prepared Linux QCOW2

Read-only validation rejects a non-QCOW2 image, an existing backing parent,
path ambiguity, architecture mismatch, or provider-variant mismatch:

```sh
state_root=/var/tmp/vmcell-acceptance/state
base=/var/tmp/vmcell-acceptance/images/linux-qga.qcow2

vmcell --state-root "$state_root" image validate \
  --path "$base" \
  --guest-os linux \
  --guest-arch x86-64 \
  --provider qemu
```

Registration is state mutation and therefore remains acceptance-gated:

```sh
vmcell --state-root "$state_root" image add \
  --id linux-qga \
  --path "$base" \
  --guest-os linux \
  --guest-arch x86-64 \
  --provider qemu
vmcell --state-root "$state_root" image validate --id linux-qga --provider qemu
vmcell --state-root "$state_root" image inspect linux-qga
```

The immutable base is revalidated before use. Each cell gets exactly one
writable QCOW2 overlay with that base as its parent.

## 3. Resolve the provider-neutral plan before mutation

The primary human intent does not name QEMU when one compatible path survives:

```sh
vmcell --state-root "$state_root" run --image linux-qga --plan-only
vmcell --json --state-root "$state_root" run --image linux-qga --plan-only
```

The explicit equivalent is:

```sh
vmcell --state-root "$state_root" run \
  --image linux-qga \
  --provider qemu \
  --accelerator kvm \
  --plan-only
```

The plan must report `provider=qemu`, `accelerator=kvm`, `transport=qga`,
`guest=linux/x86_64`, `support=untested`, and `authority=none`. It is
descriptive and non-authorizing. The engine revalidates image, provider,
accelerator, architecture, and transport evidence before mutation.

TCG is never implicit, never enabled by configuration, and never used after a
KVM failure. Its development-only path requires both `--accelerator tcg` and
`--allow-tcg` on the same CLI invocation.

## 4. Run, exec, copy, and artifact concepts

The following commands mutate a real VM and remain acceptance-gated. Normal
`run` creates, starts, executes, and applies its cleanup policy:

```sh
vmcell --state-root "$state_root" run \
  --image linux-qga \
  --provider qemu \
  --accelerator kvm \
  -- /usr/bin/uname -a
```

Use `--keep` only for an admitted diagnostic window, then use the reported
CellId with the provider-neutral surface:

```sh
cell_id=00000000-0000-0000-0000-000000000000
vmcell --state-root "$state_root" inspect "$cell_id"
vmcell --state-root "$state_root" exec "$cell_id" -- /usr/bin/id
vmcell --state-root "$state_root" copy-in "$cell_id" \
  --source ./input.txt --destination input.txt
vmcell --state-root "$state_root" copy-out "$cell_id" --source output.txt
vmcell --state-root "$state_root" artifact collect "$cell_id" --path output.txt
vmcell --state-root "$state_root" operation list "$cell_id"
```

QGA remains credentialless, but each request still requires an engine-issued
guest authority and a fresh exact-QMP proof of the running networkless VM.
Requests, replies, output, and file transfers are bounded. A timeout or agent
loss after dispatch is an unknown guest effect and is never replayed.

## 5. Inspect, recover, and clean up exact-owned state

```sh
vmcell --state-root "$state_root" status
vmcell --state-root "$state_root" inspect "$cell_id"
vmcell --state-root "$state_root" reconcile "$cell_id"
vmcell --state-root "$state_root" operation list "$cell_id"
vmcell --state-root "$state_root" operation inspect OPERATION_ID
vmcell --state-root "$state_root" operation reconcile OPERATION_ID
vmcell --state-root "$state_root" gc
vmcell --state-root "$state_root" stop "$cell_id"
vmcell --state-root "$state_root" destroy "$cell_id"
```

Unix process mutation requires the durable canonical executable path, hash of
the opened `/proc/<pid>/exe` object, PID, `/proc` start token, launch digest,
exact process instance, and matching QMP definition. QMP/QGA paths are
length-bounded and an off-state launch refuses a pre-existing socket, file, or
symlink as stale/foreign state. PID, path, socket, process name, or QEMU name
alone never authorizes adoption, stop, kill, or deletion. Unix launch makes the
durable leader PID the process-group id, revalidates that binding while the
leader is live, and refuses completion or removal until the exact group is
empty. An owned waiter reaps confirmed child exits. Cleanup also requires both
QMP and QGA endpoint paths to be absent, so stale or foreign entries are
retained for manual review. Every write and deletion stays inside the exact
open-and-revalidated state-root/CellId runtime tree.

## Diagnostics and operator response

| Observation | Stable distinction | Response |
|---|---|---|
| QEMU system executable absent | QEMU probe unavailable | Install nothing automatically; satisfy the external host prerequisite. |
| `qemu-img` absent | QEMU image tool unavailable | Supply an admitted matching toolchain. |
| `/dev/kvm` missing | native accelerator KVM unavailable; device missing | Do not load modules automatically. |
| `/dev/kvm` permission denied | not read-write usable by current identity | Do not change groups, ACLs, owner, or mode automatically. |
| Host/guest architecture differs | `vmcell.run_plan.architecture_mismatch` | Select a matching prepared variant. |
| QCOW2/provider variant differs | incompatible image variant or integrity failure | Register the correct immutable QEMU variant. |
| QMP socket is stale/colliding or overlong | ownership/manual-review or configuration rejection | Do not unlink an unproven endpoint. |
| QMP connect, frame, or command failure | typed provider timeout/invalid-response/command failure | Reconcile; never infer ownership from PID or socket. |
| QGA absent, not ready, malformed, or lost | typed guest not-ready/timeout/transport failure | Retain uncertainty and never replay. |
| Executable, process, QMP, base, or overlay drift | ownership changed | Retain the cell for manual review. |
| Unknown guest effect or cleanup ambiguity | durable retained/manual-review state | Inspect, reconcile, and clean only after exact ownership proof. |

Human and versioned JSON modes share the stable error taxonomy. Run plans and
receipts contain no credentials, command payloads, raw provider diagnostics,
or guest output.

## Non-mutating dedicated-host preflight

[`linux-kvm-preflight.sh`](../tools/linux-kvm-preflight.sh) validates an exact
clean source SHA, native host proof, effective identity, private state root,
canonical QEMU executable hashes/versions, KVM advertisement and read/write
openability with matching pre-open/opened-FD/current-path identity, immutable
base identity, a bounded fingerprint of the actual state-root runtime tree,
the truthful future `<cell-id>/qemu/{qmp.sock,qga.sock}` production pattern,
foreign QEMU prestate, network prestate, and external writer-exclusivity
evidence. QEMU read-only probes have explicit time and output bounds. It opens
`/dev/kvm` without ioctl, starts no QEMU process, creates no overlay, connects
to no QMP/QGA endpoint, changes no host setting, and atomically publishes one
new `0600` receipt as its only persistent output, without replacing an existing
path. Bounded private probe/temp files may exist only during the preflight and
are removed on normal completion or trapped interruption. The receipt parent
must be effective-user-owned `0700` and outside both the repository and vmcell
state root:

```sh
tools/linux-kvm-preflight.sh \
  --repository-root "$PWD" \
  --candidate-sha REQUIRED_EXACT_40_HEX_SHA \
  --state-root "$state_root" \
  --base-image "$base" \
  --qemu-system /usr/bin/qemu-system-x86_64 \
  --qemu-img /usr/bin/qemu-img \
  --owned-namespace vmcell-linux-kvm-acceptance-001 \
  --writer-exclusivity-evidence REQUIRED_EXTERNAL_EVIDENCE_ID \
  --receipt /var/tmp/vmcell-acceptance/receipts/kvm-preflight.json
```

The future authorized transaction must bind that receipt into
[`linux-kvm-acceptance-template.json`](receipts/linux-kvm-acceptance-template.json)
and supply real QMP/QGA lifecycle, pre/post state, crash, nonreplay,
exact-cleanup, rollback, and retained/manual-review evidence. Fixture mode is
always labeled `evidence_source=fixture`, `authorizing=false`, and
`real_platform_acceptance=false`; neither fixture nor hosted CI can promote
the support row.
