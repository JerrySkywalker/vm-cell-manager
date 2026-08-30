# VMCell unattended development contract

Status: **OWNER APPROVED IMPLEMENTATION BASELINE**
Contract: `vmcell.unattended-development-contract.v1`
Authorizing real-platform work: **false**

## 1. Intent

This contract allows a local Codex process to perform long, coherent repository
development without stopping for every expected warning or short external wait.
It does not grant elevation, host-management authority, image-preparation
authority, or real Hyper-V authority.

The preferred unit is one 8–12 hour repository Goal with durable checkpoints.
A platform execution limit may still interrupt a session; therefore continuity
comes from exact checkpoints and commits, not from assuming a process can run
forever.

## 2. Authorized unattended scope

The unattended agent may:

- read repository instructions and perform fresh Git/GitHub admission;
- close out PR #77 through the normal workflow if its exact audited state is
  unchanged and all merge gates pass;
- create one isolated qualification-tooling branch and worktree from exact
  `origin/dev`;
- modify only preflight tooling, fixtures, static/AST tests, narrow CI wiring,
  owner-preview documentation, and directly related validation tests;
- run fixture-only and static checks on ordinary CI identities;
- run formatting, Clippy, tests, builds, package determinism, disclosure scans,
  and diff checks with bounded resources;
- commit coherent locally green work, push only its work branch, and open one
  PR into `dev`;
- wait for exact-head CI, inspect failures, and perform at most two bounded
  correction cycles attributable to its PR;
- request a fresh independent read-only audit;
- merge normally only after every required gate passes, then verify exact-dev
  CI;
- update external non-secret checkpoint JSON and SHA-256 manifests after each
  phase.

## 3. Prohibited unattended scope

The agent must not:

- invoke the Live Hyper-V preflight;
- elevate, self-relaunch elevated, request UAC, or change group membership;
- query or mutate live Hyper-V, VMMS, VMs, switches, VHD/VHDX, services,
  runners, processes, features, registry, storage, ACLs, network, or firewall;
- download, mount, install, activate, build, convert, resize, or otherwise
  prepare Windows media or images;
- handle guest passwords, product keys, download tokens, private ISO paths, or
  other credentials;
- stop or reconfigure a runner or another Codex process;
- modify production Rust execution paths in the qualification-tooling PR;
- change `Cargo.lock`, package version, support status, `main`, or
  `release/v0.4.1`;
- use Docker, force push, bypass protection, dismiss reviews, self-approve, or
  weaken checks;
- merge while required source-correctness CI is absent or failing;
- describe fixture, hosted CI, or preflight evidence as real-platform PASS.

## 4. Admission before mutation

Before any repository write, the agent must:

1. read `AGENTS.md` and all applicable repository instructions;
2. verify the base checkout is clean and record the `Cargo.lock` digest;
3. fetch and prune `origin`;
4. resolve `origin/dev`, `origin/main`, `origin/release/v0.4.1`, the candidate
   SHA, PR #77 state, and relevant exact-head CI;
5. verify no foreign process owns the requested branch or worktree;
6. stop on protected-ref mismatch, dirty/ambiguous ownership, incompatible
   divergence, or unexpected PR content;
7. create the isolated branch/worktree and an external admission checkpoint
   immediately after successful admission.

No long design-only pause is permitted after admission. The agent proceeds to
the first bounded implementation phase.

## 5. Continue rules

The following are expected conditions and do not independently justify an early
`BLOCKED` result:

- self-hosted R4 fails at checkout because of external network access while
  repository steps never execute, provided repository policy classifies it as
  nonblocking and required Hosted CI is exact-head green;
- Apple/macOS/HVF evidence is absent;
- real Hyper-V acceptance has not started;
- an optional check is unavailable while all required checks are present;
- a GitHub check is pending for less than the configured wait window;
- a P3 observation exists;
- an anticipated fixture fails during implementation and remains inside the
  correction budget;
- external state is unchanged since the last poll;
- the image and owner-attended inputs are not yet ready while the independent
  repository lane still has useful authorized work.

When useful work remains inside scope, the agent continues.

## 6. Valid stop conditions

Early stop is correct only for one of the following:

- protected candidate/release/main identity drift;
- dirty or ambiguously owned starting worktree;
- PR #77 or the tooling PR gains an unexpected commit, file, or author;
- incompatible `dev` divergence;
- P0 or P1 finding;
- unresolved P2 after the bounded correction budget;
- required source-correctness CI failure not attributable to a correctable PR
  change;
- more than two tooling correction cycles;
- suspected credential or private-host disclosure;
- Live provider access occurs in generic CI;
- completion requires elevation, host mutation, image mutation, runner repair,
  or another expansion of owner authority;
- branch protection, review, or permissions explicitly rejects the allowed
  operation;
