//! Validated identifier newtypes.
//!
//! Identifiers are opaque ASCII strings (letters, digits, `-`, `_`), 1–64 bytes. Generation is
//! an application concern; the domain only validates.

use std::fmt;

/// Rejected identifier text.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("invalid identifier: must be 1-64 ASCII letters, digits, '-' or '_'")]
pub struct IdError;

fn valid(text: &str) -> bool {
    !text.is_empty()
        && text.len() <= 64
        && text
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

macro_rules! define_id {
    ($(#[$meta:meta])* $name:ident) => {
        $(#[$meta])*
        #[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
        pub struct $name(String);

        impl $name {
            /// Validates and wraps an identifier.
            ///
            /// # Errors
            /// Returns [`IdError`] when the text is empty, longer than 64 bytes or contains
            /// characters other than ASCII letters, digits, `-` and `_`.
            pub fn parse(text: &str) -> Result<Self, IdError> {
                if valid(text) { Ok(Self(text.to_owned())) } else { Err(IdError) }
            }

            /// The identifier text.
            #[must_use]
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }
    };
}

define_id!(
    /// A Yulius user (superadmin or GitHub-authenticated user).
    UserId
);
define_id!(
    /// A sandbox: one GitHub repository and its isolated workspace.
    SandboxId
);
define_id!(
    /// A task: one coding-agent session lineage.
    TaskId
);
define_id!(
    /// One process run of a task.
    AttemptId
);
define_id!(
    /// A connected provider account.
    AccountId
);
define_id!(
    /// An automation definition.
    AutomationId
);
define_id!(
    /// A workflow definition.
    WorkflowId
);
define_id!(
    /// An inter-task message.
    MessageId
);
define_id!(
    /// A task step.
    StepId
);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_uuid_like_and_rejects_bad_text() {
        assert!(TaskId::parse("3f2b1c9e-aaaa-4bbb-8ccc-0123456789ab").is_ok());
        assert!(TaskId::parse("task_1").is_ok());
        assert_eq!(TaskId::parse(""), Err(IdError));
        assert_eq!(TaskId::parse("../etc"), Err(IdError));
        assert_eq!(TaskId::parse("a b"), Err(IdError));
        assert_eq!(TaskId::parse(&"a".repeat(65)), Err(IdError));
    }
}
