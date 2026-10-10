//! Session documents and their full-text index (SPEC-001 SES-001..006; migration 0007).

use async_trait::async_trait;
use rusqlite::{OptionalExtension, Row, params};
use yulius_app::error::AppResult;
use yulius_app::model::ProviderSessionRecord;
use yulius_app::ports::session_docs::{
    IndexAdvance, SessionChunk, SessionDocRecord, SessionDocStore, SessionKind, SessionSearchHit,
};
use yulius_domain::harness::Harness;
use yulius_domain::ids::{SandboxId, TaskId, UserId};
use yulius_domain::time::Timestamp;

use crate::{SqliteStore, bad_column, db, to_i64, to_u32, to_u64};

const COLS: &str = "id, sandbox_id, kind, task_id, harness, session_id, title, author_id, \
    git_branch, cwd_relative, base, transcript_path, sha256, bytes, redactions, started_at, \
    ended_at, continued_task_id, indexed_bytes, indexed_lines, index_full, created_at, updated_at";

fn parse<T>(
    column: &str,
    text: &str,
    f: fn(&str) -> Result<T, yulius_domain::ids::IdError>,
) -> rusqlite::Result<T> {
    f(text).map_err(|_| bad_column(column, text))
}

fn from_row(row: &Row<'_>) -> rusqlite::Result<SessionDocRecord> {
    let sandbox: String = row.get("sandbox_id")?;
    let kind: String = row.get("kind")?;
    let task: Option<String> = row.get("task_id")?;
    let harness: String = row.get("harness")?;
    let author: Option<String> = row.get("author_id")?;
    let continued: Option<String> = row.get("continued_task_id")?;
    Ok(SessionDocRecord {
        id: row.get("id")?,
        sandbox_id: parse("sandbox id", &sandbox, SandboxId::parse)?,
        kind: SessionKind::parse(&kind).ok_or_else(|| bad_column("kind", &kind))?,
        task_id: task
            .as_deref()
            .map(|t| parse("task id", t, TaskId::parse))
            .transpose()?,
        harness: Harness::parse(&harness).map_err(|_| bad_column("harness", &harness))?,
        session_id: row.get("session_id")?,
        title: row.get("title")?,
        author_id: author
            .as_deref()
            .map(|u| parse("user id", u, UserId::parse))
            .transpose()?,
        git_branch: row.get("git_branch")?,
        cwd_relative: row.get("cwd_relative")?,
        base: row.get("base")?,
        transcript_path: row.get("transcript_path")?,
        sha256: row.get("sha256")?,
        bytes: to_u64(row.get("bytes")?),
        redactions: to_u32(row.get("redactions")?),
        started_at: row.get::<_, Option<i64>>("started_at")?.map(Timestamp),
        ended_at: row.get::<_, Option<i64>>("ended_at")?.map(Timestamp),
        continued_task_id: continued
            .as_deref()
            .map(|t| parse("task id", t, TaskId::parse))
            .transpose()?,
        indexed_bytes: to_u64(row.get("indexed_bytes")?),
        indexed_lines: to_u64(row.get("indexed_lines")?),
        index_full: row.get::<_, i64>("index_full")? != 0,
        created_at: Timestamp(row.get("created_at")?),
        updated_at: Timestamp(row.get("updated_at")?),
    })
}

#[async_trait]
impl SessionDocStore for SqliteStore {
    async fn insert_session_doc(&self, doc: &SessionDocRecord) -> AppResult<()> {
        let d = doc.clone();
        self.call(move |c| {
            c.execute(
                &format!(
                    "INSERT INTO session_docs ({COLS}) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, \
                     ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23)"
                ),
                params![
                    d.id,
                    d.sandbox_id.as_str(),
                    d.kind.as_str(),
                    d.task_id.as_ref().map(TaskId::as_str),
                    d.harness.as_str(),
                    d.session_id,
                    d.title,
                    d.author_id.as_ref().map(UserId::as_str),
                    d.git_branch,
                    d.cwd_relative,
                    d.base,
                    d.transcript_path,
                    d.sha256,
                    to_i64(d.bytes),
                    i64::from(d.redactions),
                    d.started_at.map(|t| t.0),
                    d.ended_at.map(|t| t.0),
                    d.continued_task_id.as_ref().map(TaskId::as_str),
                    to_i64(d.indexed_bytes),
                    to_i64(d.indexed_lines),
                    i64::from(d.index_full),
                    d.created_at.0,
                    d.updated_at.0,
                ],
            )
            .map_err(db)?;
            Ok(())
        })
        .await
    }

