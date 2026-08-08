# ADR 0003: OpenStack is not a local provider

- Status: Accepted
- Date: 2026-08-08

## Context

The provider abstraction is intentionally local: the execution host is known, provider state is directly observable, and owned VM/disk/process cleanup has local semantics.

OpenStack introduces a remote cloud control plane with placement, identity, image, network, quota, and multi-host semantics. Treating it as merely another local hypervisor provider would hide material differences in ownership and failure behavior.

## Decision

Do not implement `OpenStackProvider` inside VM Cell Manager.

If a future orchestration layer needs local workstation execution and private-cloud execution, `vmcell` and OpenStack should be separate peer backends selected by that higher layer.

## Consequences

- Provider traits can remain intentionally local and simpler.
- The project does not need cloud credentials, tenancy, placement, or remote network semantics.
- Multi-machine personal use may invoke `vmcell` remotely without redefining the local provider model.
- A future cloud backend belongs outside this repository unless the product scope is deliberately re-chartered.
