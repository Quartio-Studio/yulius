//! Auto-saved notes (SPEC-041 NOT-003..007): one notepad per sandbox, shared by the people who can
//! see it (writers edit it), one for the team, shared by everyone signed in, and one per person
//! that only they read and edit, the same in every sandbox (SPEC-103 NOT-011..013). Edits are whole
//! texts with the revision they started from; a stale revision is refused, so a teammate's edit is
//! never overwritten silently.
//!
//! Presence (SPEC-112 NOT-018..019): where each person's selection is in a shared notepad, relayed
//! to the notepad's readers as they send it and stored nowhere, at most
//! [`PRESENCE_PER_SECOND`] updates per person and notepad.

use yulius_contracts::api::notes::{Note, NotePresence, SaveNote};
use yulius_contracts::events::UiEvent;
use yulius_domain::ids::{SandboxId, UserId};
use yulius_domain::permission::Action;
use yulius_domain::time::Timestamp;

use crate::auth::{AuthContext, rfc3339};
use crate::context::App;
use crate::error::{AppError, AppResult};

/// Largest note body, in bytes of UTF-8 (NOT-006).
pub const MAX_NOTE_BYTES: usize = 256 * 1024;

/// Presence updates a person may send per notepad in [`PRESENCE_WINDOW_MS`] (NOT-019).
pub const PRESENCE_PER_SECOND: usize = 20;
/// The window [`PRESENCE_PER_SECOND`] counts in.
const PRESENCE_WINDOW_MS: i64 = 1_000;
/// Most (person, notepad) counters kept at once; past it, idle ones are dropped first.
const MAX_PRESENCE_COUNTERS: usize = 4_096;
/// Largest offset a selection may name: notes hold at most 256 KiB, far fewer UTF-16 units.
pub const MAX_PRESENCE_OFFSET: u32 = 1 << 20;

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

    /// `PUT /api/notes`, `PUT /api/sandboxes/{sid}/notes` and `PUT /api/me/notes`: stores the
    /// text when the note is still at `input.revision` and announces it.
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
        let event = UiEvent::NoteUpdated {
            scope: scope.wire().to_owned(),
            sandbox_id: match &scope {
                Scope::Sandbox(sid) => Some(sid.to_string()),
                Scope::Team | Scope::Me(_) => None,
            },
            revision: stored.revision,
            updated_by: stored.updated_by.clone(),
        };
        match &scope {
            Scope::Team => self.events.publish(None, false, event),
            Scope::Sandbox(sid) => self.events.publish(Some(sid.clone()), false, event),
            // Its owner's other tabs and devices, and nobody else (SPEC-103 NOT-013).
            Scope::Me(user) => self.events.publish_to_user(user.clone(), event),
        }
        Ok(view(&scope, Some(stored)))
    }

    /// `PUT /api/notes/presence` and `PUT /api/sandboxes/{sid}/notes/presence`: relays the
    /// caller's selection to the notepad's readers (NOT-018). There is no presence in "Me".
    ///
    /// # Errors
    /// `NotFound` without read access to the sandbox, `Invalid` for an offset over
    /// [`MAX_PRESENCE_OFFSET`], `Capacity` past [`PRESENCE_PER_SECOND`].
    pub async fn note_presence(
        &self,
        ctx: &AuthContext,
        target: NoteTarget<'_>,
        input: &NotePresence,
    ) -> AppResult<()> {
        if matches!(target, NoteTarget::Me) {
            return Err(AppError::NotFound);
        }
        let scope = self.note_scope(ctx, target, Action::View).await?;
        if input
            .selection
            .is_some_and(|s| s.anchor.max(s.head) > MAX_PRESENCE_OFFSET)
        {
            return Err(AppError::Invalid(
                "a selection lies within the notes' text".into(),
            ));
        }
        self.count_presence(&format!("{}|{}", ctx.user.id, scope.key()))?;
        let name = ctx
            .user
            .github_login
            .clone()
            .unwrap_or_else(|| ctx.user.display_name.clone());
        let event = UiEvent::NotePresence {
            scope: scope.wire().to_owned(),
            sandbox_id: match &scope {
                Scope::Sandbox(sid) => Some(sid.to_string()),
                Scope::Team | Scope::Me(_) => None,
            },
            user_id: ctx.user.id.to_string(),
            name,
            revision: input.revision,
            selection: input.selection,
            hello: input.hello,
        };
        match &scope {
            Scope::Sandbox(sid) => self.events.publish(Some(sid.clone()), false, event),
            Scope::Team | Scope::Me(_) => self.events.publish(None, false, event),
        }
        Ok(())
    }

    /// Counts one presence update under `key`, refusing it past the limit (NOT-019).
    fn count_presence(&self, key: &str) -> AppResult<()> {
        const REFUSAL: &str = "too many presence updates; slow down";
        let now = self.now();
        let mut counters = self
            .note_presence
            .lock()
            .map_err(|_| AppError::Internal("presence limiter poisoned".into()))?;
        if !counters.contains_key(key) && counters.len() >= MAX_PRESENCE_COUNTERS {
            counters.retain(|_, times| {
                times.retain(|t| now.0 - *t < PRESENCE_WINDOW_MS);
                !times.is_empty()
            });
            if counters.len() >= MAX_PRESENCE_COUNTERS {
                return Err(AppError::Capacity(REFUSAL.into()));
            }
        }
        let times = counters.entry(key.to_owned()).or_default();
        times.retain(|t| now.0 - *t < PRESENCE_WINDOW_MS);
        if times.len() >= PRESENCE_PER_SECOND {
            return Err(AppError::Capacity(REFUSAL.into()));
        }
        times.push(now.0);
        Ok(())
    }
}