    async fn replace_session_doc(&self, doc: &SessionDocRecord) -> AppResult<bool> {
        let d = doc.clone();
        self.call(move |c| {
            let tx = c.transaction().map_err(db)?;
            tx.execute(
                "DELETE FROM session_chunks WHERE doc_id = ?1",
                [d.id.as_str()],
            )
            .map_err(db)?;
            let changed = tx
                .execute(
                    "UPDATE session_docs SET title = ?1, author_id = ?2, git_branch = ?3, \
                     cwd_relative = ?4, base = ?5, transcript_path = ?6, sha256 = ?7, bytes = ?8, \
                     redactions = ?9, started_at = ?10, ended_at = ?11, indexed_bytes = 0, \
                     indexed_lines = 0, index_full = 0, updated_at = ?12 WHERE id = ?13",
                    params![
                        d.title,
                        d.author_id.as_ref().map(UserId::as_str),
                        d.git_branch,
                        d.cwd_relative,
                        d.base,
                        d.transcript_path,
                        d.sha256,
                        to_i64(d.bytes),
                        i64::from(d.redactions),
                        d.started_at.map(|t| t.0),
                        d.ended_at.map(|t| t.0),
                        d.updated_at.0,
                        d.id,
                    ],
                )
                .map_err(db)?;
            tx.commit().map_err(db)?;
            Ok(changed > 0)
        })
        .await
    }

    async fn session_doc(&self, id: &str) -> AppResult<Option<SessionDocRecord>> {
        let id = id.to_owned();
        self.call(move |c| {
            c.query_row(
                &format!("SELECT {COLS} FROM session_docs WHERE id = ?1"),
                [id],
                from_row,
            )
            .optional()
            .map_err(db)
        })
        .await
    }

    async fn imported_session_doc(
        &self,
        sandbox: &SandboxId,
        harness: Harness,
        session_id: &str,
    ) -> AppResult<Option<SessionDocRecord>> {
        let (sandbox, session) = (sandbox.as_str().to_owned(), session_id.to_owned());
        self.call(move |c| {
            c.query_row(
                &format!(
                    "SELECT {COLS} FROM session_docs WHERE sandbox_id = ?1 AND kind = 'imported' \
                     AND harness = ?2 AND session_id = ?3"
                ),
                params![sandbox, harness.as_str(), session],
                from_row,
            )
            .optional()
            .map_err(db)
        })
        .await
    }

    async fn session_docs(&self, sandbox: &SandboxId) -> AppResult<Vec<SessionDocRecord>> {
        let sandbox = sandbox.as_str().to_owned();
        self.call(move |c| {
            let mut stmt = c
                .prepare(&format!(
                    "SELECT {COLS} FROM session_docs WHERE sandbox_id = ?1 \
                     ORDER BY COALESCE(ended_at, updated_at) DESC, id"
                ))
                .map_err(db)?;
            let rows = stmt.query_map([sandbox], from_row).map_err(db)?;
            rows.collect::<Result<Vec<_>, _>>().map_err(db)
        })
        .await
    }

    async fn task_sessions_without_doc(
        &self,
        sandbox: &SandboxId,
    ) -> AppResult<Vec<ProviderSessionRecord>> {
        let sandbox = sandbox.as_str().to_owned();
        self.call(move |c| {
            let mut stmt = c
                .prepare(
                    "SELECT p.id, p.sandbox_id, p.task_id, p.harness, p.session_id, \
                     p.transcript_ref, p.origin, p.first_seen, p.last_seen \
                     FROM provider_sessions p WHERE p.sandbox_id = ?1 AND NOT EXISTS ( \
                       SELECT 1 FROM session_docs d WHERE d.kind = 'task' \
                       AND d.task_id = p.task_id AND d.harness = p.harness \
                       AND d.session_id = p.session_id) \
                     ORDER BY p.first_seen",
                )
                .map_err(db)?;
            let rows = stmt
                .query_map([sandbox], |r| {
                    let sb: String = r.get(1)?;
                    let tk: String = r.get(2)?;
                    let h: String = r.get(3)?;
                    Ok(ProviderSessionRecord {
                        id: r.get(0)?,
                        sandbox_id: parse("sandbox id", &sb, SandboxId::parse)?,
                        task_id: parse("task id", &tk, TaskId::parse)?,
                        harness: Harness::parse(&h).map_err(|_| bad_column("harness", &h))?,
                        session_id: r.get(4)?,
                        transcript_ref: r.get(5)?,
                        origin: r.get(6)?,
                        first_seen: Timestamp(r.get(7)?),
                        last_seen: Timestamp(r.get(8)?),
                    })
                })
                .map_err(db)?;
            rows.collect::<Result<Vec<_>, _>>().map_err(db)
        })
        .await
    }

