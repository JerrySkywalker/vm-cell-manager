# VMCell one-week Windows Hyper-V owner-preview plan

Status: **OWNER APPROVED IMPLEMENTATION BASELINE**  
Contract: `vmcell.one-week-windows-hyperv-plan.v1`  
Authorizing: **false**  
Real-platform acceptance: **NOT_STARTED**

## 1. Purpose

This plan defines the shortest conservative route to an owner-local usable
VMCell preview on one dedicated Windows x86_64 host using Hyper-V and
PowerShell Direct. It is not a public launch plan, a v1.0 plan, a support
promotion, a release publication, or authority to mutate Hyper-V.

The target outcome is deliberately narrow:

- exact checksum-bound VMCell `0.4.1` candidate;
- one legitimate owner-controlled Windows Server 2022 x86_64 image;
- Generation 2 Hyper-V VM with Secure Boot observed before first start;
- networkless guest control through PowerShell Direct;
- bounded execution, copy, artifact, recovery, reconciliation, and cleanup;
- three sequential successful owner-local runs;
- unchanged immutable base image and preserved foreign host state;
- sanitized, non-promoting evidence.

Apple hardware, macOS, HVF, Linux, QEMU, WHPX, Docker, public packaging, and
v1.0 are outside the critical path.

## 2. Audited planning baseline

The following identities are planning inputs, not permission to skip fresh
admission:

| Item | Audited value |
| --- | --- |
| `dev` before PR #77 | `03cf6d425aeaca9fddd909bd3c330dd669f78549` |
| PR #77 corrected head | `958688452a6b8e7534fdeeaad2eefff242b2acf9` |
| frozen release ref | `release/v0.4.1` |
| frozen release SHA | `0e7fcf37f4310562d318f9d5c709ddf8e8ca1637` |
| frozen Windows binary SHA-256 | `249db6841161d634449142584ad7924b26cbe7b31a41eca9b813dd2eb8acec1b` |
| package version | `0.4.1` |
| current real-platform status | `NOT_STARTED` / `untested` |

Every execution must fetch and record live refs. Any incompatible change makes
these values stale and requires a new audit decision.

## 3. Definition of owner-local usable

The preview is usable only when all of U0 through U9 are proven:

| Gate | Required evidence |
| --- | --- |
| U0 — identity | Exact candidate source, archive, binary, version, and hashes agree. |
| U1 — host admission | Dedicated Windows x86_64 host, elevated owner-attended session, Hyper-V capability, VMMS, provider read access, and exclusive window are proven. |
| U2 — containment | State, runtime, image, package, and binary paths resolve to admitted ordinary non-reparse local NTFS boundaries with adequate free capacity. |
| U3 — image | Windows Server 2022 VHDX is fixed, parentless, detached, read-only, ordinary, non-reparse, hash-bound, provenance-bound, and legally owner-controlled. |
| U4 — stopped cell | One exact-owned stopped VM is created; Generation 2, Secure Boot, networkless configuration, one overlay, and exact parent are observed before start. |
| U5 — guest control | PowerShell Direct reaches a configured local guest profile using credentials supplied only through bounded stdin. |
| U6 — functionality | Bounded command execution proves stdout, stderr, zero and nonzero exit behavior, copy-in, copy-out, and artifact hashing. |
| U7 — repetition | Three sequential runs use fresh cell/operation identities and produce no authority reuse. |
| U8 — recovery | One admitted failure or interruption case is reconciled without blind replay or foreign-object adoption. |
| U9 — cleanup | Repeated exact-owned cleanup is idempotent; VM, runtime, overlay, and endpoints are absent; base hash and foreign inventory are unchanged. |

Passing U0–U9 means `OWNER_LOCAL_PREVIEW_PASS`. It does not automatically make
the support matrix `experimental` or `supported`.

## 4. Required pre-real-run correction

One narrow post-PR-77 qualification-tooling/documentation PR is required before
the live admission is trusted. It must not change the production Rust binary.

Required changes:

1. Replace hard-coded `C:` and unconditional `V:` observations with
   path-derived classification for every supplied operational path.
2. Add an explicit runtime-root input and bind it independently from the state
   root.
3. Distinguish operational storage from sanitized evidence storage. Evidence
   may be stored separately; its location must never cause an unrelated
   operational drive to be treated as candidate authority.
4. Validate ordinary/non-reparse ancestry, local backing, NTFS policy, and
   available capacity for state, runtime, image, package, and binary paths.
5. Add a non-overwriting evidence-output contract with a pre-existing ordinary
   parent or document a wrapper that provides exactly those guarantees.
6. Add a post-create, pre-start read-only check for Generation 2, Secure Boot,
   disk parentage, network adapter count, and exact VM identity.
7. Add deterministic fixtures for every accepted/rejected storage and firmware
   case. Generic CI must never invoke Live mode.
8. Add an owner-preview guide bound to the exact candidate and safe credential
   channel. Historical quickstarts must not be silently repurposed.

The PR must preserve `Cargo.lock`, package version `0.4.1`, support rows, frozen
release evidence, `main`, and `release/v0.4.1`.

## 5. Seven-day critical path

### Day 1 — governance closeout and fresh baseline

- Independently confirm PR #77 still has the audited head, base, diff, and
  exact-head Hosted Windows/Linux results.
- Merge PR #77 only through the normal protected workflow.
- Wait for exact merged-`dev` Hosted Windows/Linux qualification.
- Create the isolated qualification-tooling branch, worktree, target directory,
  and external checkpoint.
