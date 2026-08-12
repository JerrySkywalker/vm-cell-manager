//! Explicit, opt-in repository-local reliability checks.
//!
//! This target is intentionally ignored by the normal test suite. It uses a
//! fixed seed, a disjoint corpus, and canonical serialized reproductions so a
//! failure can be rerun without a provider, guest, host command, or filesystem
//! fixture. Broader plan/job/image matrices belong to their own reliability
//! slices rather than being folded into this harness bootstrap.

use serde::Serialize;
use vm_cell_manager::core::cell::CellState;
use vm_cell_manager::core::lifecycle::validate_transition;

const CONTRACT: &str = "vmcell.reliability-harness.v1";
const LIFECYCLE_SEED: u64 = 0x6a09_e667_f3bc_c909;
const MAX_LIFECYCLE_MINIMIZER_CASES: usize = 31;

const CELL_STATES: [CellState; 6] = [
    CellState::Creating,
    CellState::Stopped,
    CellState::Running,
    CellState::Destroying,
    CellState::Destroyed,
    CellState::Failed,
];

// These are intentionally the exact examples in `core::lifecycle`'s normal
// unit tests. The extended corpus must never replay them.
const CANONICAL_LIFECYCLE_CASES: [(CellState, CellState); 5] = [
    (CellState::Creating, CellState::Stopped),
    (CellState::Stopped, CellState::Running),
    (CellState::Running, CellState::Destroying),
    (CellState::Destroying, CellState::Destroyed),
    (CellState::Destroyed, CellState::Running),
];

// This is an independent, data-oriented oracle for the valid transitions
// absent from the canonical examples above. Every other member of the disjoint
// corpus is expected to be rejected.
const ADDITIONAL_VALID_LIFECYCLE_CASES: [(CellState, CellState); 6] = [
    (CellState::Creating, CellState::Failed),
    (CellState::Stopped, CellState::Destroying),
    (CellState::Running, CellState::Stopped),
    (CellState::Running, CellState::Failed),
    (CellState::Failed, CellState::Destroying),
    (CellState::Destroying, CellState::Failed),
];

#[test]
#[ignore = "extended reliability suite; run explicitly with cargo test --test reliability_harness -- --ignored"]
fn seeded_lifecycle_cases_are_reproducible_and_disjoint_from_normal_ci() {
    let first = lifecycle_cases(LIFECYCLE_SEED);
    let second = lifecycle_cases(LIFECYCLE_SEED);

    assert_eq!(
        first, second,
        "{CONTRACT}: fixed corpus generation must be reproducible for seed={LIFECYCLE_SEED:016x}"
    );
    assert_eq!(first.len(), MAX_LIFECYCLE_MINIMIZER_CASES);

    for (position, case) in first.iter().copied().enumerate() {
        assert!(
            !is_canonical_lifecycle_case(case.from, case.to),
            "extended corpus must stay disjoint from canonical lifecycle cases: {}",
            case.reproduction(),
        );
        assert!(
            !first[..position]
                .iter()
                .any(|previous| previous.from == case.from && previous.to == case.to),
            "extended corpus must not repeat a case: {}",
            case.reproduction(),
        );

        let actual = validate_transition(case.from, case.to);
        assert_eq!(
            actual.is_ok(),
            case.expected_valid,
            "reproduction={}; minimized_input={}",
            case.reproduction(),
            minimize_model_mismatch(case).reproduction(),
        );
    }
}

