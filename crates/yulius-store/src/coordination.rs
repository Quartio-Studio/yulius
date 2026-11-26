//! Steps, messages, scope claims and record numbers.

use async_trait::async_trait;
use rusqlite::{OptionalExtension, Row, params};
use yulius_app::error::{AppError, AppResult};
use yulius_app::model::{
    MessageRecord, RecordReservation, ReviewRoundRecord, ScopeClaimRecord, StepRecord, StepTiming,
};
use yulius_app::ports::store::CoordinationStore;
use yulius_app::ports::store::ReviewStart;
use yulius_domain::ids::{MessageId, SandboxId, StepId, TaskId, UserId};
use yulius_domain::time::Timestamp;

use crate::{SqliteStore, bad_column, to_i64, to_u32, to_u64};

fn step_from_row(row: &Row<'_>) -> rusqlite::Result<StepRecord> {
    let id: String = row.get("id")?;
    let task: String = row.get("task_id")?;
    Ok(StepRecord {
        id: StepId::parse(&id).map_err(|_| bad_column("step id", &id))?,
        task_id: TaskId::parse(&task).map_err(|_| bad_column("task id", &task))?,
        ordinal: to_u32(row.get("ordinal")?),
        title: row.get("title")?,
        status: row.get("status")?,
        note: row.get("note")?,
        updated_by: row.get("updated_by")?,
        updated_at: row.get::<_, Option<i64>>("updated_at")?.map(Timestamp),
        effort: u8::try_from(row.get::<_, i64>("effort")?.clamp(1, 3)).unwrap_or(1),
    })
}

const STEP_COLS: &str = "id, task_id, ordinal, title, status, note, updated_by, updated_at, effort";

fn opt_task(text: Option<String>) -> rusqlite::Result<Option<TaskId>> {
    text.as_deref()
        .map(|t| TaskId::parse(t).map_err(|_| bad_column("task id", t)))
        .transpose()
}

fn message_from_row(row: &Row<'_>) -> rusqlite::Result<MessageRecord> {
    let id: String = row.get("id")?;
    let sandbox: String = row.get("sandbox_id")?;
    let from_user: Option<String> = row.get("from_user")?;
    Ok(MessageRecord {
        id: MessageId::parse(&id).map_err(|_| bad_column("message id", &id))?,
        sandbox_id: SandboxId::parse(&sandbox).map_err(|_| bad_column("sandbox id", &sandbox))?,
        from_task: opt_task(row.get("from_task")?)?,
        from_user: from_user
            .as_deref()
            .map(|u| UserId::parse(u).map_err(|_| bad_column("user id", u)))
            .transpose()?,
        to_task: opt_task(row.get("to_task")?)?,
        kind: row.get("kind")?,
        subject: row.get("subject")?,
        body: row.get("body")?,
        created_at: Timestamp(row.get("created_at")?),
    })
}

/// Completing a previously open step proves progress in a running goal's continuation.
fn reset_goal_attempts_on_progress(
    tx: &rusqlite::Transaction<'_>,
    task: &str,
    previous: Option<&str>,
    next: &str,
) -> rusqlite::Result<()> {
    if matches!(next, "done" | "skipped")
        && matches!(previous, Some("pending" | "in_progress" | "blocked"))
    {
        tx.execute(
            "UPDATE tasks SET gate_blocks = 0 WHERE id = ?1 AND mode = 'goal' AND gate_blocks != 0",
            [task],
        )?;
    }
    Ok(())
}

/// Keeps how many steps `task`'s plan has now as its first size, once a step of it is finished
/// and no first size is kept yet (SPEC-484 PLN-009): in the transaction of the step write, so
/// the number is the plan as that write left it.
fn keep_first_plan_size(tx: &rusqlite::Transaction<'_>, task: &str) -> rusqlite::Result<usize> {
    tx.execute(
        "UPDATE tasks SET plan_first_steps = (SELECT COUNT(*) FROM steps WHERE task_id = ?1) \
         WHERE id = ?1 AND plan_first_steps IS NULL \
         AND EXISTS (SELECT 1 FROM steps WHERE task_id = ?1 AND status IN ('done', 'skipped'))",
        [task],
    )
}

