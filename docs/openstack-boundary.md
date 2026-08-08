# OpenStack Boundary

OpenStack is intentionally not a VM Cell Manager provider in the initial architecture.

## Different problem layers

VM Cell Manager manages disposable VM execution cells on the **current local host**:

```text
CLI / local caller
        │
        ▼
      vmcell
        │
   local provider
   ┌────┴────┐
   ▼         ▼
Hyper-V     QEMU
   │         │
local VM   local VM
```

OpenStack provides a distributed IaaS control plane across a resource pool:

```text
API / cloud client
       │
       ▼
cloud control plane
       │
 scheduler / placement
       │
 ┌─────┼─────┐
 ▼     ▼     ▼
host  host  host
```

A cloud control plane owns concerns such as remote compute placement, identity, quotas, networking services, image distribution, multi-tenant policy, and controller availability. Those responsibilities would make the local runtime materially larger and less predictable.

## Why OpenStack is not a `LocalVmProvider`

The provider contract in this repository assumes:

- the execution host is local;
- VM lifecycle is directly observable from that host;
- owned disks/processes/VM objects have local cleanup semantics;
- a cell request does not invoke a remote scheduler to choose a different machine.

An OpenStack request violates those assumptions. Hiding that semantic difference behind the same local provider trait would produce a misleading abstraction.

## Future composition

If a future higher-level execution system needs both workstation-local cells and a private cloud, the correct shape is peer backends:

```text
Higher-level execution/orchestration layer
        │
        ├── Local execution -> vm-cell-manager
        │                         ├── Hyper-V
        │                         └── QEMU
        │
        └── Cloud execution -> OpenStack / another cloud API
```

The higher layer can normalize workload intent and evidence while preserving the different resource, ownership, and failure semantics of each backend.

## When to reconsider

This repository should not add OpenStack merely because more than one personal machine exists. Remote invocation of `vmcell` on a small set of machines is still conceptually simpler than introducing an IaaS cloud.

A real OpenStack/cloud backend becomes relevant when requirements include a substantial compute pool, dynamic placement, tenant/resource quotas, shared image services, virtual networking as infrastructure, or cloud-style availability/operations.

Even then, support belongs first in the higher-level orchestrator rather than inside the local `vmcell` runtime.
