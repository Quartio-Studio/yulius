//! Sandbox shell DTOs (docs/api.md "Browser, editor, shell"; SPEC-001 UI-015, UI-016).

use serde::{Deserialize, Serialize};

/// `POST /api/sandboxes/{sid}/shells` body; the body may also be empty.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateShell {
    /// Open the shell in this task's worktree (a task of the same sandbox); the sandbox
    /// repository otherwise.
    #[serde(default)]
    pub task_id: Option<String>,
    /// Display name; "Terminal N" (the lowest free number) when absent.
    #[serde(default)]
    pub name: Option<String>,
}

/// `PATCH /api/sandboxes/{sid}/shells/{id}` body.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RenameShell {
    /// New display name (1 to 40 characters, no control characters).
    pub name: String,
}

/// One of the caller's shells: `GET /api/sandboxes/{sid}/shells` lists them, `POST` and
/// `PATCH` return one. Attach with `WS /api/shells/{id}/terminal`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Shell {
    /// Shell id.
    pub id: String,
    /// Display name.
    pub name: String,
    /// The task whose worktree it started in; `null` for the sandbox repository.
    pub task_id: Option<String>,
    /// Absolute path of the folder it started in, as its processes see it: paths printed in
    /// it link to the editor (F 2026-10-10).
    pub folder: String,
    /// Creation time (RFC 3339).
    pub created_at: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_shell_accepts_an_optional_task_and_name() -> Result<(), serde_json::Error> {
        assert_eq!(
            serde_json::from_str::<CreateShell>("{}")?,
            CreateShell::default()
        );
        assert_eq!(
            serde_json::from_str::<CreateShell>(r#"{"task_id":"t1"}"#)?.task_id,
            Some("t1".into())
        );
        assert_eq!(
            serde_json::from_str::<CreateShell>(r#"{"name":"build"}"#)?.name,
            Some("build".into())
        );
        assert!(serde_json::from_str::<CreateShell>(r#"{"cwd":"/"}"#).is_err());
        assert!(serde_json::from_str::<RenameShell>("{}").is_err());
        Ok(())
    }
}