const MSG_COLS: &str = "m.id, m.sandbox_id, m.from_task, m.from_user, m.to_task, m.kind, m.subject, m.body, m.created_at";

/// The most review rounds one analytics read takes: the newest merges first (a range of years
/// of a busy project). SPEC-493 REV-006.
const MAX_REVIEW_ROUNDS_READ: u32 = 20_000;
const REVIEW_ROUND_COLS: &str = "id, task_id, round, head, agent, model, reason, subagent_id, \
    model_answered, started_at, ended_at, verdict, cut_off, pr_number, merged_at";

fn review_round_from_row(row: &Row<'_>) -> rusqlite::Result<ReviewRoundRecord> {
    let task: String = row.get("task_id")?;
    Ok(ReviewRoundRecord {
        id: row.get("id")?,
        task_id: TaskId::parse(&task).map_err(|_| bad_column("task id", &task))?,
        round: to_u32(row.get("round")?),
        head: row.get("head")?,
        agent: row.get("agent")?,
        model: row.get("model")?,
        reason: row.get("reason")?,
        subagent_id: row.get("subagent_id")?,
        model_answered: row.get("model_answered")?,
        started_at: Timestamp(row.get("started_at")?),
        ended_at: row.get::<_, Option<i64>>("ended_at")?.map(Timestamp),
        verdict: row.get("verdict")?,
        cut_off: row.get("cut_off")?,
        pr_number: row.get::<_, Option<i64>>("pr_number")?.map(to_u64),
        merged_at: row.get::<_, Option<i64>>("merged_at")?.map(Timestamp),
    })
}

/// With just one round the reviewer ID suffices (including legacy undated records).
/// After a continuation, the transcript time identifies the run. Merged history is frozen.
fn round_for_observation(
    c: &rusqlite::Connection,
    task: &str,
    subagent: &str,
    at: Option<Timestamp>,
) -> rusqlite::Result<Option<(String, String, Option<i64>)>> {
    c.query_row(
        "SELECT id, model, ended_at FROM review_rounds \
         WHERE task_id = ?1 AND subagent_id = ?2 AND merged_at IS NULL \
         AND ((started_at <= ?3 AND (ended_at IS NULL OR ended_at >= ?3)) \
              OR NOT EXISTS (SELECT 1 FROM review_rounds WHERE task_id = ?1 \
                  AND subagent_id = ?2 AND merged_at IS NULL LIMIT 1 OFFSET 1)) \
         ORDER BY started_at DESC, rowid DESC LIMIT 1",
        params![task, subagent, at.map(|t| t.0)],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    )
    .optional()
}

