# Version-neutral contract and compatibility inventory

Status: repository-local prework on the exact `dev` base
`27dcc1c56db91f8c8ce34bcb8d7e3ed667962158`. This inventory and its fixtures
do not choose issue #61, mint a corrected candidate, change a version, or renew
package, CI, support, or real-platform evidence.

## Frozen fixture binding

`tests/fixtures/compat/manifest.json` carries contract
`vmcell.frozen-compatibility-fixtures.v1` and binds the immutable v0.1-v0.4
source SHAs, Cargo versions, Rust 1.85.0 contract, durable-state fixture set,
JobSpec availability, and historical portable-package layout identities. All
four exact frozen candidates remain `RETIRED_CORRECTION_REQUIRED`.

The shared `legacy-v1` state tree represents the byte-compatible direct state
surface emitted throughout v0.1-v0.4: installation, image, cell, guest
operation, and operation-bound artifact records. The `job-correlated-v2` tree
adds the v0.4 JobSpec correlation fields. The committed v0.4 TOML fixture is a
strict schema-v1 input; the future-schema and secret-like fixtures are negative
reader cases. Paths are materialized only inside a test-owned temporary root;
on Unix the materialized directories/files are explicitly private (`0700` /
`0600`) before the production reader opens them.

Package fixtures are metadata snapshots, not archived binaries. They bind the
historical archive names, platform families, checksum file, and layout-contract
source. v0.1-v0.2 have the Windows portable family, while v0.3-v0.4 also have
the Linux user-portable family. No frozen archive or receipt is reused as
evidence for a future corrected tree.

## Current contract inventory

| Surface | Version/identity | Durable or public boundary | Deterministic evidence | Remaining limitation |
| --- | --- | --- | --- | --- |
| CLI grammar and exit status | clap command surface; stable exit classes 0/2/3/4/5/6/7/8/9/10 | public automation/human interface | `cli_automation_contract`, CLI unit tests | help text is tested structurally, not stored as a complete golden file |
| JSON envelopes | schema 1 plus named `vmcell.*.v1` contracts | stdout success and stderr error envelopes | CLI automation tests and committed compatibility golden subsets | timestamps and fresh IDs are intentionally not golden values |
| JobSpec | `vmcell.job-spec.v1` | strict bounded TOML input; source bytes SHA-256-bound to plans/results | committed v0.4 valid, future, and secret-like fixtures; parser and CLI rejection tests | no claim that v0.1-v0.3 accepted JobSpec; they did not expose it |
| Job plan/result | `vmcell.job-plan.v1`, `vmcell.job-result.v1`, `vmcell.job-operations.v1` | non-authorizing plan and bounded result metadata | core/CLI serialization and model-matrix tests | no historical real guest result is a current golden |
| Durable state | format 1 direct; format 2 job-correlated | installation/image/cell/operation/artifact JSON | committed v1/v2 trees; real `vmcell state check`; complete tree byte snapshot before/after | no automatic migration or downgrade contract |
| Image | image schema 1 and immutable binding | registered metadata plus external base identity | state fixture, image validation, unregister and replacement tests | fixtures never open or validate a real VHDX/QCOW2 payload |
| Artifact | artifact schema 1 direct; schema 2 job-correlated | operation-bound manifest, relative files, hash and size | committed artifact bytes/manifests plus state compatibility check | package/CI fixtures do not prove guest transport or collection |
| Windows package | versioned `windows-x86_64.zip`, adjacent `SHA256SUMS.txt` | deterministic user-owned portable layout | current package scripts/tests; frozen metadata fixture revisions | old archive bytes are intentionally absent; renewed build receipt required |
| Linux package | versioned `linux-x86_64.tar.gz`, adjacent `SHA256SUMS.txt` | deterministic user-owned portable layout | current package scripts/tests; v0.3/v0.4 metadata fixtures | old archive bytes are intentionally absent; renewed build receipt required |

## Read, reject, and no-rewrite rehearsal

`tests/frozen_compatibility_contract.rs` runs the current binary in fresh
processes. It proves:

1. format-1 frozen state is accepted as `compatible`, reports exact record
   counts, redacts legacy provider detail on read, and retains an identical
   directory/file byte snapshot;
2. v0.4 format-2 job-correlated state is accepted with the same no-rewrite
   proof;
3. a future image schema fails with integrity exit 9 and
   `vmcell.state.upgrade_required`, without changing the state tree or external
   base sentinel;
4. the v0.4 JobSpec parses, while future-schema and secret-like inputs fail
   with `vmcell.job_spec.unsupported_schema` or the bounded invalid-document
   class before state creation, preserve source bytes, and do not disclose
   fixture paths or sentinel content.

The rehearsal is provider-free. It neither probes nor mutates Hyper-V, QEMU,
WHPX, KVM, HVF, QGA, a guest, a runner, or a host service.

## Golden, redaction, and error-taxonomy gaps

Committed goldens deliberately match only stable fields. The success golden
binds the state compatibility contract/status/counts; error goldens bind schema,
code, category, retryability, and exit. Dynamic `checked_at`, fresh UUIDs,
timings, provider observations, and prose messages remain non-golden.

Redaction coverage now includes a committed legacy-state provider-detail
sentinel and a committed secret-like JobSpec input. Broader provider stderr,
configuration, argv, receipt-control, and path-redaction cases remain covered
by focused unit/integration tests. These fixtures contain synthetic sentinels,
not credentials.

The CLI classifier has deterministic coverage for its error families, but a
committed golden for every individual error code would add high churn without
strengthening the frozen reader decision. The highest-value compatibility
goldens are therefore the two stop-before-mutation classes exercised here:
future durable state and future/invalid JobSpec.

## Authority and next decision

This prework supplies reader and contract evidence only. It does not authorize
a version bump, release ref/tag, package publication, support promotion,
provider/guest execution, real-platform acceptance, migration, downgrade, or a
selection in issue #61. If the Owner later selects a correction strategy, the
chosen new source tree still needs fresh package identities, exact-source
hosted Windows/Linux CI, independent audit, and new dedicated-host R5 receipts
for every tuple claimed.
