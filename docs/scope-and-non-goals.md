# Scope and Non-goals

VM Cell Manager is intentionally narrow. Its purpose is to make a disposable full-system VM on the **local machine** behave like a predictable execution-cell primitive for developers and automation.

## In scope

- Windows, Linux, and macOS host portability where the underlying platform permits it.
- Full-system Windows and Linux guests as first-class engineering workloads.
- Native Hyper-V on Windows.
- QEMU with KVM, HVF, and WHPX as the portable reference path.
- Immutable logical images with provider-specific variants.
- Single-layer copy-on-write cell overlays.
- Local lifecycle: create, start, inspect, stop, destroy, garbage collect.
- Guest command/file transports such as PowerShell Direct, QGA, and SSH.
- Capability discovery rather than lowest-common-denominator feature hiding.
- Automation-first CLI, JSON schemas, deterministic exit behavior, and conservative ownership checks.
- Optional future provider features such as TPM, Secure Boot, nested virtualization, or device/GPU capabilities when they are justified by real local workloads.

## Out of scope

### Cloud and cluster control

The project does not own:

- multi-host placement;
- cloud tenancy;
- quotas;
- distributed scheduling;
- distributed image registries;
- SDN or virtual routers as a platform service;
- IAM;
- high-availability controller clusters;
- OpenStack-compatible APIs.

### Higher-level orchestration

The project does not own:

- agent planning;
- Task DAGs;
- repository leases;
- CI job queues;
- GitHub Actions protocols;
- MCP orchestration;
- autonomous retry policy;
- human approval or production promotion policy.

Higher-level systems may call `vmcell` as one execution backend.

### Application-specific automation

The project does not know how to operate MATLAB, STK, Ansys, SolidWorks, Visual Studio, or other engineering applications. It supplies the guest environment in which those applications and their own APIs/automation layers run.

### Physical HIL

Physical boards, JTAG probes, lab instruments, CAN devices, USB programmers, and other HIL resources have different ownership and rollback semantics. They belong in physical-resource systems such as dedicated HIL tooling, not in this VM runtime.

### General VM administration

VM Cell Manager is not intended to replace Hyper-V Manager, virt-manager, VMware/Parallels management interfaces, or an infrastructure inventory GUI. Long-lived pet VMs are not the core product model.

## Stop conditions

Development should reconsider the project rather than duplicate mature tools if the target scope collapses into one of these categories:

- Linux-only microVM sandbox -> evaluate smolvm first.
- Ubuntu/cloud-style local instances -> evaluate Multipass first.
- declarative development environment provisioning -> evaluate Vagrant first.
- generic hypervisor API/GUI -> evaluate libvirt/virt-manager or native tools first.
- multi-host IaaS -> evaluate OpenStack or a cloud platform first.

The project's reason to exist is the remaining intersection: full-system engineering VMs, especially Windows guests, exposed as disposable local execution cells through a small Rust automation surface.
