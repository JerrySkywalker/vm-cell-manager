# ADR 0002: Separate VM providers from guest I/O transports

- Status: Accepted
- Date: 2026-08-08

## Context

Hypervisor lifecycle and guest command execution do not share the same portability boundary. Hyper-V can manage both Windows and Linux VMs, while PowerShell Direct is specifically valuable for Windows guests. QEMU lifecycle is naturally managed through QMP, while guest execution may use QGA or SSH.

Coupling guest execution directly to a hypervisor provider would make provider code accumulate operating-system and application-specific concerns.

## Decision

Use two interfaces:

- `LocalVmProvider`: create/inspect/start/stop/destroy and provider capabilities;
- `GuestTransport`: execute commands and move files across the host/guest boundary.

Examples:

```text
Hyper-V + Windows -> PowerShell Direct
Hyper-V + Linux   -> SSH
QEMU + Windows    -> QGA
QEMU + Linux      -> QGA or SSH
```

## Consequences

- Guest networking is not required for all workflows.
- New guest transports can be added without adding hypervisors.
- New providers can reuse existing guest transports.
- Application-specific APIs/MCP remain outside both interfaces.
