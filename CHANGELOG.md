# Changelog

All notable changes to this project will be documented in this file.

The project is currently pre-release and follows an architecture-first bootstrap process.

## [Unreleased]

### Added
- Initial open-source repository governance and project bootstrap.
- M1 ownership/state manifests, immutable VHDX registration, Hyper-V differencing-disk and VM lifecycle contracts, read-only reconciliation, versioned JSON output, and mock-driven safety tests.
- Subprocess crash-consistency, duplicate-root, cross-installation authority, tombstone stress, and cross-process state-containment coverage for the M1 real-acceptance boundary.
- M2 engine-issued guest authority, bounded PowerShell Direct execution and copy contracts, non-secret durable operation records, hash-bound artifact storage, TTL creation, and explicit exact-owned garbage collection.
- Mock, schema, credential-redaction, timeout/output, artifact-crash, concurrent-GC, and Windows reparse safety coverage for M2.
- Provider-neutral lifecycle authority, bounded process execution, QMP/QGA protocol foundations, QCOW2 immutable-base/single-overlay ownership, explicit QEMU accelerator policy, and stacked QEMU CLI routing for M3.
- Fake QMP/QGA, launch-digest, no-network, explicit-TCG, provider-authority, process-timeout, and provider-neutral lifecycle coverage for M3.
- M4 Unix private-state/no-follow identity gates, Linux KVM device usability filtering, canonical QEMU executable discovery, Unix QMP socket coverage, and a native-Linux validation workflow.

### Changed
- Hardened M1 provider authority, physical runtime containment, persisted identity binding, partial-provisioning recovery, and tombstone reconciliation before real Hyper-V acceptance.
- Narrowed advertised Hyper-V guest transports to PowerShell Direct and QEMU guest transport to implemented Linux QGA behavior; SSH remains unsupported.
