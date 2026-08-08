# Provider Model

Providers translate portable cell intent into local hypervisor operations. The provider boundary is local by design: a provider manages VM resources on the current host and does not perform multi-host placement.

## v1 provider set

### Hyper-V

Primary Windows-native provider.

Expected responsibilities:

- detect Hyper-V availability without enabling it automatically;
- create a VM from a registered VHDX image variant;
- create a single differencing VHDX per disposable cell;
- configure bounded CPU/memory settings;
- start, stop, inspect, and destroy only owned cells;
- expose provider-specific capability facts to the core.

PowerShell Direct belongs to the guest transport layer, not the Hyper-V lifecycle provider.

### QEMU

Portable reference provider.

Expected accelerator mapping:

- Linux: KVM;
- macOS: HVF;
- Windows: WHPX;
- TCG: explicit emulation only, never a silent fallback for an accelerated request.

Expected responsibilities:

- QMP-based lifecycle/control;
- QCOW2 backing/overlay images;
- explicit accelerator capability discovery;
- process ownership and cleanup;
- local socket/process state required for the cell lifecycle.

QGA belongs to the guest transport layer.

## Capability discovery

A provider should expose observed capabilities rather than a marketing-level feature list. The bootstrap `doctor` probe is intentionally conservative.

Candidate capability dimensions:

```text
host_os
host_arch
guest_os
guest_arch
full_system_vm
hardware_acceleration
accelerators
cow_overlay
guest_transports
networkless_guest_exec
secure_boot
tpm
nested_virtualization
shared_folder
gpu_or_device_assignment
```

The schema will be versioned before it is treated as a stable external API.

## Provider selection

Selection should follow these rules:

1. An explicitly requested provider wins if it is available and satisfies the request.
2. Otherwise the platform default is preferred.
3. A provider that cannot satisfy a required capability must not be selected merely because it is installed.
4. Cross-architecture software emulation requires explicit opt-in.

Initial defaults:

```text
Windows -> Hyper-V
Linux   -> QEMU/KVM
macOS   -> QEMU/HVF
```

QEMU/WHPX remains useful on Windows for portability and provider-contract testing even though Hyper-V is the preferred Windows-native path.

## Future providers

A provider should be added only when it unlocks a meaningful workload that the existing Hyper-V/QEMU pair cannot serve well.

Possible future candidates include Apple Virtualization.framework, libvirt, or VirtualBox. They are not roadmap commitments.

OpenStack is not a local provider and is deliberately outside this interface. See `openstack-boundary.md`.