#[async_trait]
impl CoordinationStore for SqliteStore {
    async fn replace_steps(
        &self,
        task: &TaskId,
        steps: &[StepRecord],
        actor: &str,
        at: Timestamp,
    ) -> AppResult<()> {
        let (task, steps, actor) = (task.as_str().to_owned(), steps.to_vec(), actor.to_owned());
        self.call(move |c| {
            let tx = c.transaction()?;
            let mut old = std::collections::BTreeMap::new();
            {
                let mut stmt = tx.prepare("SELECT id, status FROM steps WHERE task_id = ?1")?;
                let rows = stmt.query_map([&task], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
                for row in rows {
                    let (id, status) = row?;
                    old.insert(id, status);
                }
            }
            tx.execute("DELETE FROM steps WHERE task_id = ?1", [&task])?;
            for s in &steps {
                tx.execute(
                    &format!("INSERT INTO steps ({STEP_COLS}) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)"),
                    params![s.id.as_str(), task, i64::from(s.ordinal), s.title, s.status, s.note, s.updated_by, s.updated_at.map(|t| t.0), i64::from(s.effort.clamp(1, 3))],
                )?;
                let previous = old.get(s.id.as_str());
                if previous.map(String::as_str) != Some(s.status.as_str()) {
                    reset_goal_attempts_on_progress(&tx, &task, previous.map(String::as_str), &s.status)?;
                    tx.execute(
                        "INSERT INTO step_events (step_id, task_id, at, actor, from_status, to_status, note) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                        params![s.id.as_str(), task, at.0, actor, previous, s.status, s.note],
                    )?;
                }
            }
            keep_first_plan_size(&tx, &task)?;
            Ok(tx.commit()?)
        })
        .await
    }

    async fn plan_first_steps(&self, task: &TaskId) -> AppResult<Option<u32>> {
        let t = task.as_str().to_owned();
        self.call(move |c| {
            let first: Option<Option<i64>> = c
                .query_row(
                    "SELECT plan_first_steps FROM tasks WHERE id = ?1",
                    [t],
                    |r| r.get(0),
                )
                .optional()?;
            Ok(first.flatten().and_then(|n| u32::try_from(n).ok()))
        })
        .await
    }

    async fn set_spec_status(&self, task: &TaskId, status: Option<&str>) -> AppResult<bool> {
        let (t, status) = (task.as_str().to_owned(), status.map(str::to_owned));
        self.call(move |c| {
            let changed = c.execute(
                "UPDATE tasks SET spec_status = ?2 WHERE id = ?1 AND spec_status IS NOT ?2",
                params![t, status],
            )?;
            Ok(changed > 0)
        })
        .await
    }

    async fn spec_status(&self, task: &TaskId) -> AppResult<Option<String>> {
        let t = task.as_str().to_owned();
        self.call(move |c| {
            let status: Option<Option<String>> = c
                .query_row("SELECT spec_status FROM tasks WHERE id = ?1", [t], |r| {
                    r.get(0)
                })
                .optional()?;
            Ok(status.flatten())
        })
        .await
    }

    async fn steps(&self, task: &TaskId) -> AppResult<Vec<StepRecord>> {
        let t = task.as_str().to_owned();
        self.call(move |c| {
            let mut stmt = c.prepare(&format!(
                "SELECT {STEP_COLS} FROM steps WHERE task_id = ?1 ORDER BY ordinal"
            ))?;
            let rows = stmt.query_map([t], step_from_row)?;
            Ok(rows.collect::<Result<Vec<_>, _>>()?)
        })
        .await
    }

    async fn step_timings(&self, task: &TaskId) -> AppResult<Vec<StepTiming>> {
        let t = task.as_str().to_owned();
        self.call(move |c| {
            let mut stmt = c.prepare(
                "SELECT step_id, \
                     MAX(CASE WHEN to_status = 'in_progress' THEN at END), \
                     MAX(CASE WHEN to_status IN ('done', 'skipped') THEN at END) \
                     FROM step_events WHERE task_id = ?1 GROUP BY step_id",
            )?;
            let rows = stmt.query_map([t], |r| {
                let id: String = r.get(0)?;
                Ok(StepTiming {
                    step_id: StepId::parse(&id).map_err(|_| bad_column("step id", &id))?,
                    started_at: r.get::<_, Option<i64>>(1)?.map(Timestamp),
                    finished_at: r.get::<_, Option<i64>>(2)?.map(Timestamp),
                })
            })?;
            Ok(rows.collect::<Result<Vec<_>, _>>()?)
        })
        .await
    }

    async fn update_step(
        &self,
        step: &StepId,
        status: &str,
        note: Option<&str>,
        actor: &str,
        at: Timestamp,
    ) -> AppResult<StepRecord> {
        let (id, status, note, actor) = (
            step.as_str().to_owned(),
            status.to_owned(),
            note.map(str::to_owned),
            actor.to_owned(),
        );
        self.call(move |c| {
            let tx = c.transaction()?;
            let current = tx
                .query_row(&format!("SELECT {STEP_COLS} FROM steps WHERE id = ?1"), [&id], step_from_row)
                .optional()?
                .ok_or(AppError::NotFound)?;
            tx.execute(
                "UPDATE steps SET status = ?1, note = COALESCE(?2, note), updated_by = ?3, updated_at = ?4 WHERE id = ?5",
                params![status, note, actor, at.0, id],
            )?;
            tx.execute(
                "INSERT INTO step_events (step_id, task_id, at, actor, from_status, to_status, note) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![id, current.task_id.as_str(), at.0, actor, current.status, status, note],
            )?;
            reset_goal_attempts_on_progress(&tx, current.task_id.as_str(), Some(&current.status), &status)?;
            let updated = tx.query_row(&format!("SELECT {STEP_COLS} FROM steps WHERE id = ?1"), [&id], step_from_row)?;
            keep_first_plan_size(&tx, current.task_id.as_str())?;
            tx.commit()?;
            Ok(updated)
        })
        .await
    }

    async fn insert_message(&self, m: &MessageRecord, recipients: &[TaskId]) -> AppResult<()> {
        let (m, recipients) = (m.clone(), recipients.to_vec());
        self.call(move |c| {
            let tx = c.transaction()?;
            tx.execute(
                "INSERT INTO messages (id, sandbox_id, from_task, from_user, to_task, kind, subject, body, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                params![
                    m.id.as_str(),
                    m.sandbox_id.as_str(),
                    m.from_task.as_ref().map(TaskId::as_str),
                    m.from_user.as_ref().map(UserId::as_str),
                    m.to_task.as_ref().map(TaskId::as_str),
                    m.kind,
                    m.subject,
                    m.body,
                    m.created_at.0
                ],
            )?;
            for r in &recipients {
                tx.execute("INSERT OR IGNORE INTO message_receipts (message_id, task_id) VALUES (?1, ?2)", params![m.id.as_str(), r.as_str()])?;
            }
            Ok(tx.commit()?)
        })
        .await
    }

