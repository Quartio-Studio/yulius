//! Runs recorded before SPEC-087 get their metrics from their transcripts, once (SPEC-091
//! RCT-003, RCT-004): a store upgraded from schema version 22 with the wrong figures of old runs,
//! the real harness adapter and transcripts on disk in the sandbox's home.

use std::path::{Path, PathBuf};

use crate::support::{self, HarnessOptions, TestResult};
use yulius_app::model::MetricsRecord;
use yulius_app::ports::store::TaskStore;
use yulius_domain::ids::TaskId;
use yulius_store::SqliteStore;

const SESSION: &str = "5d2a8c41-3e6f-4b7a-9c0d-1e2f3a4b5c6d";
const WORKTREE: &str = "/work/demo";
/// The worktree of a task whose transcript cannot be read until its project directory is fixed.
const BROKEN: &str = "/work/broken";

fn ms(rfc3339: &str) -> Result<i64, Box<dyn std::error::Error>> {
    Ok(yulius_harness::time::parse_rfc3339(rfc3339)
        .ok_or("time")?
        .0)
}

fn fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../yulius-harness/tests/fixtures/claude")
}

/// A store as SPEC-087's version left it (schema 22): two runs of a resumed session with the
/// old watcher's figures (the first without active time and ended late, by a restart, after the
/// second had started; the second without a row), a later run of that session with no line in its
/// transcript, a run whose transcript is gone, two whose session ids are not ones (a relative and
/// an absolute path), a live run, and a run whose transcript cannot be read yet.
fn old_store(db: &Path, outside: &str) -> TestResult {
    yulius_store::create_at_version(db, 22)?;
    let conn = rusqlite::Connection::open(db)?;
    let (r1, e1) = (ms("2026-10-10T09:00:00Z")?, ms("2026-10-10T09:30:00Z")?);
    let (r2, e2) = (ms("2026-10-10T10:00:00Z")?, ms("2026-10-10T10:05:00Z")?);
    let late = ms("2026-10-10T10:30:00Z")?;
    let (r3, e3) = (ms("2026-10-10T12:00:00Z")?, ms("2026-10-10T12:10:00Z")?);
    conn.execute_batch(&format!(
        "INSERT INTO users (id, display_name, is_superadmin, is_instance_admin, created_at) VALUES ('u', 'U', 0, 0, 1);
         INSERT INTO sandboxes (id, slug, repo_owner, repo_name, default_branch, profile, port_base, port_count, created_by, onboarding_state, created_at) VALUES ('s1', 's', 'o', 'r', 'main', '{{}}', 20000, 10, 'u', 'not_needed', 1);
         INSERT INTO tasks (id, sandbox_id, number, title, prompt, harness, status, origin, created_by, owner_id, created_at, updated_at, worktree_path) VALUES
           ('t1', 's1', 1, 'T', 'p', 'claude', 'closed', 'ui', 'u', 'u', 1, 1, '{WORKTREE}'),
           ('t2', 's1', 2, 'T', 'p', 'claude', 'running', 'ui', 'u', 'u', 2, 2, '/work/other'),
           ('t3', 's1', 3, 'T', 'p', 'claude', 'closed', 'ui', 'u', 'u', 3, 3, '{BROKEN}');
         INSERT INTO attempts (id, task_id, seq, harness, provider_session_id, launch_kind, state, run_dir, token_hash, host_token, started_at, ended_at) VALUES
           ('a1', 't1', 1, 'claude', '{SESSION}', 'new', 'exited', '/r/a1', 'h1', x'00', {r1}, {late}),
           ('a2', 't1', 2, 'claude', '{SESSION}', 'resume', 'exited', '/r/a2', 'h2', x'00', {r2}, {e2}),
           ('a3', 't2', 1, 'claude', 'gone-session', 'new', 'exited', '/r/a3', 'h3', x'00', {r1}, {e1}),
           ('a4', 't2', 2, 'claude', 'live-session', 'new', 'live', '/r/a4', 'h4', x'00', {r2}, NULL),
           ('a5', 't1', 3, 'claude', '{SESSION}', 'resume', 'exited', '/r/a5', 'h5', x'00', {r3}, {e3}),
           ('a6', 't3', 1, 'claude', '{SESSION}', 'new', 'exited', '/r/a6', 'h6', x'00', {r1}, {e1}),
           ('a7', 't2', 3, 'claude', '../../escape', 'new', 'exited', '/r/a7', 'h7', x'00', {r1}, {e1}),
           ('a8', 't2', 4, 'claude', '{outside}', 'new', 'exited', '/r/a8', 'h8', x'00', {r1}, {e1});
         INSERT INTO metrics (attempt_id, task_id, model, input_tokens, output_tokens, cache_read, cache_write, turns, active_ms, updated_at) VALUES
           ('a1', 't1', 'claude-opus-5-5', 45, 310, 0, 0, 7, NULL, {e1}),
           ('a3', 't2', 'claude-opus-5-5', 1, 77, 0, 0, 2, 1000, {e1});"
    ))?;
    Ok(())
}

