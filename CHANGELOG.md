# Changelog

All notable changes to this project will be documented in this file.

The project is currently pre-release and follows an architecture-first bootstrap process.

## [Unreleased]

- Added a schema-versioned, read-only `vmcell status` aggregate and concise
  doctor/list/inspect/reconcile/operation diagnostics for provider availability,
  cell retention/expiry, uncertain guest operations, and fail-closed cleanup
  guidance.
- Deterministic portable Windows ZIP/checksum packaging with schema-versioned
  build provenance, install/remove instructions, repeat-build verification,
  and a manual trusted-runner artifact workflow.
- A release-gated, copyable Windows Human MVP quick start aligned with CLI help:
  portable install, doctor, prepared-VHDX validation/registration, one bounded
  `vmcell run`, and explicit cleanup verification.

### Added
- Stable M5 JSON schema-version constants and a provider-neutral automation
  error envelope with deterministic codes, categories, retryability, and exit
  statuses.
- Stable provider-neutral reconciliation ownership/action classifications and
  a versioned machine-readable doctor/provider capability contract.
- Bounded state-lock waiting, explicit dry-run-capable artifact retention,
  manifest size/count revalidation, and redacted durable/CLI diagnostics.
- Provider-normalized readiness, versioned redacted JSON for argument failures,
  explicit legacy CLI migration behavior, and cross-provider contract tests.
- Initial open-source repository governance and project bootstrap.
- M1 ownership/state manifests, immutable VHDX registration, Hyper-V differencing-disk and VM lifecycle contracts, read-only reconciliation, versioned JSON output, and mock-driven safety tests.
- Subprocess crash-consistency, duplicate-root, cross-installation authority, tombstone stress, and cross-process state-containment coverage for the M1 real-acceptance boundary.
- M2 engine-issued guest authority, bounded PowerShell Direct execution and copy contracts, non-secret durable operation records, hash-bound artifact storage, TTL creation, and explicit exact-owned garbage collection.
- Mock, schema, credential-redaction, timeout/output, artifact-crash, concurrent-GC, and Windows reparse safety coverage for M2.
- Provider-neutral lifecycle authority, bounded process execution, QMP/QGA protocol foundations, QCOW2 immutable-base/single-overlay ownership, explicit QEMU accelerator policy, and stacked QEMU CLI routing for M3.
- Fake QMP/QGA, launch-digest, no-network, explicit-TCG, provider-authority, process-timeout, and provider-neutral lifecycle coverage for M3.
- M4 Unix private-state/no-follow identity gates, Linux KVM device usability filtering, canonical QEMU executable discovery, Unix QMP socket coverage, and a native-Linux validation workflow.

### Changed
- Adopted a permanent stable-`main`/integration-`dev` branch model: ephemeral
  `agent/*` development, frozen `release/vX.Y.Z` promotion, `hotfix/*`
  synchronization, exact-head/exact-dev/exact-main gates, immutable version
  tags, and real-platform acceptance tracked separately from repository-local
  merge eligibility.
- Hardened M1 provider authority, physical runtime containment, persisted identity binding, partial-provisioning recovery, and tombstone reconciliation before real Hyper-V acceptance.
- Narrowed advertised Hyper-V guest transports to PowerShell Direct and QEMU guest transport to implemented Linux QGA behavior; SSH remains unsupported.