    async fn task_messages(&self, task: &TaskId, limit: u32) -> AppResult<Vec<MessageRecord>> {
        let t = task.as_str().to_owned();
        self.call(move |c| {
            let mut stmt = c
                .prepare(&format!(
                    "SELECT {MSG_COLS} FROM messages m \
                     LEFT JOIN message_receipts r ON r.message_id = m.id AND r.task_id = ?1 \
                     WHERE r.task_id IS NOT NULL OR m.from_task = ?1 ORDER BY m.created_at DESC LIMIT ?2"
                ))?;
            let rows = stmt.query_map(params![t, i64::from(limit)], message_from_row)?;
            let mut out = rows.collect::<Result<Vec<_>, _>>()?;
            out.reverse();
            Ok(out)
        })
        .await
    }

    async fn insert_review_round(&self, round: &ReviewRoundRecord) -> AppResult<()> {
        let r = round.clone();
        self.call(move |c| {
            c.execute(
                "INSERT INTO review_rounds (id, task_id, round, head, agent, model, reason, \
                 subagent_id, model_answered, started_at, ended_at, verdict, cut_off, pr_number, \
                 merged_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)",
                params![
                    r.id,
                    r.task_id.as_str(),
                    i64::from(r.round),
                    r.head,
                    r.agent,
                    r.model,
                    r.reason,
                    r.subagent_id,
                    r.model_answered,
                    r.started_at.0,
                    r.ended_at.map(|t| t.0),
                    r.verdict,
                    r.cut_off,
                    r.pr_number.map(to_i64),
                    r.merged_at.map(|t| t.0),
                ],
            )?;
            Ok(())
        })
        .await
    }