    async fn delete_session_doc(&self, id: &str) -> AppResult<bool> {
        let id = id.to_owned();
        self.call(move |c| {
            let tx = c.transaction().map_err(db)?;
            tx.execute(
                "DELETE FROM session_chunks WHERE doc_id = ?1",
                [id.as_str()],
            )
            .map_err(db)?;
            let deleted = tx
                .execute("DELETE FROM session_docs WHERE id = ?1", [id.as_str()])
                .map_err(db)?;
            tx.commit().map_err(db)?;
            Ok(deleted > 0)
        })
        .await
    }

    async fn set_session_doc_path(&self, id: &str, path: &str) -> AppResult<()> {
        let (id, path) = (id.to_owned(), path.to_owned());
        self.call(move |c| {
            let tx = c.transaction().map_err(db)?;
            tx.execute(
                "DELETE FROM session_chunks WHERE doc_id = ?1",
                [id.as_str()],
            )
            .map_err(db)?;
            tx.execute(
                "UPDATE session_docs SET transcript_path = ?1, indexed_bytes = 0, \
                 indexed_lines = 0, index_full = 0 WHERE id = ?2",
                params![path, id],
            )
            .map_err(db)?;
            tx.commit().map_err(db)?;
            Ok(())
        })
        .await
    }

    async fn set_session_continued(&self, id: &str, task: &TaskId) -> AppResult<()> {
        let (id, task) = (id.to_owned(), task.as_str().to_owned());
        self.call(move |c| {
            c.execute(
                "UPDATE session_docs SET continued_task_id = ?1 WHERE id = ?2",
                params![task, id],
            )
            .map_err(db)?;
            Ok(())
        })
        .await
    }

    async fn append_session_chunks(
        &self,
        doc_id: &str,
        chunks: &[SessionChunk],
        advance: IndexAdvance,
    ) -> AppResult<bool> {
        let (doc, chunks) = (doc_id.to_owned(), chunks.to_vec());
        self.call(move |c| {
            let tx = c.transaction().map_err(db)?;
            let moved = tx
                .execute(
                    "UPDATE session_docs SET indexed_bytes = ?1, indexed_lines = ?2, \
                     index_full = ?3, bytes = MAX(bytes, ?4), updated_at = MAX(updated_at, ?5) \
                     WHERE id = ?6 AND indexed_bytes = ?7",
                    params![
                        to_i64(advance.bytes),
                        to_i64(advance.lines),
                        i64::from(advance.full),
                        to_i64(advance.size),
                        advance.at.0,
                        doc,
                        to_i64(advance.expected_bytes),
                    ],
                )
                .map_err(db)?;
            if moved == 0 {
                return Ok(false);
            }
            {
                let mut insert = tx
                    .prepare(
                        "INSERT INTO session_chunks (doc_id, line, role, text) \
                         VALUES (?1, ?2, ?3, ?4)",
                    )
                    .map_err(db)?;
                for chunk in &chunks {
                    insert
                        .execute(params![doc, to_i64(chunk.line), chunk.role, chunk.text])
                        .map_err(db)?;
                }
            }
            tx.commit().map_err(db)?;
            Ok(true)
        })
        .await
    }

    async fn reset_session_index(&self, doc_id: &str) -> AppResult<()> {
        let doc = doc_id.to_owned();
        self.call(move |c| {
            let tx = c.transaction().map_err(db)?;
            tx.execute(
                "DELETE FROM session_chunks WHERE doc_id = ?1",
                [doc.as_str()],
            )
            .map_err(db)?;
            tx.execute(
                "UPDATE session_docs SET indexed_bytes = 0, indexed_lines = 0, index_full = 0 \
                 WHERE id = ?1",
                [doc.as_str()],
            )
            .map_err(db)?;
            tx.commit().map_err(db)?;
            Ok(())
        })
        .await
    }

