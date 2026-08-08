# Security Policy

VM Cell Manager is infrastructure software that may create, start, stop, and destroy virtual machines. Until the first stable release, security reports should be submitted privately to the repository owner rather than as public exploit demonstrations.

## Current security posture

The project is pre-1.0 and intentionally fail-closed around host mutation. The initial implementation will not automatically enable hypervisors, create host-global virtual switches, or modify virtual machines that are not owned by VM Cell Manager.

## Reporting

Please use GitHub's private vulnerability reporting feature when it is enabled for this repository. If it is not available, contact the repository owner privately and include reproduction steps, affected platform/provider, and the expected impact.