    async fn record_codex_review_round(
        &self,
        task: &TaskId,
        subagent: &str,
        model: &str,
        head: Option<&str>,
        started_at: Timestamp,
        report: Option<(Timestamp, yulius_domain::review::Verdict)>,
    ) -> AppResult<()> {
        // Codex's child ID is stable across duplicate starts, stops, and native resume.
        let id = format!("codex:{task}:{subagent}");
        let (task, subagent, model, head) = (
            task.to_string(),
            subagent.to_owned(),
            model.to_owned(),
            head.map(str::to_owned),
        );
        let ended_at = report.map(|(at, _)| at.0);
        let verdict = report.map(|(_, verdict)| verdict.as_str());
        self.call(move |c| {
            c.execute(
                "INSERT INTO review_rounds (id, task_id, round, head, agent, model, reason, \
                 subagent_id, started_at, ended_at, verdict, model_answered) \
                 SELECT ?1, ?2, COALESCE((SELECT MAX(round) \
                 FROM review_rounds WHERE task_id = ?2 AND merged_at IS NULL \
                 AND ended_at <= ?6), 0) + 1, ?3, 'codex-reviewer', ?4, 'round', \
                 ?5, ?6, ?7, ?8, CASE WHEN ?7 IS NOT NULL THEN ?4 END \
                 WHERE NOT EXISTS (SELECT 1 FROM delivery WHERE task_id = ?2 AND review_merge_at >= ?6) \
                 ON CONFLICT(id) DO UPDATE SET ended_at = excluded.ended_at, \
                 verdict = excluded.verdict, model_answered = excluded.model_answered \
                 WHERE review_rounds.merged_at IS NULL AND review_rounds.ended_at IS NULL \
                 AND excluded.ended_at >= review_rounds.started_at",
                params![id, task, head, model, subagent, started_at.0, ended_at, verdict],
            )?;
            Ok(())
        })
        .await
    }

    async fn review_rounds(
        &self,
        task: &TaskId,
        open_only: bool,
    ) -> AppResult<Vec<ReviewRoundRecord>> {
        let t = task.as_str().to_owned();
        self.call(move |c| {
            let mut stmt = c.prepare(&format!(
                "SELECT {REVIEW_ROUND_COLS} \
                     FROM review_rounds WHERE task_id = ?1 AND (?2 = 0 OR merged_at IS NULL) \
                     ORDER BY started_at, rowid",
            ))?;
            let rows = stmt.query_map(params![t, open_only], review_round_from_row)?;
            Ok(rows.collect::<Result<Vec<_>, _>>()?)
        })
        .await
    }

    async fn delivery_review_rounds(
        &self,
        task: &TaskId,
        pr: Option<u64>,
    ) -> AppResult<Vec<ReviewRoundRecord>> {
        let task = task.to_string();
        self.call(move |c| {
            let mut stmt = c.prepare(&format!(
                "WITH wanted AS (SELECT CASE WHEN EXISTS (SELECT 1 FROM review_rounds \
                   WHERE task_id = ?1 AND pr_number IS NULL AND merged_at IS NULL) THEN NULL \
                   ELSE (SELECT MAX(merged_at) FROM review_rounds WHERE task_id = ?1 AND pr_number IS ?2) \
                   END AS merged) \
                 SELECT {REVIEW_ROUND_COLS} FROM review_rounds, wanted \
                 WHERE task_id = ?1 AND merged_at IS wanted.merged \
                 AND pr_number IS (CASE WHEN wanted.merged IS NULL THEN NULL ELSE ?2 END) \
                 ORDER BY started_at, rowid LIMIT {MAX_REVIEW_ROUNDS_READ}"
            ))?;
            let rows = stmt.query_map(params![task, pr.map(to_i64)], review_round_from_row)?;
            Ok(rows.collect::<Result<Vec<_>, _>>()?)
        }).await
    }