- Record candidate/package/binary hashes and verify no production source change
  is planned.

Terminal Day-1 result: `PR77_CLOSED_EXACT_DEV_GREEN` or a precise protected
workflow blocker.

### Day 1–2 — one tooling/docs PR

- Implement the eight corrections in section 4.
- Expand fixtures and AST/static mutation denial tests.
- Run all canonical PowerShell, Rust, formatting, fixture, build, disclosure,
  and diff gates with bounded build resources.
- Push one branch and open one non-draft PR into `dev`.
- Wait for exact-head Hosted Windows/Linux CI and a fresh independent audit.
- Merge only if P0=0, P1=0, P2=0 and required checks pass; then verify exact-dev
  CI.

This lane is suitable for an 8–12 hour unattended Codex execution. It may wait
for external CI and perform at most two bounded corrections attributable to the
PR.

### Day 1–2 in parallel — owner image preparation

This is a separate attended lane. VMCell does not download, license, activate,
install, or distribute Windows.

- Use legitimate owner-controlled Windows Server 2022 media or image.
- Prepare a Generation-2-compatible, fixed VHDX outside VMCell.
- Ensure at least one local user profile exists and PowerShell Direct
  prerequisites are met.
- Remove embedded secrets and product keys from all repository, argv,
  environment, log, and receipt channels.
- Detach the image, prove parentless state, mark it read-only, compute SHA-256,
  and complete the sanitized provenance packet.
- Place operational inputs beneath admitted ordinary private non-reparse local
  NTFS roots with sufficient capacity.

The prepared image is not accepted merely because a manifest says
`generation=2`; actual stopped-VM firmware evidence is still required.

### Day 3 — offline binding and read-only live admission

- Revalidate exact merged tooling and frozen candidate identities.
- Validate the completed provenance packet without modifying image bytes.
- Run the exact non-authorizing Live preflight once in an owner-attended
  elevated read context.
- Preserve raw local evidence outside Git and produce only a sanitized receipt.
- Stop if any observation is unavailable, access denied, stale, mismatched, or
  inferred rather than proven.

Terminal Day-3 result: `PREFLIGHT_PASS` ceiling only. It is never real-platform
acceptance.

### Day 4 — separately authorized real Hyper-V acceptance

- Establish the exclusive window and fresh foreign-state baseline.
- Register and validate the immutable image.
- Create one stopped networkless cell and inspect all stopped-cell invariants.
- Start it only after the Generation 2, Secure Boot, overlay parent, ownership,
  and networkless checks pass.
- Prove PowerShell Direct readiness and run the smallest bounded command first.
- Continue through streams, exit codes, copy, artifacts, repetition, recovery,
  reconciliation, and cleanup only while every previous stage remains proven.

Any unknown effect ends the mutation sequence. Do not auto-repair, replay,
adopt, or delete ambiguous state.

### Day 5 — closeout

- Independently review the complete real-run evidence.
- Verify exact no-residue cleanup, unchanged base hash, and preserved foreign
  inventory.
- Produce a sanitized owner-local preview receipt.
- Decide separately whether the evidence is sufficient for any future support
  proposal. Default is no promotion.

### Day 6–7 — single-defect contingency

- If no production defect is found, retain these days as unused contingency.
- If one bounded production defect is proven, stop using frozen `0.4.1` as the
  execution candidate, preserve evidence, create one correction PR, produce a
  new candidate version/hash/receipt, run full qualification, and repeat only
  the failed acceptance slice plus required regression checks.
- A second production defect, ambiguous effect, image defect, or insufficient
  time ends the week as `PARTIAL_RESUMABLE`, not a rushed PASS.

## 6. Estimated work

| Work | Estimated elapsed effort |
| --- | ---: |
| PR #77 closeout | 1–3 hours including CI |
| Tooling/docs PR | 8–12 Codex hours |
| Owner image preparation | 2–6 attended hours, highly media-dependent |
| Offline binding and Live preflight | 1–2 attended hours |
| Real acceptance | 6–10 attended hours |
| Closeout audit | 2–4 hours |
| Contingency | up to 2 days |

The largest schedule uncertainty is image readiness, not known Rust
implementation work.

## 7. Explicit non-goals

- no Apple/macOS/HVF work;
- no Windows 11/vTPM work;
- no Linux/KVM or QEMU/WHPX acceptance;
- no Docker installation or use;
- no host-feature, network, firewall, service, runner, ACL, or storage repair;
- no automatic Windows download or image preparation;
- no JobSpec overlay requirement for the first owner-local preview;
- no public release, tag, package publication, `main` merge, or support
  promotion;
- no rewriting of frozen release receipts.

## 8. Owner decisions required before execution

The owner must explicitly approve all of the following:

1. `OWNER_LOCAL_PREVIEW_PASS` as the one-week target rather than public support.
2. Windows Server 2022 x86_64, Generation 2, Secure Boot, networkless VM.
3. Operational state/runtime/image/package/binary data on admitted local NTFS;
   evidence storage classified separately.
4. PR #77 closeout before the tooling/docs PR.
5. A single tooling/docs PR that leaves the frozen production binary unchanged.
6. Separate attended authority for image preparation and real Hyper-V work.
7. Stop after one production correction or any unknown effect.

Until those decisions are recorded, this document remains planning evidence
only.
