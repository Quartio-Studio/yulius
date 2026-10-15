//! Sessions as read-only documents (SPEC-001 SES-001..006, IMP-004, IMP-007; SPEC-002 §6.8).
//!
//! A sandbox's session library holds two kinds of documents: the Claude Code session of every
//! Yulius task (synchronized lazily from `provider_sessions`, read from the live transcript) and
//! the sessions people upload from their computers with `yulius onboard` (immutable copies under
//! `<data>/sandboxes/<sid>/sessions/<doc>/`, one per (sandbox, harness, session id), attributed
//! to the uploader, replaced when the same person uploads a changed transcript). People list,
//! search and read them on the Sessions page (read access); agents search and read their own
//! sandbox's sessions with `sessions_search`/`session_read`. Everything served is redacted
//! ([`crate::transcripts`]); the index is SQLite FTS5, filled incrementally and bounded.

use std::collections::{BTreeSet, HashMap};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde_json::{Value, json};
use yulius_contracts::api::common::UserRef;
use yulius_contracts::api::sessions::{
    SessionDoc, SessionHit, SessionList, SessionTask, SnippetPart, TranscriptBlock,
    TranscriptEntry, TranscriptPage,
};
use yulius_contracts::api::tasks::TaskSummary;
use yulius_contracts::import::SessionRef;
use yulius_domain::harness::Harness;
use yulius_domain::ids::{SandboxId, TaskId, UserId};
use yulius_domain::permission::{AccessLevel, Action};

use crate::auth::{AuthContext, rfc3339};
use crate::context::App;
use crate::coordination::ProjectCaller;
use crate::error::{AppError, AppResult};
use crate::imports::{aux_relative, copy_immutable, move_tree, parse_rfc3339};
use crate::lifecycle::NewTask;
use crate::model::{ProviderSessionRecord, SandboxRecord, TaskOrigin, TaskRecord, UserRecord};
use crate::ports::session_docs::{IndexAdvance, SessionDocRecord, SessionKind};
use crate::ports::store::TaskFieldUpdate;
use crate::projects::blocking;
use crate::supervisor::RunStart;
use crate::transcripts::{self, MAX_PAGE_CHARS};

/// Most imported sessions one sandbox holds (SPEC-002 §9).
pub const MAX_IMPORTED_SESSIONS: u64 = 10_000;
/// Most bytes of imported transcripts one sandbox holds (SPEC-002 §9).
pub const MAX_IMPORTED_BYTES: u64 = 8 * 1024 * 1024 * 1024;
/// Transcript bytes indexed per session; the rest is readable but not searchable.
pub const MAX_INDEXED_BYTES: u64 = 64 * 1024 * 1024;
/// Transcript bytes indexed per refresh (a search or a list), across sessions.
pub const INDEX_BYTES_PER_REFRESH: u64 = 32 * 1024 * 1024;
/// Most sessions in one search result.
pub const MAX_SEARCH_RESULTS: usize = 20;
/// Most matching passages shown per session.
pub const MAX_HITS_PER_SESSION: usize = 3;
/// Longest search text.
pub const MAX_QUERY_CHARS: usize = 200;
/// Most search terms used.
pub const MAX_QUERY_TERMS: usize = 12;
/// Most sessions in one list response.
pub const MAX_LIST: usize = 500;
/// Default and largest `session_read` text for agents.
pub const READ_CHARS_DEFAULT: usize = 24_000;
/// Largest `session_read` text for agents.
pub const READ_CHARS_MAX: usize = 100_000;
/// Snippet match markers produced by the store.
const MARK_OPEN: char = '\u{2}';
const MARK_CLOSE: char = '\u{3}';

/// What happened to one uploaded session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StoreOutcome {
    /// Stored for the first time.
    New,
    /// The uploader's earlier copy was replaced.
    Updated,
    /// Already held with this transcript.
    Unchanged,
    /// Another person's session with this id is kept.
    Taken,
}

/// List filters (`GET /api/sandboxes/{sid}/sessions`).
#[derive(Debug, Clone, Default)]
pub struct SessionFilter {
    /// `task` or `imported`; all when `None`.
    pub kind: Option<SessionKind>,
    /// Author login.
    pub author: Option<String>,
    /// Full-text search.
    pub query: Option<String>,
    /// Most sessions returned (≤ [`MAX_LIST`]).
    pub limit: Option<usize>,
}

/// Builds an FTS5 query from free text: every word becomes a quoted prefix term (a word made of
/// several tokens, `create_import`, a phrase); terms are joined with AND (`any = false`) or OR.
/// `None` when nothing searchable is left.
#[must_use]
pub fn fts_query(text: &str, any: bool) -> Option<String> {
    let mut terms = Vec::new();
    for word in text.split_whitespace() {
        let tokens: Vec<String> = word
            .split(|c: char| !c.is_alphanumeric())
            .filter(|t| !t.is_empty())
            .map(str::to_lowercase)
            .collect();
        if tokens.is_empty() {
            continue;
        }
        let term = format!("\"{}\"*", tokens.join(" "));
        if !terms.contains(&term) {
            terms.push(term);
        }
        if terms.len() >= MAX_QUERY_TERMS {
            break;
        }
    }
    if terms.is_empty() {
        return None;
    }
    Some(terms.join(if any { " OR " } else { " AND " }))
}

/// Splits a store snippet into marked parts.
#[must_use]
pub fn snippet_parts(snippet: &str) -> Vec<SnippetPart> {
    let mut parts = Vec::new();
    let mut current = String::new();
    let mut hit = false;
    for ch in snippet.chars() {
        let toggle = match ch {
            MARK_OPEN => Some(true),
            MARK_CLOSE => Some(false),
            _ => None,
        };
        match toggle {
            Some(next) => {
                if !current.is_empty() {
                    parts.push(SnippetPart {
                        text: std::mem::take(&mut current),
                        hit,
                    });
                }
                hit = next;
            }
            None => current.push(if ch.is_control() && ch != '\n' {
                ' '
            } else {
                ch
            }),
        }
    }
    if !current.is_empty() {
        parts.push(SnippetPart { text: current, hit });
    }
    parts
}