    async fn start_review_subagent(
        &self,
        task: &TaskId,
        agent: &str,
        subagent: &str,
        continuation_id: &str,
        at: Timestamp,
    ) -> AppResult<ReviewStart> {
        let (task, agent, subagent, id) = (
            task.as_str().to_owned(),
            agent.to_owned(),
            subagent.to_owned(),
            continuation_id.to_owned(),
        );
        self.call(move |c| {
            let tx = c.transaction()?;
            let previous: Option<(i64, Option<i64>, bool)> = tx
                .query_row(
                    "SELECT rowid, ended_at, agent = ?3 AND merged_at IS NULL \
                     AND COALESCE(ended_at, started_at) <= ?4 FROM review_rounds \
                     WHERE task_id = ?1 AND subagent_id = ?2 ORDER BY rowid DESC LIMIT 1",
                    params![task, subagent, agent, at.0],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                )
                .optional()?;
            let start = match previous {
                Some((_, _, false)) => ReviewStart::Ignored,
                Some((_, None, true)) => ReviewStart::Active,
                Some((row, Some(_), true)) => {
                    // Reserve before reading Git. A stop can settle this row even when
                    // the hook client timed out waiting for its head.
                    tx.execute(
                        "INSERT INTO review_rounds (id, task_id, round, agent, model, \
                         reason, subagent_id, model_answered, started_at) \
                         SELECT ?1, task_id, (SELECT MAX(round) + 1 FROM review_rounds \
                             WHERE task_id = ?2 AND merged_at IS NULL \
                             AND ended_at IS NOT NULL), agent, model, 'kept', \
                             subagent_id, model_answered, ?3 \
                         FROM review_rounds WHERE rowid = ?4",
                        params![id, task, at.0, row],
                    )?;
                    ReviewStart::Continued
                }
                None => {
                    let linked = tx.execute(
                        "UPDATE review_rounds SET subagent_id = ?1 WHERE rowid = (\
                             SELECT rowid FROM review_rounds WHERE task_id = ?2 \
                             AND merged_at IS NULL AND subagent_id IS NULL \
                             ORDER BY started_at DESC, rowid DESC LIMIT 1) \
                             AND agent = ?3 AND started_at <= ?4",
                        params![subagent, task, agent, at.0],
                    )?;
                    if linked == 1 {
                        ReviewStart::Active
                    } else {
                        ReviewStart::Ignored
                    }
                }
            };
            tx.commit()?;
            Ok(start)
        })
        .await
    }

    async fn set_review_round_head(&self, id: &str, head: &str) -> AppResult<()> {
        let (id, head) = (id.to_owned(), head.to_owned());
        self.call(move |c| {
            c.execute(
                "UPDATE review_rounds SET head = ?1 WHERE id = ?2 \
                 AND merged_at IS NULL AND head IS NULL",
                params![head, id],
            )?;
            Ok(())
        })
        .await
    }

    async fn merged_review_rounds(
        &self,
        sandboxes: &[SandboxId],
        from: Option<Timestamp>,
        to: Option<Timestamp>,
    ) -> AppResult<Vec<ReviewRoundRecord>> {
        let ids: Vec<String> = sandboxes.iter().map(|s| s.as_str().to_owned()).collect();
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        self.call(move |c| {
            let marks = vec!["?"; ids.len()].join(", ");
            let sql = format!(
                "SELECT r.id, r.task_id, r.round, r.head, r.agent, r.model, r.reason, \
                 r.subagent_id, r.model_answered, r.started_at, r.ended_at, r.verdict, r.cut_off, \
                 r.pr_number, r.merged_at FROM review_rounds r JOIN tasks t ON t.id = r.task_id \
                 WHERE t.sandbox_id IN ({marks}) AND r.merged_at IS NOT NULL \
                 AND r.merged_at >= ? AND r.merged_at < ? ORDER BY r.merged_at DESC, r.started_at, r.rowid \
                 LIMIT {MAX_REVIEW_ROUNDS_READ}"
            );
            let mut stmt = c.prepare(&sql)?;
            let mut values: Vec<rusqlite::types::Value> =
                ids.into_iter().map(rusqlite::types::Value::Text).collect();
            values.push(rusqlite::types::Value::Integer(from.map_or(i64::MIN, |t| t.0)));
            values.push(rusqlite::types::Value::Integer(to.map_or(i64::MAX, |t| t.0)));
            let rows = stmt.query_map(rusqlite::params_from_iter(values), review_round_from_row)?;
            Ok(rows.collect::<Result<Vec<_>, _>>()?)
        })
        .await
    }