    async fn search_session_chunks(
        &self,
        sandbox: &SandboxId,
        fts_query: &str,
        limit: u32,
    ) -> AppResult<Vec<SessionSearchHit>> {
        let (sandbox, query) = (sandbox.as_str().to_owned(), fts_query.to_owned());
        self.call(move |c| {
            let mut stmt = c
                .prepare(
                    "SELECT k.doc_id, k.line, k.role, \
                     snippet(session_fts, 0, char(2), char(3), '…', 24), bm25(session_fts) \
                     FROM session_fts \
                     JOIN session_chunks k ON k.id = session_fts.rowid \
                     JOIN session_docs d ON d.id = k.doc_id \
                     WHERE session_fts MATCH ?1 AND d.sandbox_id = ?2 \
                     ORDER BY bm25(session_fts) LIMIT ?3",
                )
                .map_err(db)?;
            let rows = stmt
                .query_map(params![query, sandbox, i64::from(limit)], |r| {
                    Ok(SessionSearchHit {
                        doc_id: r.get(0)?,
                        line: to_u64(r.get(1)?),
                        role: r.get(2)?,
                        snippet: r.get(3)?,
                        rank: r.get(4)?,
                    })
                })
                .map_err(db)?;
            rows.collect::<Result<Vec<_>, _>>().map_err(db)
        })
        .await
    }

    async fn imported_session_usage(&self, sandbox: &SandboxId) -> AppResult<(u64, u64)> {
        let sandbox = sandbox.as_str().to_owned();
        self.call(move |c| {
            c.query_row(
                "SELECT COUNT(*), COALESCE(SUM(bytes), 0) FROM session_docs \
                 WHERE sandbox_id = ?1 AND kind = 'imported'",
                [sandbox],
                |r| Ok((to_u64(r.get(0)?), to_u64(r.get(1)?))),
            )
            .map_err(db)
        })
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use yulius_app::ports::store::{SandboxStore as _, TaskStore as _, UserStore as _};

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    async fn seeded() -> Result<SqliteStore, Box<dyn std::error::Error>> {
        let store = SqliteStore::open_in_memory()?;
        store
            .call(|c| {
                c.execute_batch(
                    "INSERT INTO users (id, display_name, is_superadmin, is_instance_admin, created_at) VALUES ('u1', 'U', 1, 1, 1);
                     INSERT INTO sandboxes (id, slug, repo_owner, repo_name, default_branch, profile, port_base, port_count, created_by, onboarding_state, created_at) VALUES ('s1', 'a', 'o', 'a', 'main', '{}', 20000, 10, 'u1', 'not_needed', 1);
                     INSERT INTO sandboxes (id, slug, repo_owner, repo_name, default_branch, profile, port_base, port_count, created_by, onboarding_state, created_at) VALUES ('s2', 'b', 'o', 'b', 'main', '{}', 20010, 10, 'u1', 'not_needed', 1);
                     INSERT INTO tasks (id, sandbox_id, number, title, prompt, harness, status, origin, created_by, created_at, updated_at) VALUES ('t1', 's1', 1, 'T', 'p', 'claude', 'closed', 'ui', 'u1', 1, 1);
                     INSERT INTO provider_sessions (id, sandbox_id, task_id, harness, session_id, transcript_ref, origin, first_seen, last_seen) VALUES ('p1', 's1', 't1', 'claude', 'sess-task', NULL, 'native', 1, 1);",
                )
                .map_err(db)
            })
            .await?;
        Ok(store)
    }

    fn doc(
        id: &str,
        sandbox: &str,
        kind: SessionKind,
        session: &str,
    ) -> Result<SessionDocRecord, Box<dyn std::error::Error>> {
        Ok(SessionDocRecord {
            id: id.to_owned(),
            sandbox_id: SandboxId::parse(sandbox)?,
            kind,
            task_id: if kind == SessionKind::Task {
                Some(TaskId::parse("t1")?)
            } else {
                None
            },
            harness: Harness::ClaudeCode,
            session_id: session.to_owned(),
            title: "Fix the login".to_owned(),
            author_id: UserId::parse("u1").ok(),
            git_branch: Some("main".to_owned()),
            cwd_relative: None,
            base: None,
            transcript_path: "/data/x.jsonl".to_owned(),
            sha256: Some("a".repeat(64)),
            bytes: 10,
            redactions: 1,
            started_at: Some(Timestamp(5)),
            ended_at: Some(Timestamp(6)),
            continued_task_id: None,
            indexed_bytes: 0,
            indexed_lines: 0,
            index_full: false,
            created_at: Timestamp(7),
            updated_at: Timestamp(7),
        })
    }

    fn advance(expected: u64, bytes: u64) -> IndexAdvance {
        IndexAdvance {
            expected_bytes: expected,
            bytes,
            lines: 2,
            full: false,
            size: bytes,
            at: Timestamp(9),
        }
    }

    fn chunk(line: u64, text: &str) -> SessionChunk {
        SessionChunk {
            line,
            role: "assistant".to_owned(),
            text: text.to_owned(),
        }
    }

    #[tokio::test]
    async fn documents_are_unique_per_sandbox_session_and_searchable_only_there() -> TestResult {
        let store = seeded().await?;
        let s1 = SandboxId::parse("s1")?;
        let s2 = SandboxId::parse("s2")?;
        store
            .insert_session_doc(&doc("d1", "s1", SessionKind::Imported, "sess-1")?)
            .await?;
        assert!(
            store
                .insert_session_doc(&doc("d1b", "s1", SessionKind::Imported, "sess-1")?)
                .await
                .is_err(),
            "one imported document per session and sandbox"
        );
        store
            .insert_session_doc(&doc("d2", "s2", SessionKind::Imported, "sess-1")?)
            .await?;
        assert_eq!(
            store.task_sessions_without_doc(&s1).await?.len(),
            1,
            "the task's lineage entry has no document yet"
        );
        store
            .insert_session_doc(&doc("d3", "s1", SessionKind::Task, "sess-task")?)
            .await?;
        assert!(store.task_sessions_without_doc(&s1).await?.is_empty());

        assert!(
            store
                .append_session_chunks(
                    "d1",
                    &[
                        chunk(0, "We migrate the checkout to Stripe"),
                        chunk(3, "unrelated")
                    ],
                    advance(0, 100),
                )
                .await?
        );
        assert!(
            !store
                .append_session_chunks("d1", &[chunk(4, "dup")], advance(0, 120))
                .await?,
            "a stale position writes nothing"
        );
        store
            .append_session_chunks(
                "d2",
                &[chunk(0, "checkout in the other sandbox")],
                advance(0, 50),
            )
            .await?;
        let hits = store
            .search_session_chunks(&s1, "\"checkout\"*", 10)
            .await?;
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].doc_id, "d1");
        assert_eq!(hits[0].line, 0);
        assert!(
            hits[0].snippet.contains("\u{2}checkout\u{3}"),
            "{}",
            hits[0].snippet
        );
        let reloaded = store.session_doc("d1").await?.ok_or("doc")?;
        assert_eq!((reloaded.indexed_bytes, reloaded.indexed_lines), (100, 2));
        assert_eq!(store.imported_session_usage(&s1).await?, (1, 100));