fn snippet_text(parts: &[SnippetPart]) -> String {
    parts
        .iter()
        .map(|part| {
            if part.hit {
                format!("«{}»", part.text)
            } else {
                part.text.clone()
            }
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn kind_arg(text: Option<&str>) -> AppResult<Option<SessionKind>> {
    match text.map(str::trim) {
        None | Some("" | "all") => Ok(None),
        Some(other) => SessionKind::parse(other)
            .map(Some)
            .ok_or_else(|| AppError::Invalid("kind must be all, task or imported".into())),
    }
}

/// Parses the `kind` query parameter of the list endpoint.
///
/// # Errors
/// `Invalid` for anything but `all`, `task` and `imported`.
pub fn parse_kind(text: Option<&str>) -> AppResult<Option<SessionKind>> {
    kind_arg(text)
}

fn session_task(task: &TaskRecord) -> SessionTask {
    SessionTask {
        id: task.id.to_string(),
        number: task.number,
        title: task.title.clone(),
        status: task.status.as_str().to_owned(),
    }
}

/// Sort key: when the session was last active.
fn activity(doc: &SessionDocRecord, tasks: &HashMap<TaskId, TaskRecord>) -> i64 {
    let task_time = doc
        .task_id
        .as_ref()
        .and_then(|id| tasks.get(id))
        .map_or(0, |task| task.updated_at.0);
    doc.ended_at
        .map_or(doc.updated_at.0, |t| t.0)
        .max(task_time)
}

impl App {
    /// Directory of a sandbox's imported sessions.
    pub(crate) fn sessions_dir(&self, sandbox: &SandboxId) -> PathBuf {
        self.sandbox_dir(sandbox).join("sessions")
    }

    /// Creates documents for task lineage entries that have none yet.
    async fn sync_task_docs(&self, sandbox: &SandboxId) -> AppResult<()> {
        let store = &self.deps.store;
        let missing = store.task_sessions_without_doc(sandbox).await?;
        if missing.is_empty() {
            return Ok(());
        }
        for entry in missing {
            let Some(task) = store.task(&entry.task_id).await? else {
                continue;
            };
            let path = self.task_transcript_path(&task, &entry);
            let now = self.now();
            let doc = SessionDocRecord {
                id: self.deps.crypto.new_id(),
                sandbox_id: sandbox.clone(),
                kind: SessionKind::Task,
                task_id: Some(task.id.clone()),
                harness: entry.harness,
                session_id: entry.session_id.clone(),
                title: task.title.clone(),
                author_id: Some(task.created_by.clone()),
                git_branch: task.branch.clone(),
                cwd_relative: None,
                base: None,
                transcript_path: path.to_string_lossy().into_owned(),
                sha256: None,
                bytes: 0,
                redactions: 0,
                started_at: Some(entry.first_seen),
                ended_at: Some(entry.last_seen),
                continued_task_id: None,
                indexed_bytes: 0,
                indexed_lines: 0,
                index_full: false,
                created_at: now,
                updated_at: now,
            };
            match store.insert_session_doc(&doc).await {
                Ok(()) | Err(AppError::Conflict(_)) => {}
                Err(err) => return Err(err),
            }
        }
        Ok(())
    }

    /// Where a task's session transcript lives: the harness's file under the sandbox home for
    /// the task worktree, or the stored copy of an imported lineage entry until the task has
    /// resumed it natively.
    fn task_transcript_path(&self, task: &TaskRecord, entry: &ProviderSessionRecord) -> PathBuf {
        let native = self.native_transcript(task, entry.harness, &entry.session_id);
        match &entry.transcript_ref {
            Some(stored) if !native.exists() => PathBuf::from(stored),
            _ => native,
        }
    }

    fn native_transcript(&self, task: &TaskRecord, harness: Harness, session_id: &str) -> PathBuf {
        let worktree = task.worktree_path.as_ref().map_or_else(
            || {
                self.sandbox_dir(&task.sandbox_id)
                    .join("worktrees")
                    .join(task.id.as_str())
            },
            PathBuf::from,
        );
        match harness {
            Harness::ClaudeCode => self.deps.harness.claude_transcript(
                &self.sandbox_home(&task.sandbox_id),
                &worktree,
                session_id,
            ),
        }
    }

    /// A task session read from a stored copy moves to the native transcript once the task
    /// resumed it (the copy stops growing; the native file has the continuation).
    /// A transcript rewritten in place (stepped back over a usage limit, SPEC-034) is indexed
    /// again from its start: the index reads transcripts as append-only.
    pub(crate) async fn reindex_session(&self, sandbox: &SandboxId, session_id: &str) {
        let store = &self.deps.store;
        let Ok(docs) = store.session_docs(sandbox).await else {
            return;
        };
        for doc in docs.iter().filter(|d| d.session_id == session_id) {
            if let Err(err) = store.reset_session_index(&doc.id).await {
                tracing::warn!(error = %err, "could not reset a session's index");
            }
        }
    }

    async fn follow_resumed_copies(&self, docs: &mut [SessionDocRecord]) -> AppResult<()> {
        let stored_root = self.config.data_dir.join("transcripts");
        for doc in docs.iter_mut() {
            if doc.kind != SessionKind::Task
                || !Path::new(&doc.transcript_path).starts_with(&stored_root)
            {
                continue;
            }
            let Some(task_id) = &doc.task_id else {
                continue;
            };
            let Some(task) = self.deps.store.task(task_id).await? else {
                continue;
            };
            let native = self.native_transcript(&task, doc.harness, &doc.session_id);
            if native.exists() {
                let path = native.to_string_lossy().into_owned();
                self.deps.store.set_session_doc_path(&doc.id, &path).await?;
                doc.transcript_path = path;
                doc.indexed_bytes = 0;
                doc.indexed_lines = 0;
            }
        }
        Ok(())
    }

    /// Brings the sandbox's index up to date within [`INDEX_BYTES_PER_REFRESH`]: new task
    /// sessions get documents; grown transcripts are indexed from where indexing stopped.
    ///
    /// # Errors
    /// Store failures (unreadable transcripts are skipped).
    pub async fn refresh_session_index(&self, sandbox: &SandboxId) -> AppResult<()> {
        self.sync_task_docs(sandbox).await?;
        let store = &self.deps.store;
        let mut docs: Vec<SessionDocRecord> = store
            .session_docs(sandbox)
            .await?
            .into_iter()
            .filter(|doc| !doc.index_full)
            .collect();
        self.follow_resumed_copies(&mut docs).await?;
        let paths: Vec<PathBuf> = docs
            .iter()
            .map(|doc| PathBuf::from(&doc.transcript_path))
            .collect();
        let sizes: Vec<Option<u64>> = blocking(move || {
            Ok(paths
                .iter()
                .map(|path| fs::metadata(path).ok().map(|meta| meta.len()))
                .collect())
        })
        .await?;
        let mut budget = INDEX_BYTES_PER_REFRESH;
        for (doc, size) in docs.into_iter().zip(sizes) {
            let Some(size) = size else { continue };
            if size == doc.indexed_bytes {
                continue;
            }
            if size < doc.indexed_bytes {
                store.reset_session_index(&doc.id).await?;
                continue;
            }
            if budget == 0 {
                break;
            }
            let allowed = MAX_INDEXED_BYTES
                .saturating_sub(doc.indexed_bytes)
                .min(budget);
            if allowed == 0 {
                // The per-session bound: the rest stays readable, not searchable.
                store
                    .append_session_chunks(
                        &doc.id,
                        &[],
                        IndexAdvance {
                            expected_bytes: doc.indexed_bytes,
                            bytes: doc.indexed_bytes,
                            lines: doc.indexed_lines,
                            full: true,
                            size,
                            at: self.now(),
                        },
                    )
                    .await?;
                continue;
            }
            let path = PathBuf::from(&doc.transcript_path);
            let (from_byte, from_line) = (doc.indexed_bytes, doc.indexed_lines);
            let batch = match blocking(move || {
                transcripts::read_for_index(&path, from_byte, from_line, allowed, usize::MAX)
                    .map_err(AppError::internal)
            })
            .await
            {
                Ok(batch) => batch,
                Err(err) => {
                    tracing::debug!(doc = %doc.id, error = %err, "transcript not indexed");
                    continue;
                }
            };
            let read = batch.end_byte.saturating_sub(from_byte);
            budget = budget.saturating_sub(read.max(1));
            if read == 0 && batch.chunks.is_empty() {
                continue;
            }
            let full = !batch.eof && batch.end_byte >= MAX_INDEXED_BYTES;
            store
                .append_session_chunks(
                    &doc.id,
                    &batch.chunks,
                    IndexAdvance {
                        expected_bytes: from_byte,
                        bytes: batch.end_byte,
                        lines: batch.end_line,
                        full,
                        size,
                        at: self.now(),
                    },
                )
                .await?;
        }
        Ok(())
    }

    /// Ranked matches in a sandbox's sessions, grouped per session (best first), excluding the
    /// sessions of `exclude` (the asking task).
    async fn search_docs(
        &self,
        sandbox: &SandboxId,
        query: &str,
        exclude: Option<&TaskId>,
    ) -> AppResult<Vec<(String, Vec<SessionHit>)>> {
        let query: String = query.chars().take(MAX_QUERY_CHARS).collect();
        let store = &self.deps.store;
        let mut hits = Vec::new();
        for any in [false, true] {
            let Some(fts) = fts_query(&query, any) else {
                return Ok(Vec::new());
            };
            hits = store.search_session_chunks(sandbox, &fts, 400).await?;
            if !hits.is_empty() || !fts.contains(" AND ") {
                break;
            }
        }
        let excluded: BTreeSet<String> = match exclude {
            Some(task) => store
                .session_docs(sandbox)
                .await?
                .into_iter()
                .filter(|doc| doc.task_id.as_ref() == Some(task))
                .map(|doc| doc.id)
                .collect(),
            None => BTreeSet::new(),
        };
        let mut grouped: Vec<(String, Vec<SessionHit>)> = Vec::new();
        for hit in hits {
            if excluded.contains(&hit.doc_id) {
                continue;
            }
            let entry = SessionHit {
                line: hit.line,
                role: hit.role,
                snippet: snippet_parts(&hit.snippet),
            };
            match grouped.iter_mut().find(|(id, _)| *id == hit.doc_id) {
                Some((_, list)) => {
                    if list.len() < MAX_HITS_PER_SESSION {
                        list.push(entry);
                    }
                }
                None => grouped.push((hit.doc_id, vec![entry])),
            }
        }
        Ok(grouped)
    }

    async fn sandbox_tasks(&self, sandbox: &SandboxId) -> AppResult<HashMap<TaskId, TaskRecord>> {
        let store = &self.deps.store;
        let mut tasks = store.tasks(sandbox, false).await?;
        tasks.extend(store.tasks(sandbox, true).await?);
        Ok(tasks.into_iter().map(|t| (t.id.clone(), t)).collect())
    }

    async fn doc_view(
        &self,
        doc: &SessionDocRecord,
        tasks: &HashMap<TaskId, TaskRecord>,
        logins: &mut HashMap<UserId, String>,
        viewer: Option<(&UserRecord, AccessLevel)>,
    ) -> SessionDoc {
        let task = doc.task_id.as_ref().and_then(|id| tasks.get(id));
        let author_id = match (doc.kind, task) {
            (SessionKind::Task, Some(task)) => Some(task.created_by.clone()),
            _ => doc.author_id.clone(),
        };
        let author = match author_id {
            Some(id) => {
                if !logins.contains_key(&id) {
                    let login = self.login_of(&id).await;
                    logins.insert(id.clone(), login);
                }
                logins.get(&id).map(|login| UserRef {
                    login: login.clone(),
                })
            }
            None => None,
        };
        let imported = doc.kind == SessionKind::Imported;
        let (can_continue, can_delete) = match viewer {
            Some((user, level)) => (
                imported && level.allows(Action::Work),
                imported
                    && (level.allows(Action::Administer)
                        || (doc.author_id.as_ref() == Some(&user.id)
                            && level.allows(Action::Work))),
            ),
            None => (false, false),
        };
        let ended = match task {
            Some(task) if doc.kind == SessionKind::Task => Some(
                doc.ended_at
                    .map_or(task.updated_at, |t| t.max(task.updated_at)),
            ),
            _ => doc.ended_at,
        };
        SessionDoc {
            id: doc.id.clone(),
            sandbox_id: doc.sandbox_id.to_string(),
            kind: doc.kind.as_str().to_owned(),
            title: task
                .filter(|_| doc.kind == SessionKind::Task)
                .map_or_else(|| doc.title.clone(), |t| t.title.clone()),
            harness: doc.harness.as_str().to_owned(),
            session_id: doc.session_id.clone(),
            author,
            task: task.map(session_task),
            continued_in: doc
                .continued_task_id
                .as_ref()
                .and_then(|id| tasks.get(id))
                .map(session_task),
            git_branch: doc.git_branch.clone(),
            started_at: doc.started_at.map(rfc3339),
            ended_at: ended.map(rfc3339),
            imported_at: imported.then(|| rfc3339(doc.created_at)),
            bytes: doc.bytes,
            redactions: doc.redactions,
            can_continue,
            can_delete,
            hits: Vec::new(),
        }
    }

    /// `GET /api/sandboxes/{sid}/sessions` (read): task and imported sessions, newest first,
    /// filtered by kind and author, or ranked by a full-text query.
    ///
    /// # Errors
    /// `NotFound` without access, `Invalid` filters.
    pub async fn list_sessions(
        &self,
        ctx: &AuthContext,
        sid: &str,
        filter: &SessionFilter,
    ) -> AppResult<SessionList> {
        let id = SandboxId::parse(sid).map_err(|_| AppError::NotFound)?;
        let (_, level) = self.require(&ctx.user, &id, Action::View).await?;
        self.refresh_session_index(&id).await?;
        let store = &self.deps.store;
        let docs = store.session_docs(&id).await?;
        let tasks = self.sandbox_tasks(&id).await?;
        let mut logins = HashMap::new();
        let mut views = Vec::with_capacity(docs.len());
        let mut by_id = HashMap::new();
        for doc in &docs {
            let view = self
                .doc_view(doc, &tasks, &mut logins, Some((&ctx.user, level)))
                .await;
            by_id.insert(doc.id.clone(), views.len());
            views.push((activity(doc, &tasks), view));
        }
        let mut authors: Vec<String> = views
            .iter()
            .filter_map(|(_, view)| view.author.as_ref().map(|a| a.login.clone()))
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        authors.sort_by_key(|login| login.to_lowercase());
        let wanted = |view: &SessionDoc| {
            filter.kind.is_none_or(|kind| view.kind == kind.as_str())
                && filter.author.as_deref().is_none_or(|author| {
                    view.author
                        .as_ref()
                        .is_some_and(|a| a.login.eq_ignore_ascii_case(author))
                })
        };
        let limit = filter.limit.unwrap_or(MAX_LIST).clamp(1, MAX_LIST);
        let query = filter
            .query
            .as_deref()
            .map(str::trim)
            .filter(|q| !q.is_empty());
        let sessions: Vec<SessionDoc> = match query {
            Some(query) => {
                let mut ranked = Vec::new();
                for (doc_id, hits) in self.search_docs(&id, query, None).await? {
                    if let Some(index) = by_id.get(&doc_id) {
                        let mut view = views[*index].1.clone();
                        if wanted(&view) {
                            view.hits = hits;
                            ranked.push(view);
                        }
                    }
                }
                ranked
            }
            None => {
                views.sort_by_key(|view| std::cmp::Reverse(view.0));
                views
                    .into_iter()
                    .map(|(_, view)| view)
                    .filter(|view| wanted(view))
                    .collect()
            }
        };
        let total = u32::try_from(sessions.len()).unwrap_or(u32::MAX);
        let mut sessions: Vec<SessionDoc> = sessions.into_iter().take(limit).collect();
        for view in &mut sessions {
            self.redact_view_secrets(view).await;
        }
        Ok(SessionList {
            sessions,
            authors,
            total,
            query: query.map(str::to_owned),
        })
    }

    async fn readable_doc(
        &self,
        ctx: &AuthContext,
        id: &str,
        action: Action,
    ) -> AppResult<(SessionDocRecord, AccessLevel)> {
        let doc = self
            .deps
            .store
            .session_doc(id)
            .await?
            .ok_or(AppError::NotFound)?;
        let (_, level) = self.require(&ctx.user, &doc.sandbox_id, action).await?;
        Ok((doc, level))
    }

    /// `GET /api/sessions/{id}` (read).
    ///
    /// # Errors
    /// `NotFound` for unknown sessions or without access.
    pub async fn session_detail(&self, ctx: &AuthContext, id: &str) -> AppResult<SessionDoc> {
        let (doc, level) = self.readable_doc(ctx, id, Action::View).await?;
        let tasks = self.sandbox_tasks(&doc.sandbox_id).await?;
        let mut view = self
            .doc_view(&doc, &tasks, &mut HashMap::new(), Some((&ctx.user, level)))
            .await;
        self.redact_view_secrets(&mut view).await;
        Ok(view)
    }

    /// `GET /api/sessions/{id}/transcript?from=&limit=` (read): a bounded, redacted page.
    ///
    /// # Errors
    /// `NotFound` without access or when the transcript is gone.
    pub async fn session_transcript(
        &self,
        ctx: &AuthContext,
        id: &str,
        from: u64,
        limit: usize,
    ) -> AppResult<TranscriptPage> {
        let (doc, level) = self.readable_doc(ctx, id, Action::View).await?;
        let tasks = self.sandbox_tasks(&doc.sandbox_id).await?;
        let session = self
            .doc_view(&doc, &tasks, &mut HashMap::new(), Some((&ctx.user, level)))
            .await;
        let path = PathBuf::from(&doc.transcript_path);
        let page =
            blocking(
                move || match transcripts::read_page(&path, from, limit, MAX_PAGE_CHARS) {
                    Ok(page) => Ok(Some(page)),
                    Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
                    Err(err) => Err(AppError::internal(err)),
                },
            )
            .await?
            .unwrap_or_default();
        let mut entries = page.entries;
        self.redact_page_secrets(&doc.sandbox_id, &mut entries)
            .await;
        let mut session = session;
        self.redact_view_secrets(&mut session).await;
        Ok(TranscriptPage {
            session,
            entries,
            next: page.next,
            skipped_lines: page.skipped,
        })
    }

    /// The sandbox's agent secrets (exact values and encodings, the vault's matcher) in text
    /// served from its sessions to people (SES-004, VLT): pages, titles and search passages.
    /// Agent reads (`sessions_search`, `session_read`) pass through the same redaction of every
    /// MCP result in `mcp_call`.
    async fn redact_agent_secrets(&self, sandbox: &SandboxId, text: String) -> String {
        if text.is_empty() {
            return text;
        }
        self.redact_for_sandbox(sandbox, &text).await
    }

    /// [`App::redact_agent_secrets`] on a session's title and search passages.
    async fn redact_view_secrets(&self, view: &mut SessionDoc) {
        let Ok(sandbox) = SandboxId::parse(&view.sandbox_id) else {
            return;
        };
        let title = std::mem::take(&mut view.title);
        view.title = self.redact_agent_secrets(&sandbox, title).await;
        for hit in &mut view.hits {
            for part in &mut hit.snippet {
                let text = std::mem::take(&mut part.text);
                part.text = self.redact_agent_secrets(&sandbox, text).await;
            }
        }
    }

    async fn redact_page_secrets(&self, sandbox: &SandboxId, entries: &mut [TranscriptEntry]) {
        for entry in entries {
            for block in &mut entry.blocks {
                for text in block_texts(block) {
                    let original = std::mem::take(text);
                    *text = self.redact_agent_secrets(sandbox, original).await;
                }
            }
        }
    }

    /// `DELETE /api/sessions/{id}`: removes an imported session (its uploader with write access,
    /// or an admin). Task sessions go with their task. Audited.
    ///
    /// # Errors
    /// `Forbidden` for others, `Invalid` for task sessions.
    pub async fn delete_session(&self, ctx: &AuthContext, id: &str) -> AppResult<()> {
        let (doc, level) = self.readable_doc(ctx, id, Action::View).await?;
        if doc.kind != SessionKind::Imported {
            return Err(AppError::Invalid(
                "a task's session is deleted with its task".into(),
            ));
        }
        let own = doc.author_id.as_ref() == Some(&ctx.user.id) && level.allows(Action::Work);
        if !(own || level.allows(Action::Administer)) {
            return Err(AppError::Forbidden(
                "only the person who uploaded a session or an admin can delete it".into(),
            ));
        }
        if !self.deps.store.delete_session_doc(&doc.id).await? {
            return Err(AppError::NotFound);
        }
        let dir = self.sessions_dir(&doc.sandbox_id).join(&doc.id);
        blocking(move || {
            match fs::remove_dir_all(&dir) {
                Ok(()) => {}
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
                Err(err) => return Err(AppError::internal(err)),
            }
            Ok(())
        })
        .await?;
        self.deps
            .store
            .audit(
                self.now(),
                &self.login_of(&ctx.user.id).await,
                "session.deleted",
                Some(&doc.id),
                Some(&format!("{} {}", doc.harness.as_str(), doc.session_id)),
            )
            .await?;
        Ok(())
    }

    /// `POST /api/sessions/{id}/continue` (write): continues an imported session natively in a
    /// new task (IMP-004). The task resumes the stored transcript in a worktree on the session's
    /// imported base; a session already continued returns that task. Refused while the sandbox
    /// is being onboarded (U-08).
    ///
    /// # Errors
    /// `Conflict` while onboarding or when the session id is already another task's session.
    pub async fn continue_session(
        self: &Arc<Self>,
        ctx: &AuthContext,
        id: &str,
    ) -> AppResult<TaskSummary> {
        let (doc, _) = self.readable_doc(ctx, id, Action::Work).await?;
        if doc.kind != SessionKind::Imported {
            return Err(AppError::Invalid(
                "a task's session is continued from its task".into(),
            ));
        }
        let store = &self.deps.store;
        if let Some(task_id) = &doc.continued_task_id
            && let Some(task) = store.task(task_id).await?
        {
            return self.task_summary(&task).await;
        }
        self.ensure_tasks_may_start(&doc.sandbox_id, TaskOrigin::Import)
            .await?;
        if store
            .provider_session_exists(&doc.sandbox_id, doc.harness.as_str(), &doc.session_id)
            .await?
        {
            return Err(AppError::Conflict(
                "this session already belongs to a task; open that task instead".into(),
            ));
        }
        let source = PathBuf::from(&doc.transcript_path);
        let staging = self
            .config
            .data_dir
            .join("transcripts")
            .join(".staging")
            .join(self.deps.crypto.new_id());
        let main_dest = staging.join(format!("{}.jsonl", doc.session_id));
        let aux_src = source.with_extension("");
        let aux_dest = staging.join(&doc.session_id);
        let stage_dir = staging.clone();
        blocking(move || {
            copy_immutable(&source, &main_dest)?;
            if aux_src.is_dir() {
                copy_dir(&aux_src, &aux_dest)?;
            }
            Ok(())
        })
        .await
        .inspect_err(|_| {
            let _ = fs::remove_dir_all(&stage_dir);
        })?;
        let login = self.login_of(&ctx.user.id).await;
        let task = self
            .create_task_internal(NewTask {
                sandbox: doc.sandbox_id.clone(),
                created_by: ctx.user.id.clone(),
                prompt: format!(
                    "{}\n\n(Continued from the {} session {} imported by {}.)",
                    doc.title,
                    doc.harness.as_str(),
                    doc.session_id,
                    match &doc.author_id {
                        Some(author) => self.login_of(author).await,
                        None => login.clone(),
                    }
                ),
                harness: doc.harness,
                title: Some(doc.title.clone()),
                workflow_id: None,
                steps: Vec::new(),
                model: None,
                effort: None,
                origin: TaskOrigin::Import,
                automation_run: None,
                start: false,
                mode: crate::model::TaskMode::Prompt,
                supertask: false,
            })
            .await
            .inspect_err(|_| {
                let _ = fs::remove_dir_all(&staging);
            })?;
        let final_dir = self
            .config
            .data_dir
            .join("transcripts")
            .join(task.id.as_str());
        let (from, to) = (staging.clone(), final_dir.clone());
        blocking(move || move_tree(&from, &to)).await?;
        let now = self.now();
        store
            .upsert_provider_session(&ProviderSessionRecord {
                id: self.deps.crypto.new_id(),
                sandbox_id: doc.sandbox_id.clone(),
                task_id: task.id.clone(),
                harness: doc.harness,
                session_id: doc.session_id.clone(),
                transcript_ref: Some(
                    final_dir
                        .join(format!("{}.jsonl", doc.session_id))
                        .to_string_lossy()
                        .into_owned(),
                ),
                origin: "imported".into(),
                first_seen: doc.started_at.unwrap_or(now),
                last_seen: doc.ended_at.unwrap_or(now),
            })
            .await?;
        store.set_session_continued(&doc.id, &task.id).await?;
        store
            .update_task(
                &task.id,
                &TaskFieldUpdate {
                    archived_at: Some(None),
                    ..TaskFieldUpdate::default()
                },
                now,
            )
            .await?;
        store
            .audit(
                now,
                &login,
                "session.continued",
                Some(&doc.id),
                Some(&format!("task #{}", task.number)),
            )
            .await?;
        if let Err(err) = self
            .start_run(&task.id, RunStart::Resume { prompt: None }, None)
            .await
        {
            tracing::warn!(task = %task.id, error = %err, "continued session did not start");
        }
        let task = store.task(&task.id).await?.ok_or(AppError::NotFound)?;
        self.task_summary(&task).await
    }

    /// The imported base of a task continued from a session document (IMP-004).
    pub(crate) async fn continued_session_base(
        &self,
        sandbox: &SandboxId,
        task: &TaskId,
    ) -> AppResult<Option<String>> {
        Ok(self
            .deps
            .store
            .session_docs(sandbox)
            .await?
            .into_iter()
            .find(|doc| doc.continued_task_id.as_ref() == Some(task))
            .and_then(|doc| doc.base))
    }

    /// Stores one uploaded session of an applied import as a document (SES-001): new, replaced
    /// (same uploader, changed transcript), unchanged, or kept as another person's.
    ///
    /// # Errors
    /// `Capacity` at the sandbox bounds, I/O and store failures.
    pub(crate) async fn store_imported_session(
        &self,
        sandbox: &SandboxRecord,
        session: &SessionRef,
        job_dir: &Path,
        user: &UserRecord,
        base: Option<String>,
    ) -> AppResult<StoreOutcome> {
        let store = &self.deps.store;
        let harness = match session.harness {
            yulius_contracts::import::ImportHarness::Claude => Harness::ClaudeCode,
        };
        let main = session
            .files
            .first()
            .ok_or_else(|| AppError::Invalid("a session without a transcript".into()))?;
        let existing = store
            .imported_session_doc(&sandbox.id, harness, &session.session_id)
            .await?;
        if existing.is_none()
            && store
                .provider_session_exists(&sandbox.id, harness.as_str(), &session.session_id)
                .await?
        {
            // Already a task's session (imported as a task by an earlier version, or continued).
            return Ok(StoreOutcome::Unchanged);
        }
        if let Some(doc) = &existing {
            if doc.sha256.as_deref() == Some(main.sha256.as_str()) {
                return Ok(StoreOutcome::Unchanged);
            }
            if doc.author_id.as_ref() != Some(&user.id) {
                return Ok(StoreOutcome::Taken);
            }
        } else {
            let (count, bytes) = store.imported_session_usage(&sandbox.id).await?;
            if count >= MAX_IMPORTED_SESSIONS
                || bytes.saturating_add(main.size) > MAX_IMPORTED_BYTES
            {
                return Err(AppError::Capacity(format!(
                    "this sandbox already holds {count} imported sessions ({} MiB); delete old ones on the Sessions page",
                    bytes / (1024 * 1024)
                )));
            }
        }
        let doc_id = existing
            .as_ref()
            .map_or_else(|| self.deps.crypto.new_id(), |doc| doc.id.clone());
        let dir = self.sessions_dir(&sandbox.id).join(&doc_id);
        let staging = self
            .sessions_dir(&sandbox.id)
            .join(".staging")
            .join(self.deps.crypto.new_id());
        let copies: Vec<(PathBuf, PathBuf)> = session
            .files
            .iter()
            .enumerate()
            .map(|(index, file)| {
                let dest = if index == 0 {
                    staging.join(format!("{}.jsonl", session.session_id))
                } else {
                    staging
                        .join(&session.session_id)
                        .join(aux_relative(&file.name, session))
                };
                (job_dir.join(&file.name), dest)
            })
            .collect();
        let (stage_dir, target) = (staging.clone(), dir.clone());
        blocking(move || {
            let staged = (|| {
                for (src, dest) in &copies {
                    copy_immutable(src, dest)?;
                }
                if target.exists() {
                    make_writable(&target);
                    fs::remove_dir_all(&target).map_err(AppError::internal)?;
                }
                move_tree(&stage_dir, &target)
            })();
            if staged.is_err() {
                make_writable(&stage_dir);
                let _ = fs::remove_dir_all(&stage_dir);
            }
            staged
        })
        .await?;
        let now = self.now();
        let started = parse_rfc3339(&session.started_at);
        let record = SessionDocRecord {
            id: doc_id,
            sandbox_id: sandbox.id.clone(),
            kind: SessionKind::Imported,
            task_id: None,
            harness,
            session_id: session.session_id.clone(),
            title: if session.title.trim().is_empty() {
                format!("Imported {} session", session.harness)
            } else {
                session.title.trim().to_owned()
            },
            author_id: Some(user.id.clone()),
            git_branch: session.git_branch.clone(),
            cwd_relative: session.cwd_relative.clone(),
            base,
            transcript_path: dir
                .join(format!("{}.jsonl", session.session_id))
                .to_string_lossy()
                .into_owned(),
            sha256: Some(main.sha256.clone()),
            bytes: main.size,
            redactions: session.redactions,
            started_at: started,
            ended_at: parse_rfc3339(&session.ended_at).or(started),
            continued_task_id: None,
            indexed_bytes: 0,
            indexed_lines: 0,
            index_full: false,
            created_at: existing.as_ref().map_or(now, |doc| doc.created_at),
            updated_at: now,
        };
        if existing.is_some() {
            store.replace_session_doc(&record).await?;
            Ok(StoreOutcome::Updated)
        } else {
            match store.insert_session_doc(&record).await {
                Ok(()) => Ok(StoreOutcome::New),
                Err(AppError::Conflict(_)) => Ok(StoreOutcome::Unchanged),
                Err(err) => Err(err),
            }
        }
    }

    /// Fingerprint states for the onboarding plan: `new`, `changed`, `unchanged` or `taken`.
    pub(crate) async fn session_plan_state(
        &self,
        sandbox: &SandboxId,
        user: &UserId,
        harness: Harness,
        session_id: &str,
        sha256: &str,
    ) -> AppResult<&'static str> {
        let store = &self.deps.store;
        Ok(
            match store
                .imported_session_doc(sandbox, harness, session_id)
                .await?
            {
                None if store
                    .provider_session_exists(sandbox, harness.as_str(), session_id)
                    .await? =>
                {
                    "unchanged"
                }
                None => "new",
                Some(doc) if doc.sha256.as_deref() == Some(sha256) => "unchanged",
                Some(doc) if doc.author_id.as_ref() != Some(user) => "taken",
                Some(_) => "changed",
            },
        )
    }

    /// MCP `sessions_search` (SES-005): full-text search over this sandbox's task and imported
    /// sessions, the asking task's own sessions excluded.
    ///
    /// # Errors
    /// `Invalid` without a query.
    pub async fn tool_sessions_search(
        &self,
        caller: &ProjectCaller,
        args: &Value,
    ) -> AppResult<Value> {
        let query = args
            .get("query")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|q| !q.is_empty())
            .ok_or_else(|| AppError::Invalid("missing string argument `query`".into()))?;
        let kind = kind_arg(args.get("kind").and_then(Value::as_str))?;
        let limit = args
            .get("limit")
            .and_then(Value::as_u64)
            .map_or(8, |n| usize::try_from(n).unwrap_or(MAX_SEARCH_RESULTS))
            .clamp(1, MAX_SEARCH_RESULTS);
        let sandbox = &caller.sandbox;
        self.refresh_session_index(sandbox).await?;
        let own = caller.task.as_ref().map(|t| &t.id);
        let found = self.search_docs(sandbox, query, own).await?;
        let tasks = self.sandbox_tasks(sandbox).await?;
        let mut logins = HashMap::new();
        let mut results = Vec::new();
        for (doc_id, hits) in found {
            let Some(doc) = self.deps.store.session_doc(&doc_id).await? else {
                continue;
            };
            if doc.sandbox_id != *sandbox || kind.is_some_and(|k| k != doc.kind) {
                continue;
            }
            let view = self.doc_view(&doc, &tasks, &mut logins, None).await;
            results.push(json!({
                "id": view.id,
                "kind": view.kind,
                "title": view.title,
                "task_number": view.task.as_ref().map(|t| t.number),
                "author": view.author.as_ref().map(|a| a.login.clone()),
                "branch": view.git_branch,
                "started_at": view.started_at,
                "ended_at": view.ended_at,
                "matches": hits.iter().map(|hit| json!({
                    "line": hit.line,
                    "role": hit.role,
                    "text": snippet_text(&hit.snippet),
                })).collect::<Vec<_>>(),
            }));
            if results.len() >= limit {
                break;
            }
        }
        Ok(json!({
            "query": query,
            "results": results,
            "hint": if results.is_empty() {
                "Nothing matched. Try fewer or different words (names of files, functions, errors or features)."
            } else {
                "Read around a match with session_read {id, from: <line - 5>}."
            },
        }))
    }

    /// MCP `session_read` (SES-005): a bounded part of one session of this sandbox, by document
    /// id or by task number (that task's latest session).
    ///
    /// # Errors
    /// `NotFound` outside the agent's sandbox, `Invalid` without an id or task number.
    pub async fn tool_session_read(
        &self,
        caller: &ProjectCaller,
        args: &Value,
    ) -> AppResult<Value> {
        let sandbox = &caller.sandbox;
        let store = &self.deps.store;
        let doc = if let Some(id) = args.get("id").and_then(Value::as_str) {
            store
                .session_doc(id)
                .await?
                .filter(|doc| doc.sandbox_id == *sandbox)
                .ok_or(AppError::NotFound)?
        } else if let Some(number) = args.get("task_number").and_then(Value::as_u64) {
            let number = u32::try_from(number).map_err(|_| AppError::NotFound)?;
            let task = store
                .task_by_number(sandbox, number)
                .await?
                .ok_or(AppError::NotFound)?;
            self.sync_task_docs(sandbox).await?;
            store
                .session_docs(sandbox)
                .await?
                .into_iter()
                .filter(|doc| doc.task_id.as_ref() == Some(&task.id))
                .max_by_key(|doc| (doc.started_at, doc.created_at))
                .ok_or_else(|| AppError::Invalid(format!("task #{number} has no session yet")))?
        } else {
            return Err(AppError::Invalid(
                "give the session `id` from sessions_search or a `task_number`".into(),
            ));
        };
        let from = args.get("from").and_then(Value::as_u64).unwrap_or(0);
        let limit = args
            .get("limit")
            .and_then(Value::as_u64)
            .map_or(60, |n| usize::try_from(n).unwrap_or(200))
            .clamp(1, 200);
        let max_chars = args
            .get("max_chars")
            .and_then(Value::as_u64)
            .map_or(READ_CHARS_DEFAULT, |n| {
                usize::try_from(n).unwrap_or(READ_CHARS_MAX)
            })
            .clamp(1_000, READ_CHARS_MAX);
        let path = PathBuf::from(&doc.transcript_path);
        let page = blocking(move || {
            transcripts::read_page(&path, from, limit, max_chars * 2).map_err(|err| {
                if err.kind() == std::io::ErrorKind::NotFound {
                    AppError::Invalid("this session's transcript is not on the server".into())
                } else {
                    AppError::internal(err)
                }
            })
        })
        .await?;
        let (text, cut_at) = transcripts::render_text(&page.entries, max_chars);
        let tasks = self.sandbox_tasks(sandbox).await?;
        let view = self.doc_view(&doc, &tasks, &mut HashMap::new(), None).await;
        Ok(json!({
            "id": view.id,
            "kind": view.kind,
            "title": view.title,
            "task_number": view.task.as_ref().map(|t| t.number),
            "author": view.author.as_ref().map(|a| a.login.clone()),
            "from": from,
            "next_from": cut_at.or(page.next),
            "text": if text.is_empty() { "(nothing to show in this range)".to_owned() } else { text },
        }))
    }
}

/// Every text field of a transcript block.
fn block_texts(block: &mut TranscriptBlock) -> Vec<&mut String> {
    match block {
        TranscriptBlock::Text { text, .. }
        | TranscriptBlock::Thinking { text, .. }
        | TranscriptBlock::Meta { text, .. }
        | TranscriptBlock::ToolResult { text, .. } => vec![text],
        TranscriptBlock::ToolCall {
            summary,
            input,
            diff,
            ..
        } => {
            let mut texts = vec![summary, input];
            if let Some(diff) = diff {
                for hunk in &mut diff.hunks {
                    texts.push(&mut hunk.old);
                    texts.push(&mut hunk.new);
                }
            }
            texts
        }
        TranscriptBlock::Image { .. } => Vec::new(),
    }
}

/// Copies a directory tree (regular files only, links skipped).
fn copy_dir(src: &Path, dest: &Path) -> AppResult<()> {
    fs::create_dir_all(dest).map_err(AppError::internal)?;
    for entry in fs::read_dir(src).map_err(AppError::internal)? {
        let entry = entry.map_err(AppError::internal)?;
        let kind = entry.file_type().map_err(AppError::internal)?;
        let target = dest.join(entry.file_name());
        if kind.is_dir() {
            copy_dir(&entry.path(), &target)?;
        } else if kind.is_file() {
            copy_immutable(&entry.path(), &target)?;
        }
    }
    Ok(())
}

/// Read-only copies (0444) must become writable again before their directory is replaced.
fn make_writable(dir: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if entry.file_type().is_ok_and(|kind| kind.is_dir()) {
            make_writable(&path);
        } else {
            let _ = fs::set_permissions(&path, fs::Permissions::from_mode(0o644));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn queries_become_quoted_prefix_terms() {
        assert_eq!(
            fts_query("checkout stripe", false).as_deref(),
            Some("\"checkout\"* AND \"stripe\"*")
        );
        assert_eq!(
            fts_query("create_import \"OR\" x*", true).as_deref(),
            Some("\"create import\"* OR \"or\"* OR \"x\"*")
        );
        assert_eq!(fts_query("  ()* -- ", false), None);
        let many = (0..30)
            .map(|i| format!("w{i}"))
            .collect::<Vec<_>>()
            .join(" ");
        assert_eq!(
            fts_query(&many, false).map(|q| q.matches(" AND ").count()),
            Some(MAX_QUERY_TERMS - 1)
        );
    }

    #[test]
    fn snippets_split_into_marked_parts() {
        let parts = snippet_parts("we \u{2}migrate\u{3} the \u{2}checkout\u{3}…");
        assert_eq!(
            parts,
            vec![
                SnippetPart {
                    text: "we ".into(),
                    hit: false
                },
                SnippetPart {
                    text: "migrate".into(),
                    hit: true
                },
                SnippetPart {
                    text: " the ".into(),
                    hit: false
                },
                SnippetPart {
                    text: "checkout".into(),
                    hit: true
                },
                SnippetPart {
                    text: "…".into(),
                    hit: false
                },
            ]
        );
        assert_eq!(snippet_text(&parts), "we «migrate» the «checkout»…");
    }
}
