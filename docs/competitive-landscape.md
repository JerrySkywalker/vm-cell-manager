# Competitive Landscape

VM Cell Manager is not based on the premise that cross-platform VM tooling is empty. The project is justified only by a narrower product gap.

This document records the initial comparison so future contributors can decide when to build, wrap, or stop.

## smolvm

Project: https://github.com/smol-machines/smolvm

Strong overlap:

- Rust implementation;
- Windows/Linux/macOS hosts;
- ephemeral VM execution;
- developer/automation workloads;
- hardware virtualization backends.

Primary difference for VM Cell Manager:

- `vmcell` treats full-system Windows guests and Windows engineering environments as first-class workloads rather than focusing on Linux microVM/rootfs execution.
- `vmcell` intentionally includes a native Hyper-V provider on Windows in addition to QEMU portability.

Decision: learn from it; do not reimplement its Linux-microVM niche.

## Multipass

Project: https://github.com/canonical/multipass

Strong overlap:

- cross-platform local VM lifecycle;
- Hyper-V on Windows;
- QEMU-based paths on Linux/macOS;
- simple CLI experience.

Primary difference for VM Cell Manager:

- Multipass is centered on Ubuntu/cloud-style instances;
- VM Cell Manager targets provider-neutral full-system engineering images, including Windows;
- VM Cell Manager is daemonless by default and exposes local manifests/provider state rather than requiring a persistent service as the core lifecycle owner.

Decision: do not compete for the Ubuntu-instance use case.

## Vagrant

Project: https://github.com/hashicorp/vagrant

Strong overlap:

- provider abstraction;
- reusable machine images/boxes;
- Windows guests;
- lifecycle and provisioning commands.

Primary difference for VM Cell Manager:

- Vagrant is an environment-description and provisioning product;
- VM Cell Manager is a smaller runtime primitive for disposable local cells and avoids a general provisioning DSL.

Decision: do not grow into a Vagrant-compatible configuration system.

## Lima

Project: https://github.com/lima-vm/lima

Lima is an excellent Linux-VM environment, especially on macOS. It is not the primary reference for full Windows engineering guests across all three host families.

Decision: study its QEMU/HVF and host integration choices where relevant, but keep Windows/full-system requirements explicit.

## Tart

Project: https://github.com/cirruslabs/tart

Tart is a strong Apple Silicon virtualization/CI tool for macOS and Linux guests.

Decision: study image/OCI and CI ergonomics for a future macOS-native path; do not make Apple Silicon the sole architecture model.

## libvirt / virt-manager

Projects:

- https://libvirt.org/
- https://github.com/virt-manager/virt-manager

These are broader virtualization-management layers. libvirt may be useful as a future provider boundary on Linux, but it is intentionally not required by the bootstrap architecture.

Decision: reuse or wrap later if its additional lifecycle surface becomes useful; do not duplicate generic virtualization-management breadth.

## QEMU and Hyper-V

These are foundational virtualization technologies, not product-level competitors to the execution-cell abstraction.

VM Cell Manager should call stable interfaces exposed by them rather than emulate their responsibilities.

## Product test

The project remains justified only while it preserves this intersection:

```text
full-system engineering VM
+
full Windows guest first-class
+
Hyper-V native Windows path
+
QEMU portable reference path
+
immutable base / disposable overlay
+
daemonless local ownership
+
Rust automation surface
```

If an existing project begins to cover this intersection well enough, adopting or contributing upstream is preferable to maintaining a redundant implementation.