#[test]
#[ignore = "extended reliability suite; run explicitly with cargo test --test reliability_harness -- --ignored"]
fn bounded_minimizer_returns_a_real_rejected_transition_as_serialized_input() {
    let original = lifecycle_cases(LIFECYCLE_SEED)
        .into_iter()
        .next()
        .expect("the fixed corpus must not be empty");

    let first = minimize_case(original, |case| {
        validate_transition(case.from, case.to).is_err()
    })
    .expect("the bounded corpus must contain a rejected transition");
    let second = minimize_case(original, |case| {
        validate_transition(case.from, case.to).is_err()
    })
    .expect("the bounded corpus must contain a rejected transition");

    assert_eq!(first, second);
    assert!(validate_transition(first.from, first.to).is_err());
    assert_eq!(
        first.reproduction(),
        format!(
            "{{\"contract\":\"{CONTRACT}\",\"seed\":\"{LIFECYCLE_SEED:016x}\",\"source_case_index\":0,\"lifecycle\":{{\"from\":\"creating\",\"to\":\"creating\"}}}}"
        )
    );
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct LifecycleCase {
    seed: u64,
    source_case_index: usize,
    from: CellState,
    to: CellState,
    expected_valid: bool,
}

impl LifecycleCase {
    fn reproduction(self) -> String {
        serde_json::to_string(&ReproductionInput {
            contract: CONTRACT,
            seed: format!("{:016x}", self.seed),
            source_case_index: self.source_case_index,
            lifecycle: SerializedLifecycleInput {
                from: lifecycle_state_name(self.from),
                to: lifecycle_state_name(self.to),
            },
        })
        .expect("the fixed reliability reproduction input must serialize")
    }
}

#[derive(Serialize)]
struct ReproductionInput {
    contract: &'static str,
    seed: String,
    source_case_index: usize,
    lifecycle: SerializedLifecycleInput,
}

#[derive(Serialize)]
struct SerializedLifecycleInput {
    from: &'static str,
    to: &'static str,
}

fn lifecycle_cases(seed: u64) -> Vec<LifecycleCase> {
    let mut cases = ordered_noncanonical_lifecycle_cases(seed);
    let mut generator = SplitMix64::new(seed);

    for index in (1..cases.len()).rev() {
        cases.swap(index, generator.next_index(index + 1));
    }

    cases
}

fn ordered_noncanonical_lifecycle_cases(seed: u64) -> Vec<LifecycleCase> {
    CELL_STATES
        .into_iter()
        .flat_map(|from| CELL_STATES.into_iter().map(move |to| (from, to)))
        .filter(|(from, to)| !is_canonical_lifecycle_case(*from, *to))
        .enumerate()
        .map(|(source_case_index, (from, to))| LifecycleCase {
            seed,
            source_case_index,
            from,
            to,
            expected_valid: is_additional_valid_lifecycle_case(from, to),
        })
        .collect()
}

fn is_canonical_lifecycle_case(from: CellState, to: CellState) -> bool {
    CANONICAL_LIFECYCLE_CASES.contains(&(from, to))
}

fn is_additional_valid_lifecycle_case(from: CellState, to: CellState) -> bool {
    ADDITIONAL_VALID_LIFECYCLE_CASES.contains(&(from, to))
}

const fn lifecycle_state_name(state: CellState) -> &'static str {
    match state {
        CellState::Creating => "creating",
        CellState::Stopped => "stopped",
        CellState::Running => "running",
        CellState::Destroying => "destroying",
        CellState::Destroyed => "destroyed",
        CellState::Failed => "failed",
    }
}

fn minimize_model_mismatch(original: LifecycleCase) -> LifecycleCase {
    minimize_case(original, |candidate| {
        validate_transition(candidate.from, candidate.to).is_ok() != candidate.expected_valid
    })
    .unwrap_or(original)
}

fn minimize_case(
    original: LifecycleCase,
    mut fails: impl FnMut(LifecycleCase) -> bool,
) -> Option<LifecycleCase> {
    // The fixed ordered corpus is the minimization metric. The first matching
    // member is therefore the stable, smallest reproduction under this v1
    // harness contract; this is deliberately not a general delta debugger.
    let candidates = ordered_noncanonical_lifecycle_cases(original.seed);
    assert_eq!(
        candidates.len(),
        MAX_LIFECYCLE_MINIMIZER_CASES,
        "changing the lifecycle corpus requires an explicit minimizer-bound update"
    );

    candidates
        .into_iter()
        .take(MAX_LIFECYCLE_MINIMIZER_CASES)
        .find(|candidate| fails(*candidate))
}

#[derive(Debug, Clone, Copy)]
struct SplitMix64 {
    state: u64,
}

impl SplitMix64 {
    const fn new(seed: u64) -> Self {
        Self { state: seed }
    }

    fn next_index(&mut self, upper_bound: usize) -> usize {
        debug_assert!(upper_bound > 0);
        (self.next_u64() % upper_bound as u64) as usize
    }

    fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut value = self.state;
        value = (value ^ (value >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        value = (value ^ (value >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        value ^ (value >> 31)
    }
}