/// An absolute "session id" naming a transcript outside the sandbox.
fn outside(data: &Path) -> PathBuf {
    data.join("outside")
}

fn transcript_of(h: &support::Harness, data: &Path, worktree: &str) -> PathBuf {
    let home = data.join("sandboxes").join("s1").join("home");
    h.app
        .deps
        .harness
        .claude_transcript(&home, Path::new(worktree), SESSION)
}

/// The transcript and its sub-agents where the harness adapter finds the session of `t1`; the
/// project directory of `t3` is a file, so reading its transcript fails.
fn transcripts(h: &support::Harness, data: &Path) -> TestResult {
    let broken = transcript_of(h, data, BROKEN);
    let project = broken.parent().ok_or("project dir")?;
    std::fs::create_dir_all(project.parent().ok_or("projects")?)?;
    std::fs::write(project, "not a directory")?;
    let transcript = transcript_of(h, data, WORKTREE);
    // Where the session id "../../escape" of `a7` would lead: never read.
    let home = data.join("sandboxes").join("s1").join("home");
    let escaped =
        h.app
            .deps
            .harness
            .claude_transcript(&home, Path::new("/work/other"), "../../escape");
    std::fs::create_dir_all(escaped.parent().ok_or("parent")?)?;
    std::fs::copy(fixture().join("resumed.jsonl"), &escaped)?;
    // Where the absolute session id of `a8` would lead (`join` takes it whole): never read.
    std::fs::copy(
        fixture().join("resumed.jsonl"),
        outside(data).with_extension("jsonl"),
    )?;
    let sub = transcript.with_extension("").join("subagents");
    std::fs::create_dir_all(&sub)?;
    std::fs::copy(fixture().join("resumed.jsonl"), &transcript)?;
    for agent in ["agent-a7.jsonl", "agent-b2.jsonl"] {
        std::fs::copy(
            fixture().join("resumed/subagents").join(agent),
            sub.join(agent),
        )?;
    }
    Ok(())
}

type Recount = (String, Option<String>, Option<String>, Option<i64>);

fn recounts(db: &Path) -> Result<Vec<Recount>, Box<dyn std::error::Error>> {
    let conn = rusqlite::Connection::open(db)?;
    let mut stmt = conn.prepare(
        "SELECT attempt_id, outcome, before, done_at FROM metrics_recounts ORDER BY attempt_id",
    )?;
    let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?;
    Ok(rows.collect::<Result<_, _>>()?)
}

fn stored(db: &Path, attempt: &str) -> Result<(MetricsRecord, i64), Box<dyn std::error::Error>> {
    let conn = rusqlite::Connection::open(db)?;
    Ok(conn.query_row(
        "SELECT model, input_tokens, output_tokens, cache_read, cache_write, turns, active_ms, updated_at FROM metrics WHERE attempt_id = ?1",
        [attempt],
        |r| {
            let n = |i: usize| r.get::<_, Option<i64>>(i).map(|v| v.and_then(|v| u64::try_from(v).ok()));
            Ok((
                MetricsRecord {
                    model: r.get(0)?,
                    input_tokens: n(1)?,
                    output_tokens: n(2)?,
                    cache_read: n(3)?,
                    cache_write: n(4)?,
                    turns: r.get::<_, Option<u32>>(5)?,
                    active_ms: n(6)?,
                },
                r.get(7)?,
            ))
        },
    )?)
}