        // Replacing clears the index; deleting removes the passages from the index too.
        let mut newer = doc("d1", "s1", SessionKind::Imported, "sess-1")?;
        newer.sha256 = Some("b".repeat(64));
        assert!(store.replace_session_doc(&newer).await?);
        assert!(
            store
                .search_session_chunks(&s1, "\"checkout\"*", 10)
                .await?
                .is_empty()
        );
        store
            .append_session_chunks("d1", &[chunk(0, "checkout again")], advance(0, 10))
            .await?;
        assert!(store.delete_session_doc("d1").await?);
        assert!(
            store
                .search_session_chunks(&s1, "\"checkout\"*", 10)
                .await?
                .is_empty()
        );
        assert_eq!(
            store
                .search_session_chunks(&s2, "\"checkout\"*", 10)
                .await?
                .len(),
            1
        );
        Ok(())
    }

    #[tokio::test]
    async fn deleting_a_task_removes_its_session_documents_and_frees_continuations() -> TestResult {
        let store = seeded().await?;
        let s1 = SandboxId::parse("s1")?;
        let task = TaskId::parse("t1")?;
        store
            .insert_session_doc(&doc("dt", "s1", SessionKind::Task, "sess-task")?)
            .await?;
        store
            .append_session_chunks("dt", &[chunk(1, "kafka consumer")], advance(0, 10))
            .await?;
        store
            .insert_session_doc(&doc("di", "s1", SessionKind::Imported, "sess-imp")?)
            .await?;
        store.set_session_continued("di", &task).await?;
        assert_eq!(
            store
                .session_doc("di")
                .await?
                .ok_or("doc")?
                .continued_task_id,
            Some(task.clone())
        );
        assert!(store.user(&UserId::parse("u1")?).await?.is_some());
        assert!(store.sandbox(&s1).await?.is_some());
        assert!(store.delete_task(&task).await?);
        assert!(store.session_doc("dt").await?.is_none());
        assert!(
            store
                .search_session_chunks(&s1, "\"kafka\"*", 10)
                .await?
                .is_empty()
        );
        assert_eq!(
            store
                .session_doc("di")
                .await?
                .ok_or("doc")?
                .continued_task_id,
            None
        );
        Ok(())
    }
}
