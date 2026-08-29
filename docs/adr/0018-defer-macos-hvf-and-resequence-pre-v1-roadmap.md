# ADR 0018: Defer macOS/HVF and resequence the pre-v1 roadmap

## Status

Accepted as the repository decision for independent audit. This decision does
not establish real-platform acceptance, promote support, publish a release, or
authorize issue mutation.

## Context

The owner has no Apple hardware and cannot produce honest macOS, Apple Silicon,
Intel Mac, or HVF evidence before v1. Keeping an Apple-dependent v0.5 milestone
ahead of unrelated Windows and native Linux work therefore turns an unavailable
external platform into a blocker for the product paths the owner can actually
develop and qualify.

Repository-local parsing, provider, and capability abstractions already model
macOS and HVF. That vocabulary is useful forward compatibility, but it is not a
support row, acceptance packet, release gate, or statement that an Apple path
has been tested. Historical ADRs and frozen release evidence also record the
decisions and limits that existed when they were written; rewriting them would
misstate history.

## Decision

- The active pre-v1 hosts are Windows x86_64 and native Linux x86_64.
- v0.5.0 becomes **Windows and Linux Portability Closeout**, with sequential
  platform-contract, workflow/distribution-parity, and audit/closeout slices.
- Pre-v1 progression depends only on declared Windows and native Linux paths.
  Repository correctness remains distinct from real-platform acceptance, and
  no support row is promoted without exact real-platform evidence.
- macOS, Apple Silicon, Intel Mac, and HVF are deferred post-v1. Deferral is not
  a claim that these paths are impossible or permanently unsupported.
- `HostOs::Macos`, `Accelerator::Hvf`, CLI/schema values, and provider parsing
  abstractions remain modeled for forward compatibility.
- Actual macOS/HVF and macOS/TCG entries are removed from `SUPPORT_MATRIX`.
  Lookup of any absent Mac tuple returns the existing undocumented-combination
  error and fails closed.
- TCG remains an explicit development-only path on the declared Windows/Linux
  rows. It is never inferred, config-authorized, or selected as a fallback.
- The frozen v0.1-v0.4.1 release register, including all four exact v0.4.1 R5
  `NOT_EXECUTED` rows, remains historical and immutable. This decision does not
  add fabricated Windows or Linux acceptance.
- v0.6 through v1.0 are resequenced without an Apple-hardware dependency. The
  repository-local Reliability A-G work already completed under issue #48 is
  retained as completed foundation; this decision neither reimplements it nor
  manufactures a v0.8 release.
- GitHub issue/backlog metadata will be aligned only after this repository
  decision is independently audited and merged. This authoring change does not
  update or close issues #43, #44, #45, #49, or #50.

## History-preserving issue alignment

Issue #43 remains the historical Apple/HVF Three-Host Portability planning
record. Its title, body, comments, and timeline must not be retitled, rewritten,
or made to imply that the planned Apple work was completed or accepted.

After PR #77 is merged and exact-dev CI is green, the intended sequence is:

1. Close Issue #43 as `not_planned` or superseded with a short pointer to this
   ADR, PR #77, and the new v0.5 issue, while preserving all existing history.
2. Create a new **v0.5 Windows/Linux Portability Closeout** issue.
3. Update Issues #44, #45, #49, and #50 to depend on the new issue rather than
   rewriting Issue #43 into a different project.

The closeout must state that Apple/macOS/HVF work was deferred, not completed
or accepted. Apple Silicon, Intel Mac, macOS lifecycle work, and HVF acceptance
remain post-v1, non-blocking future direction. This ADR records the plan only;
it does not authorize or perform any issue mutation.

## Residual occurrence classification

Every remaining Apple/macOS/HVF occurrence must fit exactly one category:

1. **Preserved typed future vocabulary.** Enums, parsers, CLI/schema values,
   provider capability code, and negative/test fixtures may model Mac or HVF.
   They confer no selection, support, acceptance, or pre-v1 release meaning.
2. **Explicitly deferred post-v1 direction.** Active product documentation may
   discuss an eventual macOS/HVF path or a possible Apple
   Virtualization.framework provider only when labeled post-v1 and non-blocking.
3. **Immutable historical record.** Earlier ADRs and frozen release evidence
   remain unchanged and non-authorizing.
4. **Defect requiring correction.** Any active pre-v1 milestone, support row,
   acceptance packet, tooling owner, or release gate that depends on Apple
   hardware is removed or corrected by this decision.

Ambiguous active occurrences are defects. Absence from the support and
acceptance catalogs is intentional fail-closed behavior, not implicit support.

## Consequences

Pre-v1 work can advance through honest Windows and native Linux evidence while
the codebase retains compatible vocabulary for future Apple work. The declared
catalog becomes more conservative because two unevidenced Mac rows disappear;
no retained row changes status. A future post-v1 Apple effort requires a new
admission decision, hardware and architecture-specific evidence, a newly
reviewed support row and acceptance packet, and the same explicit boundary that
TCG never becomes an implicit fallback.