    async fn unfinished_review_model_at(
        &self,
        task: &TaskId,
        subagent: &str,
        at: Timestamp,
    ) -> AppResult<Option<String>> {
        let (task, subagent) = (task.as_str().to_owned(), subagent.to_owned());
        self.call(move |c| {
            let round = round_for_observation(c, &task, &subagent, Some(at))?;
            Ok(round.and_then(|(_, model, ended)| ended.is_none().then_some(model)))
        })
        .await
    }

    async fn note_review_subagent(
        &self,
        task: &TaskId,
        subagent: &str,
        model: Option<&str>,
        cut_off: bool,
        at: Option<Timestamp>,
    ) -> AppResult<()> {
        let (task, subagent, model) = (
            task.as_str().to_owned(),
            subagent.to_owned(),
            model.map(str::to_owned),
        );
        self.call(move |c| {
            let tx = c.transaction()?;
            if let Some((id, _, _)) = round_for_observation(&tx, &task, &subagent, at)? {
                tx.execute(
                    "UPDATE review_rounds SET model_answered = COALESCE(?1, model_answered), \
                     cut_off = cut_off OR ?2 WHERE id = ?3 \
                     AND ((?1 IS NOT NULL AND model_answered IS NOT ?1) OR (?2 AND NOT cut_off))",
                    params![model, cut_off, id],
                )?;
            }
            Ok(tx.commit()?)
        })
        .await
    }

    async fn end_review_round(
        &self,
        task: &TaskId,
        subagent: &str,
        at: Timestamp,
        verdict: Option<&str>,
        model_answered: Option<&str>,
    ) -> AppResult<bool> {
        let (t, s) = (task.as_str().to_owned(), subagent.to_owned());
        let (v, m) = (
            verdict.map(str::to_owned),
            model_answered.map(str::to_owned),
        );
        self.call(move |c| {
            let n = c.execute(
                "UPDATE review_rounds SET ended_at = ?1, verdict = ?2, \
                     model_answered = COALESCE(?3, model_answered) \
                     WHERE task_id = ?4 AND subagent_id = ?5 AND merged_at IS NULL \
                     AND ended_at IS NULL AND started_at <= ?1",
                params![at.0, v, m, t, s],
            )?;
            Ok(n > 0)
        })
        .await
    }

    async fn end_unlinked_review_round(
        &self,
        task: &TaskId,
        (agent, model): (&str, &str),
        subagent: &str,
        at: Timestamp,
        verdict: Option<&str>,
    ) -> AppResult<bool> {
        let (t, a, m, s) = (
            task.as_str().to_owned(),
            agent.to_owned(),
            model.to_owned(),
            subagent.to_owned(),
        );
        let v = verdict.map(str::to_owned);
        self.call(move |c| {
            let n = c.execute(
                "UPDATE review_rounds SET subagent_id = ?1, ended_at = ?2, verdict = ?3, \
                     reason = CASE WHEN agent = ?5 THEN reason ELSE 'round' END, \
                     agent = ?5, model = ?6 \
                     WHERE rowid = (SELECT rowid FROM review_rounds WHERE task_id = ?4 \
                         AND subagent_id IS NULL AND merged_at IS NULL \
                         AND ended_at IS NULL AND started_at <= ?2 \
                         ORDER BY started_at DESC, rowid DESC LIMIT 1) \
                     AND NOT EXISTS (SELECT 1 FROM review_rounds \
                         WHERE task_id = ?4 AND subagent_id = ?1)",
                params![s, at.0, v, t, a, m],
            )?;
            Ok(n > 0)
        })
        .await
    }

