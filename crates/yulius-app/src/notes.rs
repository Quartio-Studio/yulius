//! Auto-saved notes (SPEC-041 NOT-003..007): one notepad per sandbox, shared by the people who can
//! see it (writers edit it), one for the team, shared by everyone signed in, and one per person
//! that only they read and edit, the same in every sandbox (SPEC-103 NOT-011..013). Edits are whole
//! texts with the revision they started from; a stale revision is refused, so a teammate's edit is
//! never overwritten silently.

use yulius_contracts::api::notes::{Note, SaveNote};
use yulius_contracts::events::UiEvent;
use yulius_domain::ids::{SandboxId, UserId};
use yulius_domain::permission::Action;
use yulius_domain::time::Timestamp;

use crate::auth::{AuthContext, rfc3339};
use crate::context::App;
use crate::error::{AppError, AppResult};

/// Largest note body, in bytes of UTF-8 (NOT-006).
pub const MAX_NOTE_BYTES: usize = 256 * 1024;

/// A stored notepad.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NoteRecord {
    /// `team`, `sandbox:<sid>` or `user:<user id>`.
    pub scope: String,
    /// The text.
    pub body: String,
    /// Bumped by every stored edit (1 for the first).
    pub revision: u64,
    /// Who saved it last.
    pub updated_by: String,
    /// When.
    pub updated_at: Timestamp,
}

/// The notepad a request names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoteTarget<'a> {
    /// The team's.
    Team,
    /// The sandbox's with this id.
    Sandbox(&'a str),
    /// The caller's own: nothing can name another person's.
    Me,
}

/// Which notepad.
enum Scope {
    Team,
    Sandbox(SandboxId),
    Me(UserId),
}

impl Scope {
    fn key(&self) -> String {
        match self {
            Self::Team => "team".to_owned(),
            Self::Sandbox(sid) => format!("sandbox:{sid}"),
            Self::Me(user) => format!("user:{user}"),
        }
    }

    fn wire(&self) -> &'static str {
        match self {
            Self::Team => "team",
            Self::Sandbox(_) => "sandbox",
            Self::Me(_) => "me",
        }
    }
}

fn view(scope: &Scope, record: Option<NoteRecord>) -> Note {
    match record {
        Some(r) => Note {
            scope: scope.wire().to_owned(),
            body: r.body,
            revision: r.revision,
            updated_at: Some(rfc3339(r.updated_at)),
            updated_by: Some(r.updated_by),
        },
        None => Note {
            scope: scope.wire().to_owned(),
            body: String::new(),
            revision: 0,
            updated_at: None,
            updated_by: None,
        },
    }
}

impl App {
    /// The notepad `target` names, checked for `action`.
    async fn note_scope(
        &self,
        ctx: &AuthContext,
        target: NoteTarget<'_>,
        action: Action,
    ) -> AppResult<Scope> {
        let sandbox = match target {
            NoteTarget::Team => return Ok(Scope::Team),
            NoteTarget::Me => return Ok(Scope::Me(ctx.user.id.clone())),
            NoteTarget::Sandbox(sandbox) => sandbox,
        };
        let sid = SandboxId::parse(sandbox).map_err(|_| AppError::NotFound)?;
        self.require(&ctx.user, &sid, action).await?;
        Ok(Scope::Sandbox(sid))
    }

    /// `GET /api/notes`, `GET /api/sandboxes/{sid}/notes` and `GET /api/me/notes`.
    ///
    /// # Errors
    /// `NotFound` without read access to the sandbox.
    pub async fn note(&self, ctx: &AuthContext, target: NoteTarget<'_>) -> AppResult<Note> {
        let scope = self.note_scope(ctx, target, Action::View).await?;
        let record = self.deps.store.note(&scope.key()).await?;
        Ok(view(&scope, record))
    }

    /// `PUT /api/notes`, `PUT /api/sandboxes/{sid}/notes` and `PUT /api/me/notes`: stores the text when the note is
    /// still at `input.revision` and announces it.
    ///
    /// # Errors
    /// `Forbidden` without write access to the sandbox, `Invalid` over [`MAX_NOTE_BYTES`],
    /// `Conflict` when someone saved a newer revision meanwhile.
    pub async fn save_note(
        &self,
        ctx: &AuthContext,
        target: NoteTarget<'_>,
        input: &SaveNote,
    ) -> AppResult<Note> {
        let scope = self.note_scope(ctx, target, Action::Work).await?;
        if input.body.len() > MAX_NOTE_BYTES {
            return Err(AppError::Invalid(format!(
                "notes hold at most {} KiB",
                MAX_NOTE_BYTES / 1024
            )));
        }
        let by = ctx
            .user
            .github_login
            .clone()
            .unwrap_or_else(|| ctx.user.display_name.clone());
        let stored = self
            .deps
            .store
            .put_note(&scope.key(), &input.body, input.revision, &by, self.now())
            .await?
            .ok_or_else(|| {
                AppError::Conflict("someone else changed these notes; load them again".into())
            })?;
        let (sandbox_id, wire_sandbox) = match &scope {
            Scope::Team => (None, None),
            Scope::Sandbox(sid) => (Some(sid.clone()), Some(sid.to_string())),
            // Announced to its owner alone (SPEC-103 NOT-013).
            Scope::Me(_) => return Ok(view(&scope, Some(stored))),
        };
        self.events.publish(
            sandbox_id,
            false,
            UiEvent::NoteUpdated {
                scope: scope.wire().to_owned(),
                sandbox_id: wire_sandbox,
                revision: stored.revision,
                updated_by: stored.updated_by.clone(),
            },
        );
        Ok(view(&scope, Some(stored)))
    }
}
