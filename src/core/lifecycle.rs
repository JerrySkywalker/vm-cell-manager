use thiserror::Error;

use super::cell::CellState;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum LifecycleError {
    #[error("invalid cell state transition: {from:?} -> {to:?}")]
    InvalidTransition { from: CellState, to: CellState },
}

pub fn validate_transition(from: CellState, to: CellState) -> Result<(), LifecycleError> {
    let valid = matches!(
        (from, to),
        (CellState::Creating, CellState::Stopped)
            | (CellState::Creating, CellState::Failed)
            | (CellState::Stopped, CellState::Running)
            | (CellState::Stopped, CellState::Destroying)
            | (CellState::Running, CellState::Stopped)
            | (CellState::Running, CellState::Destroying)
            | (CellState::Running, CellState::Failed)
            | (CellState::Failed, CellState::Destroying)
            | (CellState::Destroying, CellState::Destroyed)
            | (CellState::Destroying, CellState::Failed)
    );

    if valid {
        Ok(())
    } else {
        Err(LifecycleError::InvalidTransition { from, to })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disposable_lifecycle_accepts_create_stop_run_destroy() {
        assert_eq!(
            validate_transition(CellState::Creating, CellState::Stopped),
            Ok(())
        );
        assert_eq!(
            validate_transition(CellState::Stopped, CellState::Running),
            Ok(())
        );
        assert_eq!(
            validate_transition(CellState::Running, CellState::Destroying),
            Ok(())
        );
        assert_eq!(
            validate_transition(CellState::Destroying, CellState::Destroyed),
            Ok(())
        );
    }

    #[test]
    fn destroyed_cells_are_terminal() {
        assert!(validate_transition(CellState::Destroyed, CellState::Running).is_err());
    }
}
