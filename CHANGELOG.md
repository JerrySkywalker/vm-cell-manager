# Changelog

All notable changes to this project will be documented in this file.

The project is currently pre-release and follows an architecture-first bootstrap process.

## [Unreleased]

- Set the repository candidate identity to `0.3.0` and completed the
  repository-local Cross-Platform Human MVP: provider-neutral support and run
  planning, Windows QEMU/WHPX and native Linux QEMU/KVM foundations for
  prepared Linux QCOW2 + QGA guests, and deterministic Linux distribution.
  Real Hyper-V, PowerShell Direct, WHPX, KVM, QGA, and release publication
  remain separately gated.
- Finalized versioned, non-authorizing human/JSON run-plan regressions across
  Windows/Hyper-V/PowerShell Direct, Windows/QEMU/WHPX/Linux/QGA, and
  Linux/QEMU/KVM/Linux/QGA without promoting any real-platform support row.
- Completed the repository-local v0.2 Windows Daily Driver documentation with
  one canonical install-to-upgrade workflow and explicit real-platform gates.
- Bumped the repository-local candidate identity to `0.2.0` and extended the
  deterministic Windows archive with generated PowerShell completion,
  candidate-only Scoop/WinGet metadata, and bounded install, state-preflight,
  rollback, and removal guidance. No package, release, or tag is published.
- Added a provider-free, read-only `vmcell state check` format-v1 compatibility
  report for v0.1 state, stable upgrade-required rejection, and explicit
  no-rewrite/no-silent-migration guidance.
- Added cooperative Windows `vmcell run` interruption at durable stage
  boundaries, known-completion cleanup handling, bounded one-line password
  stdin, and a fail-closed interruption/recovery matrix.
- Added bounded schema-v1 user configuration for non-authorizing provider,
  CPU/memory, state-root, timeout, and human run-progress defaults, with CLI
  precedence and fail-closed rejection of secret/authority/TCG fields.
- Added provider-neutral image dependency reports and an idempotent,
  dependency-gated `image unregister` command that removes metadata only and
  never reads or deletes registered base-image bytes; bounded file-identity
  metadata checks reject manifest aliases and reparses.
- Added a fail-closed, line-oriented `vmcell shell` workflow over existing
  PowerShell Direct guest authority, with bounded console input, cooperative
  interruption, durable operation IDs, and no automatic lifecycle cleanup.
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
- Native Linux QEMU/KVM/QGA human workflow, typed missing/permission/device-identity KVM diagnostics, bounded collision-safe Unix control endpoints, and a fixture-tested non-authorizing dedicated-host preflight/receipt contract.
- Deterministic `x86_64-unknown-linux-gnu` portable archive assembly with
  generated Bash/Zsh completion, exact source/build provenance, measured GLIBC
  symbol floor, layered SHA-256 manifests, archive-safety regressions, and an
  unprivileged temporary-prefix install/remove smoke contract.

### Changed
- Adopted a permanent stable-`main`/integration-`dev` branch model: ephemeral
  `agent/*` development, frozen `release/vX.Y.Z` promotion, `hotfix/*`
  synchronization, exact-head/exact-dev/exact-main gates, immutable version
  tags, and real-platform acceptance tracked separately from repository-local
  merge eligibility.
- Hardened M1 provider authority, physical runtime containment, persisted identity binding, partial-provisioning recovery, and tombstone reconciliation before real Hyper-V acceptance.
- Narrowed advertised Hyper-V guest transports to PowerShell Direct and QEMU guest transport to implemented Linux QGA behavior; SSH remains unsupported.
