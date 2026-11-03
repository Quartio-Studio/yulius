//! The Editor's shared text (SPEC-411 EDT-016..020, ADR-0261).
//!
//! Contract:
//! - one document per sandbox, folder and path, held in memory only (never in the database or a
//!   file other than the edited file); joined, read and caught up by the people who may read the
//!   folder's files (write access to the sandbox), edited and saved by those who may change it
//!   (a task worktree's owner); its steps go to the sandbox's writers only (`file.live`);
//! - an edit is an operation made at a revision, transformed past the kept steps since and
//!   applied in one critical section that also publishes it, so every stream sees steps in order;
//! - the server writes the text to the file when a browser asks, with that person's access and
//!   the document's `sha256` as base; saves and takes of the disk run one at a time per document;
//!   a disk that moved on under unsaved text is kept as a conflict until someone chooses;
//! - bounded: [`MAX_DOCS`] documents, [`MAX_STEPS`] kept steps each, a saved one dropped after
//!   [`SAVED_IDLE_MS`] untouched, an unsaved one after [`UNSAVED_IDLE_MS`].

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};

use yulius_contracts::api::files::{
    LiveApplied, LiveDoc, LiveEdit, LiveFileAction, LiveJoin, LiveState, LiveStep, OpPart,
    WriteFile,
};
use yulius_contracts::events::UiEvent;
use yulius_domain::ids::SandboxId;

use crate::auth::AuthContext;
use crate::context::App;
use crate::error::{AppError, AppResult};
use crate::files::MAX_FILE_BYTES;
use crate::live_text::{Op, OpError};

/// Most documents held at once.
pub const MAX_DOCS: usize = 256;
/// Steps kept per document for catching up and transforming late edits.
pub const MAX_STEPS: usize = 256;
/// Units the kept steps of one document may insert in all (a paste is kept while it fits).
const MAX_KEPT_UNITS: usize = 1024 * 1024;
/// A saved document untouched this long is dropped: the disk has its text.
pub const SAVED_IDLE_MS: i64 = 10 * 60 * 1000;
/// An unsaved document (a conflict nobody resolved) untouched this long is dropped.
pub const UNSAVED_IDLE_MS: i64 = 24 * 60 * 60 * 1000;
/// Edits per person and file per second.
pub const EDITS_PER_SECOND: usize = 30;
/// Longest session id.
const MAX_SESSION: usize = 32;

/// A live document's id as the wire carries it: 1 to 32 letters and digits.
pub(crate) fn is_session_id(session: &str) -> bool {
    !session.is_empty()
        && session.len() <= MAX_SESSION
        && session.bytes().all(|b| b.is_ascii_alphanumeric())
}

/// The documents.
#[derive(Default)]
pub(crate) struct LiveDocs {
    docs: Mutex<HashMap<String, Doc>>,
}

struct Doc {
    session: String,
    text: Vec<u16>,
    eol: String,
    revision: u64,
    /// `(revision made, client, operation)`, oldest first.
    steps: VecDeque<(u64, String, Op)>,
    kept_units: usize,
    sha256: String,
    /// The revision whose text is the disk's at `sha256`.
    saved_revision: Option<u64>,
    disk_sha256: Option<String>,
    touched: i64,
    /// Saves and takes of the disk, one at a time.
    writes: Arc<tokio::sync::Mutex<()>>,
}

impl Doc {
    fn saved(&self) -> bool {
        self.saved_revision == Some(self.revision)
    }

    fn state(&self) -> LiveState {
        LiveState {
            session: self.session.clone(),
            revision: self.revision,
            sha256: self.sha256.clone(),
            saved: self.saved(),
            disk_sha256: self.disk_sha256.clone(),
        }
    }

    fn whole(&self) -> LiveDoc {
        self.answer(Some(String::from_utf16_lossy(&self.text)), None)
    }

    fn answer(&self, text: Option<String>, steps: Option<Vec<LiveStep>>) -> LiveDoc {
        LiveDoc {
            session: self.session.clone(),
            revision: self.revision,
            text,
            steps,
            eol: self.eol.clone(),
            sha256: self.sha256.clone(),
            saved: self.saved(),
            disk_sha256: self.disk_sha256.clone(),
        }
    }