- an execution budget ends after a durable checkpoint has been written.

The final classification must name the actual blocker. External network,
source correctness, protected workflow, and execution-budget interruption are
different dispositions.

## 7. Phased execution

### Phase A — PR #77 closeout

- Audit exact head/base/diff/reviews/status.
- Merge only if unchanged and admissible.
- Verify exact merge SHA and exact-dev Hosted Windows/Linux CI.
- Checkpoint and continue.

### Phase B — tooling implementation

- Implement path-derived operational storage classification.
- Add explicit runtime-root and evidence-output contracts.
- Add capacity evidence and stopped-cell firmware qualification.
- Expand deterministic fixtures and mutation-deny tests.
- Complete owner-preview documentation.
- Run focused tests continuously; create a coherent local commit when the core
  static and fixture gate is green.

### Phase C — complete local gate

- Run repository canonical formatting and PowerShell validation.
- Run new and existing fixture/static suites.
- Run locked Clippy, tests, doc-tests where applicable, and builds.
- Run package determinism, diff, stale-reference, disclosure, and forbidden
  mutation scans.
- Review the complete diff and resolve all P2 findings.
- Checkpoint and commit final bounded changes.

### Phase D — PR and exact-head qualification

- Push only the work branch.
- Open one non-draft PR to `dev` with safety boundaries and
  `REAL_PLATFORM_ACCEPTANCE=NOT_STARTED`.
- Wait for required exact-head CI for up to eight hours while polling at a
  reasonable interval and recording state changes.
- Correct at most twice. Re-run the complete applicable local gate before each
  push.
- Obtain an independent audit of the final exact head.

### Phase E — merge and exact-dev qualification

- Revalidate head, base, reviews, mergeability, findings, and exact-head CI.
- Merge using the repository's normal strategy.
- Fetch and verify the exact `origin/dev` merge SHA and exact-dev CI.
- Preserve the worktree if exact-dev fails. Clean up only this task's local
  resources after exact-dev is green.

## 8. Checkpoint contract

Each completed phase writes a JSON checkpoint and SHA-256 manifest outside Git
containing only:

- UTC timestamp;
- run ID and phase;
- admitted refs and candidate hashes;
- branch, worktree, current HEAD, and changed-file list;
- completed validation with exact PASS/FAIL/NOT_RUN status;
- PR number, head SHA, workflow run IDs, and tested SHA when applicable;
- P0–P3 findings;
- `Cargo.lock`, version, support, main, and release mutation flags;
- next exact action and resume command;
- all host/provider/image/service/network/release mutation flags.

It must not contain raw host inventory, credentials, commands containing
secrets, private paths, or image bytes.

## 9. Interruption behavior

If the session budget ends:

1. finish only the current atomic file or Git operation when safe;
2. leave a compiling or clearly identified partial state;
3. prefer a coherent local commit after a green phase, but do not commit known
   failing work merely for appearance;
4. update the checkpoint and manifest;
5. report the exact branch, worktree, HEAD, dirty files, completed checks, and
   next command;
6. use `PARTIAL_RESUMABLE_EXECUTION_BUDGET`, not a fabricated protected or
   safety blocker.

## 10. Final unattended receipt

The final response must include:

```text
DISPOSITION=
RUN_ID=
START_DEV=
PR77_FINAL_STATE=
TOOLING_BRANCH=
TOOLING_WORKTREE=
FINAL_HEAD=
FILES_CHANGED=
LOCAL_VALIDATION=
FIXTURE_CASE_COUNT=
FORBIDDEN_MUTATION_AST=
PR_NUMBER=
PR_URL=
PR_HEAD_SHA=
PR_HEAD_WINDOWS_CI=
PR_HEAD_LINUX_CI=
INDEPENDENT_AUDIT=
MERGED=
MERGE_SHA=
EXACT_DEV_WINDOWS_CI=
EXACT_DEV_LINUX_CI=
CARGO_LOCK_CHANGED=
PACKAGE_VERSION=
SUPPORT_STATUS_CHANGED=
MAIN_CHANGED=
RELEASE_V041_CHANGED=
P0_FINDINGS=
P1_FINDINGS=
P2_FINDINGS=
P3_FINDINGS=
REAL_PLATFORM_ACCEPTANCE=NOT_STARTED
HYPERV_MUTATION=false
VM_MUTATION=false
SERVICE_OR_RUNNER_MUTATION=false
HOST_FEATURE_MUTATION=false
NETWORK_MUTATION=false
IMAGE_MUTATION=false
RELEASE_MUTATION=false
CHECKPOINT_ROOT=
BLOCKER=
RESUME_STATE=
SAFEST_NEXT_ACTION=
```

No unexecuted check may be reported as PASS.
