//! What a person may do to a task (SPEC-490 OWN-001, ADR-0331): the one rule every way in asks.
//!
//! A task acts on GitHub with its owner's token (ADR-0060), so steering it is the owner's. Ending
//! it or putting it away takes nothing of the owner's, so anyone who may work in its project does
//! that. Whether a person owns a task, and their access to its project, are the only facts.

use crate::permission::{AccessLevel, Action};

/// What a person does to a task.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TaskAction {
    /// Read it: detail, request, transcript, delivery, changes.
    View,
    /// End it or put it away: stop (also cancel one that waits to start), close out, archive,
    /// restore, delete. Nothing here steers the agent or acts on GitHub.
    Tidy,
    /// Steer it: type into it, send it messages, answer what it asks, change how it runs or
    /// starts, deliver it, write into its worktree, answer a request for control.
    Control,
}

/// Why a person may not do something to a task.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TaskRefusal {
    /// Their access to the project is below what the action needs.
    Access,
    /// The action is the owner's and they are not the owner.
    NotOwner,
}

impl TaskAction {
    /// A sandbox-level action taken on a task: viewing is viewing, and anything more is control
    /// unless its path names [`TaskAction::Tidy`] itself, so a new way to change a task is the
    /// owner's (fail closed).
    #[must_use]
    pub fn of(action: Action) -> Self {
        match action {
            Action::View => Self::View,
            Action::Work | Action::Configure | Action::Administer => Self::Control,
        }
    }

    /// The access to the task's project this needs.
    #[must_use]
    pub fn sandbox_action(self) -> Action {
        match self {
            Self::View => Action::View,
            Self::Tidy | Self::Control => Action::Work,
        }
    }

    /// Whether only the task's owner does this.
    #[must_use]
    pub fn owner_only(self) -> bool {
        self == Self::Control
    }
}

/// Decides `action` on a task for a person with `level` access to its project who owns it or
/// not (`owns`).
///
/// # Errors
/// [`TaskRefusal::Access`] below the needed level (checked first: what a person may not even
/// see or change in the project is not explained by its owner), [`TaskRefusal::NotOwner`] for
/// control by anyone else, whatever their level.
pub fn decide(action: TaskAction, level: AccessLevel, owns: bool) -> Result<(), TaskRefusal> {
    if !level.allows(action.sandbox_action()) {
        return Err(TaskRefusal::Access);
    }
    if action.owner_only() && !owns {
        return Err(TaskRefusal::NotOwner);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const LEVELS: [AccessLevel; 4] = [
        AccessLevel::None,
        AccessLevel::Read,
        AccessLevel::Write,
        AccessLevel::Admin,
    ];

    /// SPEC-490 AC-4: the whole table. View from read access; tidy from write access, owner or
    /// not; control from write access for the owner alone, an administrator included.
    #[test]
    fn the_rule_s_table() {
        for level in LEVELS {
            for owns in [true, false] {
                let reads = level >= AccessLevel::Read;
                let works = level >= AccessLevel::Write;
                assert_eq!(
                    decide(TaskAction::View, level, owns),
                    if reads {
                        Ok(())
                    } else {
                        Err(TaskRefusal::Access)
                    },
                    "view at {level:?}, owner {owns}"
                );
                assert_eq!(
                    decide(TaskAction::Tidy, level, owns),
                    if works {
                        Ok(())
                    } else {
                        Err(TaskRefusal::Access)
                    },
                    "tidy at {level:?}, owner {owns}"
                );
                let control = match (works, owns) {
                    (false, _) => Err(TaskRefusal::Access),
                    (true, false) => Err(TaskRefusal::NotOwner),
                    (true, true) => Ok(()),
                };
                assert_eq!(
                    decide(TaskAction::Control, level, owns),
                    control,
                    "control at {level:?}, owner {owns}"
                );
            }
        }
    }

    /// An administrator who does not own a task tidies it and never steers it.
    #[test]
    fn an_administrator_is_no_exception() {
        assert_eq!(decide(TaskAction::Tidy, AccessLevel::Admin, false), Ok(()));
        assert_eq!(
            decide(TaskAction::Control, AccessLevel::Admin, false),
            Err(TaskRefusal::NotOwner)
        );
    }

    /// A sandbox action above view on a task is control: a path that does not name tidy is the
    /// owner's.
    #[test]
    fn above_view_means_control() {
        assert_eq!(TaskAction::of(Action::View), TaskAction::View);
        for action in [Action::Work, Action::Configure, Action::Administer] {
            assert_eq!(TaskAction::of(action), TaskAction::Control);
        }
        assert_eq!(TaskAction::Tidy.sandbox_action(), Action::Work);
        assert_eq!(TaskAction::Control.sandbox_action(), Action::Work);
        assert_eq!(TaskAction::View.sandbox_action(), Action::View);
    }
}