    async fn insert_claim(&self, claim: &ScopeClaimRecord) -> AppResult<()> {
        let cl = claim.clone();
        self.call(move |c| {
            c.execute(
                "INSERT INTO scope_claims (id, task_id, sandbox_id, pattern, note, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![cl.id, cl.task_id.as_str(), cl.sandbox_id.as_str(), cl.pattern, cl.note, cl.created_at.0],
            )?;
            Ok(())
        })
        .await
    }

    async fn active_claims(&self, sandbox: &SandboxId) -> AppResult<Vec<ScopeClaimRecord>> {
        let s = sandbox.as_str().to_owned();
        self.call(move |c| {
            let mut stmt = c
                .prepare("SELECT id, task_id, sandbox_id, pattern, note, created_at FROM scope_claims WHERE sandbox_id = ?1 AND released_at IS NULL ORDER BY created_at")?;
            let rows = stmt
                .query_map([s], |r| {
                    let t: String = r.get(1)?;
                    let sb: String = r.get(2)?;
                    Ok(ScopeClaimRecord {
                        id: r.get(0)?,
                        task_id: TaskId::parse(&t).map_err(|_| bad_column("task id", &t))?,
                        sandbox_id: SandboxId::parse(&sb).map_err(|_| bad_column("sandbox id", &sb))?,
                        pattern: r.get(3)?,
                        note: r.get(4)?,
                        created_at: Timestamp(r.get(5)?),
                    })
                })?;
            Ok(rows.collect::<Result<Vec<_>, _>>()?)
        })
        .await
    }

    async fn release_claims(
        &self,
        task: &TaskId,
        pattern: Option<&str>,
        at: Timestamp,
    ) -> AppResult<u32> {
        let (t, p) = (task.as_str().to_owned(), pattern.map(str::to_owned));
        self.call(move |c| {
            let n = c
                .execute(
                    "UPDATE scope_claims SET released_at = ?1 WHERE task_id = ?2 AND released_at IS NULL AND (?3 IS NULL OR pattern = ?3)",
                    params![at.0, t, p],
                )?;
            Ok(u32::try_from(n).unwrap_or(u32::MAX))
        })
        .await
    }

    async fn reserve_record_number(&self, reservation: &RecordReservation) -> AppResult<bool> {
        let r = reservation.clone();
        self.call(move |c| {
            let inserted = c
                .execute(
                    "INSERT INTO record_numbers (sandbox_id, dir, prefix, number, task_id, reserved_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6) ON CONFLICT (sandbox_id, dir, prefix, number) DO NOTHING",
                    params![r.sandbox_id.as_str(), r.dir, r.prefix, to_i64(r.number), r.task_id.as_str(), r.reserved_at.0],
                )?;
            Ok(inserted == 1)
        })
        .await
    }

    async fn record_reservations(
        &self,
        sandbox: &SandboxId,
        dir: &str,
    ) -> AppResult<Vec<RecordReservation>> {
        let (s, d) = (sandbox.as_str().to_owned(), dir.to_owned());
        self.call(move |c| {
            let mut stmt = c
                .prepare("SELECT sandbox_id, dir, prefix, number, task_id, reserved_at FROM record_numbers WHERE sandbox_id = ?1 AND dir = ?2 ORDER BY reserved_at, number")?;
            let rows = stmt
                .query_map(params![s, d], |r| {
                    let sb: String = r.get(0)?;
                    let t: String = r.get(4)?;
                    Ok(RecordReservation {
                        sandbox_id: SandboxId::parse(&sb).map_err(|_| bad_column("sandbox id", &sb))?,
                        dir: r.get(1)?,
                        prefix: r.get(2)?,
                        number: to_u64(r.get(3)?),
                        task_id: TaskId::parse(&t).map_err(|_| bad_column("task id", &t))?,
                        reserved_at: Timestamp(r.get(5)?),
                    })
                })?;
            Ok(rows.collect::<Result<Vec<_>, _>>()?)
        })
        .await
    }
}