#[tokio::test]
async fn old_runs_get_their_transcripts_figures_once() -> TestResult {
    let dir = tempfile::tempdir()?;
    let data = dir.path().canonicalize()?;
    let db = data.join("yulius.db");
    old_store(&db, &outside(&data).to_string_lossy())?;
    // The upgrade queues the eight runs recorded before it.
    let store = SqliteStore::open(&db)?;
    let h = support::harness_with(HarnessOptions {
        dir: Some(dir),
        store: Some(store),
        ..HarnessOptions::default()
    })?;
    transcripts(&h, &data)?;

    let tally = h.app.recount_old_metrics().await?;
    assert_eq!(
        (
            tally.recomputed,
            tally.no_transcript,
            tally.no_lines,
            tally.live,
            tally.failed
        ),
        (2, 3, 1, 1, 1)
    );
    let (first, saved_at) = stored(&db, "a1")?;
    assert_eq!(
        first,
        MetricsRecord {
            model: Some("claude-opus-5-5".into()),
            input_tokens: Some(85),
            output_tokens: Some(1_510),
            cache_read: Some(2_000 + 2_300 + 2_400 + 2_500 + 2_600 + 2_700 + 2_800 + 2_900 + 2_100),
            cache_write: Some(300 + 40 + 20 + 10 + 30 + 5 + 5 + 5 + 100),
            turns: Some(3),
            active_ms: Some(220_200),
        },
        "its own lines and its background agent's, up to the next run's start: 7 answers were 3 \
         turns, and its active time was lost"
    );
    let (second, _) = stored(&db, "a2")?;
    assert_eq!(
        (second.output_tokens, second.turns, second.active_ms),
        (Some(400), Some(1), Some(39_100)),
        "the resumed run had no row; the terminal's turn after it is not its"
    );
    let (gone, _) = stored(&db, "a3")?;
    assert_eq!(
        (gone.output_tokens, gone.turns, gone.active_ms),
        (Some(77), Some(2), Some(1_000)),
        "a run whose transcript is gone keeps its figures"
    );
    let rows = recounts(&db)?;
    let outcomes: Vec<_> = rows
        .iter()
        .map(|(id, outcome, ..)| (id.as_str(), outcome.as_deref()))
        .collect();
    assert_eq!(
        outcomes,
        [
            ("a1", Some("recomputed")),
            ("a2", Some("recomputed")),
            ("a3", Some("no_transcript")),
            ("a4", Some("live")),
            ("a5", Some("no_lines")),
            ("a6", None),
            ("a7", Some("no_transcript")),
            ("a8", Some("no_transcript")),
        ]
    );
    assert!(
        rows[0]
            .2
            .as_deref()
            .is_some_and(|b| b.contains("\"turns\":7")),
        "the figures it replaced are recorded: {:?}",
        rows[0].2
    );
    assert!(
        rows.iter()
            .all(|(id, outcome, .., at)| at.is_some() == outcome.is_some()
                && (id != "a6" || at.is_none())),
        "a run that failed stays queued: {rows:?}"
    );
    assert!(stored(&db, "a5").is_err(), "nothing replaces nothing");
    let audit: i64 = rusqlite::Connection::open(&db)?.query_row(
        "SELECT COUNT(*) FROM audit_log WHERE action = 'metrics.recounted'",
        [],
        |r| r.get(0),
    )?;
    assert_eq!(audit, 1);
    assert_eq!(h.store.task_metrics(&TaskId::parse("t1")?).await?.len(), 2);

    // The next start: the run that failed is counted once its transcript can be read; nothing
    // else is counted again or changes.
    let broken = transcript_of(&h, &data, BROKEN);
    let project = broken.parent().ok_or("project dir")?;
    std::fs::remove_file(project)?;
    std::fs::create_dir_all(project)?;
    std::fs::copy(fixture().join("resumed.jsonl"), &broken)?;
    h.clock
        .0
        .fetch_add(60_000, std::sync::atomic::Ordering::SeqCst);
    let again = h.app.recount_old_metrics().await?;
    assert_eq!(
        again,
        yulius_app::metrics_recount::RecountTally {
            recomputed: 1,
            ..Default::default()
        }
    );
    let (retried, _) = stored(&db, "a6")?;
    assert_eq!(
        (retried.output_tokens, retried.turns),
        (Some(310), Some(3)),
        "its session's lines to its end, no sub-agent transcript beside it"
    );
    assert_eq!(stored(&db, "a1")?, (first, saved_at));
    let after = recounts(&db)?;
    assert_eq!(after[..5], rows[..5]);
    assert_eq!(after[6..], rows[6..]);
    assert_eq!(after[5].1.as_deref(), Some("recomputed"));

    // Once: a third round recounts nothing.
    assert!(h.app.recount_old_metrics().await?.is_empty());
    assert_eq!(recounts(&db)?, after);
    Ok(())
}
