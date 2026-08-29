//! The monotonic run lifecycle.
//!
//! ```text
//! accepted -> queued -> awaiting_approval -> dispatched -> running
//!                                          -> (running) -> succeeded | failed | canceled | expired
//! ```
//!
//! `awaiting_approval` is skipped for low-risk CI/build work; destructive,
//! production, credential, registry, and rollback operations pass through it.
//! Transitions are monotonic: a terminal state has no successors, and no
//! transition ever moves "backwards". This is the single source of truth for
//! what a valid transition is — the runtime stores (sandlot's active store,
//! the durable data-mesh record) enforce it through [`RunState::can_transition_to`].

use serde::{Deserialize, Serialize};

/// A run's lifecycle state. Ordered from earliest to terminal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunState {
    Accepted,
    Queued,
    AwaitingApproval,
    Dispatched,
    Running,
    Succeeded,
    Failed,
    Canceled,
    Expired,
}

impl RunState {
    /// A terminal state has no valid successors.
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            RunState::Succeeded | RunState::Failed | RunState::Canceled | RunState::Expired
        )
    }

    /// The states this one may transition to. `canceled`/`expired` are reachable
    /// from every non-terminal state (cancellation, timeout, lease loss).
    pub fn allowed_successors(self) -> &'static [RunState] {
        use RunState::*;
        match self {
            Accepted => &[Queued, Canceled, Expired],
            Queued => &[AwaitingApproval, Dispatched, Canceled, Expired],
            AwaitingApproval => &[Dispatched, Canceled, Expired],
            // Worker loss before a job starts is a Failed/Expired, not a silent drop.
            Dispatched => &[Running, Failed, Canceled, Expired],
            Running => &[Succeeded, Failed, Canceled, Expired],
            Succeeded | Failed | Canceled | Expired => &[],
        }
    }

    /// Whether `self -> to` is a valid, monotonic transition.
    pub fn can_transition_to(self, to: RunState) -> bool {
        self.allowed_successors().contains(&to)
    }
}

#[cfg(test)]
mod tests {
    use super::RunState::*;

    #[test]
    fn happy_path_ci_skips_approval() {
        // accepted -> queued -> dispatched -> running -> succeeded
        assert!(Accepted.can_transition_to(Queued));
        assert!(Queued.can_transition_to(Dispatched));
        assert!(Dispatched.can_transition_to(Running));
        assert!(Running.can_transition_to(Succeeded));
    }

    #[test]
    fn approval_path_is_valid_when_required() {
        assert!(Queued.can_transition_to(AwaitingApproval));
        assert!(AwaitingApproval.can_transition_to(Dispatched));
        // approval can be refused -> canceled
        assert!(AwaitingApproval.can_transition_to(Canceled));
    }

    #[test]
    fn terminal_states_have_no_successors() {
        for t in [Succeeded, Failed, Canceled, Expired] {
            assert!(t.is_terminal());
            assert!(t.allowed_successors().is_empty());
            for s in [Accepted, Queued, Running, Succeeded, Failed] {
                assert!(
                    !t.can_transition_to(s),
                    "{t:?} must not transition to {s:?}"
                );
            }
        }
    }

    #[test]
    fn transitions_are_monotonic_no_going_backwards() {
        // running cannot return to queued/accepted/awaiting_approval/dispatched
        for back in [Accepted, Queued, AwaitingApproval, Dispatched] {
            assert!(
                !Running.can_transition_to(back),
                "running -> {back:?} must be rejected"
            );
        }
        assert!(!Dispatched.can_transition_to(Queued));
        assert!(!Queued.can_transition_to(Accepted));
    }

    #[test]
    fn cancel_and_expire_reachable_from_every_nonterminal() {
        for s in [Accepted, Queued, AwaitingApproval, Dispatched, Running] {
            assert!(s.can_transition_to(Canceled), "{s:?} -> canceled");
            assert!(s.can_transition_to(Expired), "{s:?} -> expired");
        }
    }

    #[test]
    fn state_serializes_snake_case() {
        assert_eq!(
            serde_json::to_value(AwaitingApproval).unwrap(),
            serde_json::json!("awaiting_approval")
        );
    }
}