    /// Applies `op` (at the current revision) as `client`'s step.
    fn step(&mut self, client: &str, op: Op, text: Vec<u16>) {
        self.text = text;
        self.revision += 1;
        self.kept_units += op.inserted();
        self.steps.push_back((self.revision, client.to_owned(), op));
        while self.steps.len() > MAX_STEPS || (self.kept_units > MAX_KEPT_UNITS && self.steps.len() > 1) {
            if let Some((_, _, old)) = self.steps.pop_front() {
                self.kept_units -= old.inserted();
            }
        }
    }
}

/// Where a document lives: its key, the normalized root and path.
struct Place {
    sid: SandboxId,
    root: String,
    path: String,
    key: String,
}

/// The line ending Monaco gives a text (most of its line breaks', else `fallback`), and the text
/// without a BOM in it.
fn normalized(text: &str, fallback: Option<&str>) -> (String, String) {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let (mut crlf, mut cr, mut lf) = (0_usize, 0_usize, 0_usize);
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\r' if chars.peek() == Some(&'\n') => {
                chars.next();
                crlf += 1;
            }
            '\r' => cr += 1,
            '\n' => lf += 1,
            _ => {}
        }
    }
    let total = crlf + cr + lf;
    let eol = match fallback {
        Some(eol @ ("\n" | "\r\n")) => eol,
        _ if total == 0 => "\n",
        _ if (crlf + cr) * 2 > total => "\r\n",
        _ => "\n",
    };
    (in_eol(text, eol), eol.to_owned())
}

/// `text` with every line break as `eol`.
fn in_eol(text: &str, eol: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\r' => {
                if chars.peek() == Some(&'\n') {
                    chars.next();
                }
                out.push_str(eol);
            }
            '\n' => out.push_str(eol),
            c => out.push(c),
        }
    }
    out
}

/// The operation from `a` to `b`: one replacement of the part that differs, never cutting a
/// character in two.
fn replacement(a: &[u16], b: &[u16]) -> Op {
    let shortest = a.len().min(b.len());
    let mut start = a.iter().zip(b).take_while(|(x, y)| x == y).count();
    let mut tail = a
        .iter()
        .rev()
        .zip(b.iter().rev())
        .take(shortest - start)
        .take_while(|(x, y)| x == y)
        .count();
    let high = |unit: u16| (0xd800..=0xdbff).contains(&unit);
    if start > 0 && high(a[start - 1]) {
        start -= 1;
    }
    if tail > 0 && high(a[a.len() - tail - 1]) {
        tail -= 1;
    }
    let mut op = Op::default();
    op.retain(start)
        .delete(a.len() - tail - start)
        .insert(&b[start..b.len() - tail])
        .retain(tail);
    op
}

/// Bytes `text` takes in UTF-8.
fn utf8_len(text: &[u16]) -> usize {
    text.iter()
        .map(|&unit| match unit {
            0..=0x7f => 1,
            0x80..=0x7ff | 0xd800..=0xdfff => 2,
            _ => 3,
        })
        .sum()
}

fn refused(error: OpError) -> AppError {
    AppError::Invalid(match error {
        OpError::Misfit => "the edit does not fit the text; catch up first".into(),
        OpError::Split => "an edit may not cut a character in two".into(),
    })
}

fn ended() -> AppError {
    AppError::Conflict("the live document ended; join it again".into())
}

