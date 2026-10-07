//! Sandbox shell DTOs (docs/api.md "Browser, editor, shell"; SPEC-001 UI-015).

use serde::{Deserialize, Serialize};

/// `POST /api/sandboxes/{sid}/shells` body; the body may also be empty.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateShell {
    /// Open the shell in this task's worktree (a task of the same sandbox); the sandbox
    /// repository otherwise.
    #[serde(default)]
    pub task_id: Option<String>,
}

/// `POST /api/sandboxes/{sid}/shells` response; attach with `WS /api/shells/{id}/terminal`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShellCreated {
    /// Shell id.
    pub id: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_shell_accepts_an_optional_task() -> Result<(), serde_json::Error> {
        assert_eq!(
            serde_json::from_str::<CreateShell>("{}")?,
            CreateShell::default()
        );
        assert_eq!(
            serde_json::from_str::<CreateShell>(r#"{"task_id":"t1"}"#)?.task_id,
            Some("t1".into())
        );
        assert!(serde_json::from_str::<CreateShell>(r#"{"cwd":"/"}"#).is_err());
        Ok(())
    }
}