impl App {
    fn live_lock(&self) -> AppResult<std::sync::MutexGuard<'_, HashMap<String, Doc>>> {
        self.live_docs
            .docs
            .lock()
            .map_err(|_| AppError::Internal("live documents poisoned".into()))
    }

    /// Resolves a file of the folder for a live request (`write`: the caller must be able to
    /// change it).
    async fn live_place(
        &self,
        ctx: &AuthContext,
        sandbox: &str,
        root: Option<&str>,
        path: &str,
        write: bool,
    ) -> AppResult<Place> {
        let root = match root {
            None | Some("" | "sandbox") => "sandbox",
            Some(other) => other,
        };
        let target = self
            .file_target(ctx, sandbox, Some(root), path, write)
            .await?;
        if target.relative.is_empty() {
            return Err(AppError::Invalid("a live document is a file".into()));
        }
        let sid = SandboxId::parse(sandbox).map_err(|_| AppError::NotFound)?;
        Ok(Place {
            key: format!("{sid}\n{root}\n{}", target.relative),
            sid,
            root: root.to_owned(),
            path: target.relative,
        })
    }

    fn live_publish(&self, place: &Place, doc: &Doc, client: &str, op: Option<Vec<OpPart>>) {
        self.events.publish_to_writers(
            place.sid.clone(),
            UiEvent::FileLive {
                sandbox_id: place.sid.to_string(),
                root: place.root.clone(),
                path: place.path.clone(),
                session: doc.session.clone(),
                revision: doc.revision,
                client: client.to_owned(),
                op,
                sha256: doc.sha256.clone(),
                saved: doc.saved(),
                disk_sha256: doc.disk_sha256.clone(),
            },
        );
    }

    /// Drops documents left alone (saved for [`SAVED_IDLE_MS`], any for [`UNSAVED_IDLE_MS`]) and,
    /// for a new one at [`MAX_DOCS`], the least recently touched saved one.
    fn live_room(docs: &mut HashMap<String, Doc>, now: i64) -> AppResult<()> {
        docs.retain(|_, doc| {
            let idle = now - doc.touched;
            idle < UNSAVED_IDLE_MS && !(doc.saved() && idle >= SAVED_IDLE_MS)
        });
        if docs.len() < MAX_DOCS {
            return Ok(());
        }
        let oldest = docs
            .iter()
            .filter(|(_, doc)| doc.saved())
            .min_by_key(|(_, doc)| doc.touched)
            .map(|(key, _)| key.clone());
        match oldest {
            Some(key) => {
                docs.remove(&key);
                Ok(())
            }
            None => Err(AppError::Capacity(
                "too many files are being edited live; try again later".into(),
            )),
        }
    }

    /// `POST /api/sandboxes/{sid}/files/live`: the file's shared text, started from the caller's
    /// text (when they may change the folder) or from the disk when nobody edits it.
    ///
    /// # Errors
    /// Access and path errors as reading the file; `Invalid` for a file that is not text, a bad
    /// `client`, `sha256` or `eol`, or text over the size limit; `Capacity` when full.
    pub async fn live_join(
        &self,
        ctx: &AuthContext,
        sandbox: &str,
        req: &LiveJoin,
    ) -> AppResult<LiveDoc> {
        crate::notes::check_presence(None, 0, &req.client)?;
        if req.sha256.as_deref().is_some_and(|sha| !crate::files::is_sha256_hex(sha)) {
            return Err(AppError::Invalid("sha256 is 64 lowercase hex digits".into()));
        }
        if req.eol.as_deref().is_some_and(|eol| eol != "\n" && eol != "\r\n") {
            return Err(AppError::Invalid("eol is \\n or \\r\\n".into()));
        }
        if req.text.as_ref().is_some_and(|text| u64::try_from(text.len()).unwrap_or(u64::MAX) > MAX_FILE_BYTES) {
            return Err(AppError::Invalid("the text is over the editor's size limit".into()));
        }
        let place = self
            .live_place(ctx, sandbox, req.root.as_deref(), &req.path, false)
            .await?;
        let now = self.now().0;
        if let Some(doc) = self.live_lock()?.get_mut(&place.key) {
            doc.touched = now;
            return Ok(doc.whole());
        }
        let disk = self
            .file_read(ctx, sandbox, Some(&place.root), &place.path, None)
            .await?
            .ok_or(AppError::NotFound)?;
        let Some(content) = disk.content else {
            return Err(AppError::Invalid("the file is not text the editor opens".into()));
        };
        // Only someone who may change the folder starts it from their own text.
        let own = match &req.text {
            Some(text) => self
                .live_place(ctx, sandbox, Some(&place.root), &place.path, true)
                .await
                .is_ok()
                .then_some(text),
            None => None,
        };
        let (on_disk, eol) = normalized(&content, req.eol.as_deref());
        // Clean text read from an older version: the disk's is newer and as clean.
        let stale = req.saved && req.sha256.as_deref() != Some(disk.sha256.as_str());
        let (text, sha256, saved, disk_sha256) = match own {
            Some(text) if !stale => {
                let base = req.sha256.clone().unwrap_or_else(|| disk.sha256.clone());
                let text = in_eol(text, &eol);
                let saved = base == disk.sha256 && text == on_disk;
                let moved = (base != disk.sha256).then(|| disk.sha256.clone());
                (text, base, saved, moved)
            }
            _ => (on_disk, disk.sha256.clone(), true, None),
        };
        let session: String = self.deps.crypto.random_bytes()[..8]
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        let mut docs = self.live_lock()?;
        // Someone else started it meanwhile: theirs.
        if let Some(doc) = docs.get_mut(&place.key) {
            doc.touched = now;
            return Ok(doc.whole());
        }
        Self::live_room(&mut docs, now)?;
        let doc = Doc {
            session,
            text: text.encode_utf16().collect(),
            eol,
            revision: 0,
            steps: VecDeque::new(),
            kept_units: 0,
            sha256,
            saved_revision: saved.then_some(0),
            disk_sha256,
            touched: now,
            writes: Arc::default(),
        };
        let whole = doc.whole();
        docs.insert(place.key, doc);
        Ok(whole)
    }

    /// `GET /api/sandboxes/{sid}/files/live`: the steps after `since` in `session`, or the whole
    /// text when they are no longer kept or the document is another one.
    ///
    /// # Errors
    /// Access and path errors as reading the file; `Conflict` when nobody edits the file live.
    pub async fn live_catch_up(
        &self,
        ctx: &AuthContext,
        sandbox: &str,
        root: Option<&str>,
        path: &str,
        session: &str,
        since: u64,
    ) -> AppResult<LiveDoc> {
        let place = self.live_place(ctx, sandbox, root, path, false).await?;
        let mut docs = self.live_lock()?;
        let doc = docs.get_mut(&place.key).ok_or_else(ended)?;
        doc.touched = self.now().0;
        let oldest = doc.steps.front().map_or(doc.revision, |(made, _, _)| made - 1);
        if doc.session != session || since > doc.revision || since < oldest {
            return Ok(doc.whole());
        }
        let steps = doc
            .steps
            .iter()
            .filter(|(made, _, _)| *made > since)
            .map(|(made, client, op)| LiveStep {
                revision: *made,
                client: client.clone(),
                op: op.to_wire(),
            })
            .collect();
        Ok(doc.answer(None, Some(steps)))
    }

    /// `POST /api/sandboxes/{sid}/files/live/edit`: applies an operation made at `revision`,
    /// transformed past the steps since, and publishes it to the sandbox's writers.
    ///
    /// # Errors
    /// `Forbidden` for a folder the caller may not change; `Conflict` for another or an ended
    /// document, or a revision older than the kept steps; `Invalid` for an operation that does
    /// not fit, cuts a character or makes the text too large; `Capacity` past
    /// [`EDITS_PER_SECOND`].
    pub async fn live_edit(
        &self,
        ctx: &AuthContext,
        sandbox: &str,
        req: &LiveEdit,
    ) -> AppResult<LiveApplied> {
        crate::notes::check_presence(None, 0, &req.client)?;
        if !is_session_id(&req.session) {
            return Err(AppError::Invalid("session is a live document's id".into()));
        }
        let place = self
            .live_place(ctx, sandbox, req.root.as_deref(), &req.path, true)
            .await?;
        self.count_live_edit(
            &format!("{}|live:{}", ctx.user.id, place.key),
            EDITS_PER_SECOND,
        )?;
        let mut op = Op::from_wire(&req.op)
            .ok_or_else(|| AppError::Invalid("a count is beyond any text".into()))?;
        let mut docs = self.live_lock()?;
        let doc = docs.get_mut(&place.key).ok_or_else(ended)?;
        let oldest = doc.steps.front().map_or(doc.revision, |(made, _, _)| made - 1);
        if doc.session != req.session || req.revision > doc.revision || req.revision < oldest {
            return Err(ended());
        }
        for (_, _, step) in doc.steps.iter().filter(|(made, _, _)| *made > req.revision) {
            op = Op::transform(&op, step).map_err(refused)?.0;
        }
        let text = op.apply(&doc.text).map_err(refused)?;
        if u64::try_from(utf8_len(&text)).unwrap_or(u64::MAX) > MAX_FILE_BYTES {
            return Err(AppError::Invalid(format!(
                "files are limited to {MAX_FILE_BYTES} bytes in the editor"
            )));
        }
        let wire = op.to_wire();
        doc.step(&req.client, op, text);
        doc.touched = self.now().0;
        self.live_publish(&place, doc, &req.client, Some(wire));
        Ok(LiveApplied {
            revision: doc.revision,
        })
    }

    /// The document's write lock, so saves and takes of the disk run one at a time.
    fn live_writes(&self, place: &Place, session: &str) -> AppResult<Arc<tokio::sync::Mutex<()>>> {
        let docs = self.live_lock()?;
        let doc = docs.get(&place.key).filter(|doc| doc.session == session);
        Ok(doc.ok_or_else(ended)?.writes.clone())
    }

    /// The disk's `sha256` now (`""`: the file is gone).
    async fn live_disk_sha(&self, ctx: &AuthContext, sandbox: &str, place: &Place) -> AppResult<String> {
        match self
            .file_read(ctx, sandbox, Some(&place.root), &place.path, None)
            .await
        {
            Ok(file) => Ok(file.map(|file| file.sha256).unwrap_or_default()),
            Err(AppError::NotFound) => Ok(String::new()),
            Err(error) => Err(error),
        }
    }

    /// `POST /api/sandboxes/{sid}/files/live/save`: writes the shared text to the file with the
    /// caller's access. Saved text writes nothing; a disk that moved on is kept as a conflict
    /// unless `force` (Keep mine) writes over it.
    ///
    /// # Errors
    /// As [`App::file_write`]; `Conflict` for another or an ended document.
    pub async fn live_save(
        &self,
        ctx: &AuthContext,
        sandbox: &str,
        req: &LiveFileAction,
    ) -> AppResult<LiveState> {
        let place = self
            .live_place(ctx, sandbox, req.root.as_deref(), &req.path, true)
            .await?;
        let writes = self.live_writes(&place, &req.session)?;
        let _one = writes.lock().await;
        let (content, revision, base) = {
            let mut docs = self.live_lock()?;
            let doc = docs
                .get_mut(&place.key)
                .filter(|doc| doc.session == req.session)
                .ok_or_else(ended)?;
            doc.touched = self.now().0;
            let conflict = doc.disk_sha256.is_some();
            if (doc.saved() && !conflict) || (conflict && !req.force) {
                return Ok(doc.state());
            }
            let content = String::from_utf16(&doc.text).map_err(AppError::internal)?;
            (content, doc.revision, doc.sha256.clone())
        };
        let base = if req.force {
            Some(self.live_disk_sha(ctx, sandbox, &place).await?).filter(|sha| !sha.is_empty())
        } else {
            Some(base)
        };
        let written = self
            .file_write(
                ctx,
                sandbox,
                &WriteFile {
                    root: Some(place.root.clone()),
                    path: place.path.clone(),
                    content,
                    base_sha256: base,
                },
            )
            .await;
        let moved = match written {
            Ok(_) => None,
            Err(AppError::Conflict(_)) => Some(self.live_disk_sha(ctx, sandbox, &place).await?),
            Err(error) => return Err(error),
        };
        let mut docs = self.live_lock()?;
        let doc = docs
            .get_mut(&place.key)
            .filter(|doc| doc.session == req.session)
            .ok_or_else(ended)?;
        match (written, moved) {
            (Ok(file), _) => {
                doc.sha256 = file.sha256;
                doc.saved_revision = Some(revision);
                doc.disk_sha256 = None;
            }
            (_, moved) => doc.disk_sha256 = moved,
        }
        self.live_publish(&place, doc, "", None);
        Ok(doc.state())
    }

    /// `POST /api/sandboxes/{sid}/files/live/disk`: takes a newer version on disk (EDT-020): as
    /// one step when the text is saved (or with `force`, Take disk version, whatever it holds),
    /// else kept as a conflict.
    ///
    /// # Errors
    /// Access errors as reading (`force`: as changing) the file; `Invalid` when the disk's file
    /// is no longer text; `Conflict` for another or an ended document.
    pub async fn live_disk(
        &self,
        ctx: &AuthContext,
        sandbox: &str,
        req: &LiveFileAction,
    ) -> AppResult<LiveState> {
        let place = self
            .live_place(ctx, sandbox, req.root.as_deref(), &req.path, req.force)
            .await?;
        let writes = self.live_writes(&place, &req.session)?;
        let _one = writes.lock().await;
        let known = {
            let docs = self.live_lock()?;
            let doc = docs
                .get(&place.key)
                .filter(|doc| doc.session == req.session)
                .ok_or_else(ended)?;
            (!req.force).then(|| doc.sha256.clone())
        };
        let read = self
            .file_read(ctx, sandbox, Some(&place.root), &place.path, known.as_deref())
            .await;
        let disk = match read {
            Ok(disk) => disk,
            Err(AppError::NotFound) if !req.force => {
                let mut docs = self.live_lock()?;
                let doc = docs
                    .get_mut(&place.key)
                    .filter(|doc| doc.session == req.session)
                    .ok_or_else(ended)?;
                if doc.disk_sha256.as_deref() != Some("") {
                    doc.disk_sha256 = Some(String::new());
                    self.live_publish(&place, doc, "", None);
                }
                return Ok(doc.state());
            }
            Err(error) => return Err(error),
        };
        let mut docs = self.live_lock()?;
        let doc = docs
            .get_mut(&place.key)
            .filter(|doc| doc.session == req.session)
            .ok_or_else(ended)?;
        doc.touched = self.now().0;
        // Unchanged since the text's version.
        let Some(disk) = disk else {
            if doc.disk_sha256.take().is_some() {
                self.live_publish(&place, doc, "", None);
            }
            return Ok(doc.state());
        };
        let Some(content) = disk.content else {
            return Err(AppError::Invalid("the file on disk is no longer text the editor opens".into()));
        };
        if !(doc.saved() || req.force) {
            if doc.disk_sha256.as_deref() != Some(disk.sha256.as_str()) {
                doc.disk_sha256 = Some(disk.sha256);
                self.live_publish(&place, doc, "", None);
            }
            return Ok(doc.state());
        }
        let theirs: Vec<u16> = in_eol(normalized(&content, None).0.as_str(), &doc.eol)
            .encode_utf16()
            .collect();
        let op = replacement(&doc.text, &theirs);
        doc.sha256 = disk.sha256;
        doc.disk_sha256 = None;
        if op.is_noop() {
            doc.saved_revision = Some(doc.revision);
            self.live_publish(&place, doc, "", None);
        } else {
            let wire = op.to_wire();
            doc.step("", op, theirs);
            doc.saved_revision = Some(doc.revision);
            self.live_publish(&place, doc, "", Some(wire));
        }
        Ok(doc.state())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_endings_follow_most_of_the_text_as_monaco_takes_them() {
        assert_eq!(normalized("a\r\nb\r\nc\n", None), ("a\r\nb\r\nc\r\n".into(), "\r\n".into()));
        assert_eq!(normalized("\u{feff}a\nb\r\nc\n", None), ("a\nb\nc\n".into(), "\n".into()));
        assert_eq!(normalized("a\rb", None), ("a\r\nb".into(), "\r\n".into()));
        assert_eq!(normalized("abc", None).1, "\n");
        assert_eq!(normalized("a\r\nb", Some("\n")), ("a\nb".into(), "\n".into()));
    }

    #[test]
    fn a_replacement_changes_only_what_differs_and_never_cuts_a_character() -> Result<(), OpError> {
        let a: Vec<u16> = "x😀y".encode_utf16().collect();
        let b: Vec<u16> = "x😁y".encode_utf16().collect();
        let op = replacement(&a, &b);
        assert_eq!(op.apply(&a)?, b);
        assert_eq!(op.to_wire()[0], OpPart::Count(1));
        let same = replacement(&a, &a);
        assert!(same.is_noop());
        Ok(())
    }

    #[test]
    fn utf8_length_counts_pairs_as_four_bytes() {
        let text: Vec<u16> = "aé€😀".encode_utf16().collect();
        assert_eq!(utf8_len(&text), "aé€😀".len());
    }
}
