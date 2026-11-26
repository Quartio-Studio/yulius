//! Yulius-run CI runs and jobs (SPEC-002 ADR-21, tables `ci_runs` and `ci_jobs`).

use async_trait::async_trait;
use rusqlite::{OptionalExtension, Row, params, params_from_iter};
use yulius_app::ci::{
    CiJobRecord, CiJobUpdate, CiMinutesRow, CiRunRecord, CiRunUpdate, FiledFinding, GateJob,
    GateRead, GateVerdict, PullHead, Quarantined, RepairRecipient,
};
use yulius_app::error::{AppError, AppResult};
use yulius_app::ports::store_ext::CiStore;
use yulius_domain::ci::{CiRef, GateSource, RunState};
use yulius_domain::ids::{SandboxId, TaskId};
use yulius_domain::resources::{LoadMinute, Need};
use yulius_domain::time::Timestamp;

use crate::{DbResult, SqliteStore, bad_column, opt_ts, text_as, text_parsed, to_u32};

const RUN_COLS: &str = "id, sandbox_id, reference, sha, source, state, reason, requested_by, \
    status_for, status_note, status_attempts, queued_at, started_at, ended_at";
/// How long a claim without an issue holds (SPEC-217 VSI-002): a claim left by a process that
/// stopped between claiming and filing is taken over after it.
const SECURITY_CLAIM_TTL_MS: i64 = 60 * 60 * 1000;

const FINDING_COLS: &str = "fingerprint, file, anchored, issue_number, resolved_at";

fn finding_from_row(row: &Row<'_>) -> rusqlite::Result<FiledFinding> {
    let issue: i64 = row.get("issue_number")?;
    let resolved: Option<i64> = row.get("resolved_at")?;
    Ok(FiledFinding {
        fingerprint: row.get("fingerprint")?,
        file: row.get("file")?,
        anchored: row.get("anchored")?,
        issue: u64::try_from(issue).map_err(|_| bad_column("issue number", &issue.to_string()))?,
        resolved: resolved.is_some(),
    })
}

const JOB_COLS: &str = "id, run_id, ordinal, name, job_key, command, matrix, state, reason, \
    exit_code, started_at, ended_at, cpus, memory_mb, timeout_minutes, exclusive, waited_ms";

/// Adds the run `?1`, once it started and ended, to its sandbox's tally of the UTC day it started
/// (SPEC-497 OFC-003): `failed_outside` when every job of it that failed or errored ended for the
/// infrastructure (`infra_end`), and the jobs' thrown-away time.
const CI_MINUTES_ADD: &str = "INSERT INTO ci_minutes (sandbox_id, day, outcome, runs, run_ms, rerun_ms) \
     SELECT r.sandbox_id, strftime('%Y-%m-%d', r.started_at / 1000, 'unixepoch'), \
       CASE r.state WHEN 'passed' THEN 'passed' WHEN 'cancelled' THEN 'cancelled' \
         ELSE CASE WHEN EXISTS (SELECT 1 FROM ci_jobs j WHERE j.run_id = r.id \
                                AND j.state IN ('failed', 'error')) \
                   AND NOT EXISTS (SELECT 1 FROM ci_jobs j WHERE j.run_id = r.id \
                                AND j.state IN ('failed', 'error') AND COALESCE(j.infra_end, 0) = 0) \
              THEN 'failed_outside' ELSE 'failed' END END, \
       1, MAX(r.ended_at - r.started_at, 0), \
       COALESCE((SELECT SUM(j.infra_ms) FROM ci_jobs j WHERE j.run_id = r.id), 0) \
     FROM ci_runs r WHERE r.id = ?1 AND r.started_at IS NOT NULL AND r.ended_at IS NOT NULL \
     ON CONFLICT (sandbox_id, day, outcome) DO UPDATE SET runs = runs + excluded.runs, \
       run_ms = run_ms + excluded.run_ms, rerun_ms = rerun_ms + excluded.rerun_ms";

fn state(text: &str) -> rusqlite::Result<RunState> {
    RunState::parse(text).ok_or_else(|| bad_column("CI state", text))
}

fn run_from_row(row: &Row<'_>) -> rusqlite::Result<CiRunRecord> {
    let reference: String = row.get("reference")?;
    let source: String = row.get("source")?;
    let status_for: Option<String> = row.get("status_for")?;
    Ok(CiRunRecord {
        id: row.get("id")?,
        sandbox_id: text_parsed(row, "sandbox_id", "sandbox id", SandboxId::parse)?,
        reference: CiRef::parse(&reference)
            .ok_or_else(|| bad_column("CI reference", &reference))?,
        sha: row.get("sha")?,
        // A source a later release wrote reads as the integration branch's configuration, so a
        // rollback keeps reading its runs (SPEC-504 review round 1: `last_valid_yulius_toml`).
        source: GateSource::parse(&source).unwrap_or(GateSource::ProjectConfig),
        state: text_as(row, "state", "CI state", RunState::parse)?,
        reason: row.get("reason")?,
        requested_by: row.get("requested_by")?,
        status_for: status_for.as_deref().map(state).transpose()?,
        status_note: row.get("status_note")?,
        status_attempts: to_u32(row.get("status_attempts")?),
        queued_at: Timestamp(row.get("queued_at")?),
        started_at: opt_ts(row, "started_at")?,
        ended_at: opt_ts(row, "ended_at")?,
    })
}

fn job_from_row(row: &Row<'_>, with_output: bool) -> rusqlite::Result<CiJobRecord> {
    Ok(CiJobRecord {
        id: row.get("id")?,
        run_id: row.get("run_id")?,
        ordinal: to_u32(row.get("ordinal")?),
        name: row.get("name")?,
        key: row.get("job_key")?,
        command: row.get("command")?,
        matrix: row.get("matrix")?,
        state: text_as(row, "state", "CI state", RunState::parse)?,
        reason: row.get("reason")?,
        exit_code: row.get("exit_code")?,
        output: if with_output {
            row.get("output")?
        } else {
            String::new()
        },
        failures: if with_output {
            row.get("failures")?
        } else {
            None
        },
        started_at: opt_ts(row, "started_at")?,
        ended_at: opt_ts(row, "ended_at")?,
        need: Need {
            cpus: row
                .get::<_, Option<i64>>("cpus")?
                .and_then(|n| u32::try_from(n).ok()),
            memory_mb: row
                .get::<_, Option<i64>>("memory_mb")?
                .and_then(|n| u64::try_from(n).ok()),
        },
        timeout_minutes: row
            .get::<_, Option<i64>>("timeout_minutes")?
            .and_then(|n| u32::try_from(n).ok()),
        exclusive: row.get("exclusive")?,
        waited_ms: row.get("waited_ms")?,
    })
}

fn placeholders(from: usize, count: usize) -> String {
    (from..from + count)
        .map(|i| format!("?{i}"))
        .collect::<Vec<_>>()
        .join(", ")
}

fn states(list: &[RunState]) -> Vec<String> {
    list.iter().map(|s| s.as_str().to_owned()).collect()
}

fn select_runs(
    c: &rusqlite::Connection,
    filter: &str,
    args: &[&dyn rusqlite::ToSql],
) -> DbResult<Vec<CiRunRecord>> {
    let mut stmt = c.prepare(&format!("SELECT {RUN_COLS} FROM ci_runs WHERE {filter}"))?;
    let rows = stmt
        .query_map(args, run_from_row)?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

/// Keep only completed job verdicts and test names. Logs follow CI retention.
fn gate_jobs(c: &rusqlite::Connection, id: &str) -> DbResult<Vec<GateJob>> {
    use yulius_domain::ci::failures::{failed_test_names, omitted_tests};
    let mut stmt = c.prepare(
        "SELECT name, state, failures, reason FROM ci_jobs WHERE run_id = ?1
         AND state IN ('passed','failed','error','cancelled','skipped') ORDER BY ordinal",
    )?;
    let mut rows = stmt.query([id])?;
    let mut jobs = Vec::new();
    while let Some(row) = rows.next()? {
        let state = state(&row.get::<_, String>(1)?)?;
        let passed = state == RunState::Passed;
        let partial = state == RunState::Cancelled;
        let kept = row.get::<_, Option<String>>(2)?.unwrap_or_default();
        let tests = if state == RunState::Skipped {
            Vec::new()
        } else if partial {
            let reason: Option<String> = row.get(3)?;
            let names = yulius_app::ci::sibling_failure_names(reason.as_deref(), &kept);
            if names.is_empty() {
                continue;
            }
            names
        } else {
            failed_test_names(&kept)
        };
        jobs.push(GateJob {
            name: row.get(0)?,
            passed,
            partial,
            skipped: state == RunState::Skipped,
            unknown: !passed
                && state != RunState::Skipped
                && (state == RunState::Error || tests.is_empty() || omitted_tests(&kept) > 0),
            tests,
        });
    }
    Ok(jobs)
}

/// Jobs whose recorded failure could survive a green run skipping that job. This
/// lets ordinary green runs with disabled or unrelated jobs end the first-parent walk.
fn failed_gate_jobs(
    c: &rusqlite::Connection,
    sandbox: &str,
    branch: &str,
) -> DbResult<std::collections::HashSet<String>> {
    let mut stmt = c.prepare("SELECT DISTINCT j.name FROM ci_runs r JOIN ci_jobs j ON j.run_id = r.id
        WHERE r.sandbox_id = ?1 AND (r.reference = 'branch/' || ?2 OR r.reference = 'candidate/' || r.sha)
        AND r.state IN ('failed','error') AND j.state IN ('failed','error','cancelled')
        UNION SELECT json_extract(job.value,'$.name') FROM archived_gate_verdicts a, json_each(a.jobs) job
        WHERE a.sandbox_id = ?1 AND (a.reference = 'branch/' || ?2 OR a.reference = 'candidate/' || a.sha)
        AND a.state IN ('failed','error')
        AND (json_extract(job.value,'$.unknown') OR json_array_length(job.value,'$.tests') > 0)")?;
    Ok(stmt
        .query_map(params![sandbox, branch], |row| row.get(0))?
        .collect::<rusqlite::Result<_>>()?)
}

pub(crate) fn runs_for_exact_head(
    conn: &rusqlite::Connection,
    sandbox: &str,
    head: &str,
) -> DbResult<Vec<CiRunRecord>> {
    select_runs(
        conn,
        "sandbox_id = ?1 AND sha = ?2 ORDER BY rowid DESC",
        &[&sandbox, &head],
    )
}

#[async_trait]
impl CiStore for SqliteStore {
    async fn archived_gate_run(
        &self,
        sandbox: &SandboxId,
        id: &str,
    ) -> AppResult<Option<CiRunRecord>> {
        let (sandbox, id) = (sandbox.to_string(), id.to_owned());
        self.call(move |c| {
            Ok(c.query_row(
                &format!("SELECT {RUN_COLS} FROM archived_gate_verdicts WHERE sandbox_id = ?1 AND id = ?2"),
                params![sandbox, id], run_from_row
            ).optional()?)
        }).await
    }

    async fn integration_verdicts(
        &self,
        sandbox: &SandboxId,
        branch: &str,
        revisions: &[String],
        read: GateRead,
    ) -> AppResult<Vec<GateVerdict>> {
        let (sandbox, branch) = (sandbox.to_string(), branch.to_owned());
        let revisions = serde_json::to_string(revisions).map_err(AppError::internal)?;
        self.call(move |c| {
            let mut stmt = c.prepare(
                "WITH history AS (SELECT key, value AS sha FROM json_each(?3)),
                 runs AS (
                    SELECT h.key, r.id, r.sha, r.state, r.queued_at, r.rowid AS seq, NULL AS jobs
                    FROM history h JOIN ci_runs r ON r.sandbox_id = ?1 AND r.sha = h.sha
                    WHERE (r.reference = 'branch/' || ?2 OR r.reference = 'candidate/' || h.sha)
                      AND (r.state IN ('running','queued','passed','failed','error','skipped')
                           OR (r.state = 'cancelled' AND r.reference = 'candidate/' || h.sha))
                      AND NOT EXISTS (SELECT 1 FROM ci_jobs j WHERE j.run_id = r.id AND j.state = 'skipped' AND instr(j.reason, ?4) = 1)
                    UNION ALL SELECT h.key, a.id, a.sha, a.state, a.queued_at, a.sequence, a.jobs
                    FROM history h JOIN archived_gate_verdicts a ON a.sandbox_id = ?1 AND a.sha = h.sha
                    WHERE a.reference = 'branch/' || ?2 OR a.reference = 'candidate/' || h.sha
                 ), ranked AS (SELECT *, row_number() OVER (PARTITION BY sha ORDER BY (?5 != 0 AND state IN ('running','queued')), queued_at DESC, seq DESC) AS newest FROM runs)
                 SELECT r.id, r.sha, r.state, r.jobs, o.task_number
                 FROM ranked r LEFT JOIN gate_merge_owners o ON o.sandbox_id = ?1 AND o.branch = ?2 AND o.sha = r.sha
                 WHERE r.newest = 1 OR (?6 AND r.state NOT IN ('running','queued')) ORDER BY r.key, r.newest"
            )?;
            let mut rows = stmt.query(params![sandbox, branch, revisions, yulius_domain::ci::AFTER_MERGE_SKIP_REASON, read == GateRead::SettledHistory, read != GateRead::Latest])?;
            let mut result = Vec::new();
            let mut failed_jobs = None;
            while let Some(row) = rows.next()? {
                let run: String = row.get(0)?;
                let state = state(&row.get::<_, String>(2)?)?;
                let archive: Option<String> = row.get(3)?;
                let mut jobs = if matches!(state, RunState::Failed | RunState::Error | RunState::Passed) {
                    match archive.as_deref() {
                        Some(archived) => serde_json::from_str(archived).map_err(AppError::internal)?,
                        None => gate_jobs(c, &run)?,
                    }
                } else { Vec::new() };
                if state == RunState::Passed {
                    jobs.retain(|job| job.skipped);
                    if !jobs.is_empty() {
                        if failed_jobs.is_none() {
                            failed_jobs = Some(failed_gate_jobs(c, &sandbox, &branch)?);
                        }
                        jobs.retain(|job| failed_jobs.as_ref().is_some_and(|names| names.contains(&job.name)));
                    }
                }
                let complete_pass = state == RunState::Passed && jobs.is_empty();
                result.push(GateVerdict {
                    run,
                    sha: row.get(1)?,
                    state,
                    jobs,
                    owner: row.get(4)?,
                });
                if read != GateRead::Latest && complete_pass { break; }
            }
            Ok(result)
        }).await
    }

    async fn accepts_machine_notice(
        &self,
        task: &TaskId,
        unfinished_goal: bool,
    ) -> AppResult<bool> {
        let task = task.to_string();
        self.call(move |c| {
            Ok(c.query_row(
                "SELECT EXISTS(SELECT 1 FROM machine_notice_tasks WHERE id = ?1 AND (NOT needs_goal OR ?2))",
                params![task, unfinished_goal],
                |row| row.get(0),
            )?)
        })
        .await
    }

    async fn gate_workers(&self, sandbox: &SandboxId) -> AppResult<Vec<TaskId>> {
        let sandbox = sandbox.to_string();
        self.call(move |c| {
            let mut stmt =
                c.prepare("SELECT id FROM gate_workers WHERE sandbox_id = ?1 ORDER BY number")?;
            stmt.query_map([sandbox], |r| r.get::<_, String>(0))?
                .map(|id| Ok(TaskId::parse(&id?).map_err(AppError::internal)?))
                .collect()
        })
        .await
    }

    async fn reconcile_gate_repair(
        &self,
        sandbox: &SandboxId,
        detail: &str,
        available: &[TaskId],
    ) -> AppResult<Vec<RepairRecipient>> {
        let available =
            serde_json::to_string(&available.iter().map(TaskId::as_str).collect::<Vec<_>>())
                .map_err(AppError::internal)?;
        let (sandbox, detail) = (sandbox.clone(), detail.to_owned());
        self.call(move |c| {
            let tx = c.transaction()?;
            let sid = sandbox.as_str();
            // Keep the incumbent if eligible. Replacing the row also expires its delivery receipts.
            let owner: Option<(String, u32)> = tx.query_row(
                "SELECT w.id, w.number FROM gate_workers w LEFT JOIN gate_repairs r ON r.sandbox_id = w.sandbox_id
                 WHERE w.sandbox_id = ?1 AND w.id IN (SELECT value FROM json_each(?2)) ORDER BY (w.id IS r.owner_id) DESC, w.number LIMIT 1",
                params![sid, available], |r| Ok((r.get(0)?, r.get(1)?))).optional()?;
            let mut recipients = Vec::new();
            if let Some((owner, number)) = owner.filter(|_| !detail.is_empty()) {
                tx.execute("DELETE FROM gate_repairs WHERE sandbox_id = ?1 AND (detail != ?2 OR owner_id != ?3)",
                    params![sid, detail, owner])?;
                tx.execute("INSERT OR IGNORE INTO gate_repairs(sandbox_id, detail, owner_id) VALUES (?1, ?2, ?3)",
                    params![sid, detail, owner])?;
                tx.execute("DELETE FROM gate_repair_told WHERE sandbox_id = ?1 AND task_id NOT IN (SELECT id FROM gate_workers WHERE sandbox_id = ?1)", [sid])?;
                let mut stmt = tx.prepare("SELECT id FROM gate_workers w WHERE sandbox_id = ?1 AND id IN (SELECT value FROM json_each(?3))
                    AND NOT EXISTS (SELECT 1 FROM gate_repair_told t WHERE t.task_id = w.id AND t.sandbox_id = ?1)
                    ORDER BY (id = ?2) DESC, number")?;
                for id in stmt.query_map(params![sid, owner, available], |row| row.get::<_, String>(0))? {
                    let id = id?;
                    recipients.push(RepairRecipient {
                        task: TaskId::parse(&id).map_err(AppError::internal)?,
                        assigned: id == owner,
                        owner: number,
                    });
                }
            } else {
                tx.execute("DELETE FROM gate_repairs WHERE sandbox_id = ?1", [sid])?;
            }
            tx.commit()?;
            Ok(recipients)
        }).await
    }

    async fn gate_repair_due(
        &self,
        recipient: &RepairRecipient,
        detail: &str,
        acknowledge: bool,
    ) -> AppResult<bool> {
        let (task, number, detail) = (
            recipient.task.to_string(),
            recipient.owner,
            detail.to_owned(),
        );
        self.call(move |c| {
            let select = "SELECT w.sandbox_id, w.id FROM gate_workers w JOIN gate_repairs r ON r.sandbox_id = w.sandbox_id
                JOIN gate_workers owner ON owner.id = r.owner_id WHERE w.id = ?1 AND owner.number = ?2 AND r.detail = ?3
                AND (w.id = r.owner_id OR EXISTS (SELECT 1 FROM gate_repair_told a WHERE a.sandbox_id = r.sandbox_id AND a.task_id = r.owner_id))
                AND NOT EXISTS (SELECT 1 FROM gate_repair_told t WHERE t.task_id = w.id AND t.sandbox_id = w.sandbox_id)";
            let args = params![task, number, detail];
            if acknowledge {
                Ok(c.execute(&format!("INSERT INTO gate_repair_told(sandbox_id, task_id) {select}"), args).map(|n| n == 1)?)
            } else {
                Ok(c.query_row(&format!("SELECT EXISTS({select})"), args, |row| row.get(0))?)
            }
        }).await
    }

    async fn has_gate_failure(&self, sandbox: &SandboxId, branch: &str) -> AppResult<bool> {
        let (sandbox, branch) = (sandbox.to_string(), branch.to_owned());
        self.call(move |c| Ok(c.query_row("SELECT EXISTS (
            SELECT 1 FROM ci_runs WHERE sandbox_id = ?1 AND state IN ('failed','error','cancelled')
                AND (reference = 'branch/' || ?2 OR reference = 'candidate/' || sha)
            UNION ALL SELECT 1 FROM archived_gate_verdicts WHERE sandbox_id = ?1 AND state IN ('failed','error','cancelled')
                AND (reference = 'branch/' || ?2 OR reference = 'candidate/' || sha))",
            params![sandbox, branch], |row| row.get(0))?)).await
    }

    async fn ci_admitted_pulls(&self, sandbox: &SandboxId) -> AppResult<Vec<u64>> {
        let sandbox = sandbox.as_str().to_owned();
        self.call(move |c| {
            let numbers: String = c.query_row(
                "SELECT ci_open_pulls FROM sandboxes WHERE id = ?1",
                [sandbox],
                |row| row.get(0),
            )?;
            Ok(serde_json::from_str(&numbers).map_err(AppError::internal)?)
        })
        .await
    }

    async fn set_ci_admitted_pulls(&self, sandbox: &SandboxId, numbers: &[u64]) -> AppResult<()> {
        let sandbox = sandbox.as_str().to_owned();
        let numbers = serde_json::to_string(numbers).map_err(AppError::internal)?;
        self.call(move |c| {
            let changed = c.execute(
                "UPDATE sandboxes SET ci_open_pulls = ?2 WHERE id = ?1 AND removed_at IS NULL",
                params![sandbox, numbers],
            )?;
            if changed == 0 {
                return Err(AppError::NotFound.into());
            }
            Ok(())
        })
        .await
    }

    async fn insert_ci_run(&self, run: &CiRunRecord, jobs: &[CiJobRecord]) -> AppResult<()> {
        let (run, jobs) = (run.clone(), jobs.to_vec());
        self.call(move |c| {
            let tx = c.transaction()?;
            // Removal keeps the sandbox row. Its cleanup can already have taken a snapshot
            // of active runs while this producer was waiting for Git.
            let inserted = tx.execute(
                &format!(
                    "INSERT INTO ci_runs ({RUN_COLS}) SELECT {} WHERE NOT EXISTS \
                     (SELECT 1 FROM sandboxes WHERE id = ?2 AND removed_at IS NOT NULL)",
                    placeholders(1, 14)
                ),
                params![
                    run.id,
                    run.sandbox_id.as_str(),
                    run.reference.key(),
                    run.sha,
                    run.source.as_str(),
                    run.state.as_str(),
                    run.reason,
                    run.requested_by,
                    run.status_for.map(RunState::as_str),
                    run.status_note,
                    i64::from(run.status_attempts),
                    run.queued_at.0,
                    run.started_at.map(|t| t.0),
                    run.ended_at.map(|t| t.0),
                ],
            )?;
            if inserted == 0 {
                return Err(AppError::NotFound.into());
            }
            for job in &jobs {
                tx.execute(
                    &format!(
                        "INSERT INTO ci_jobs ({JOB_COLS}, output) VALUES ({})",
                        placeholders(1, 18)
                    ),
                    params![
                        job.id,
                        job.run_id,
                        i64::from(job.ordinal),
                        job.name,
                        job.key,
                        job.command,
                        job.matrix,
                        job.state.as_str(),
                        job.reason,
                        job.exit_code,
                        job.started_at.map(|t| t.0),
                        job.ended_at.map(|t| t.0),
                        job.need.cpus.map(i64::from),
                        job.need.memory_mb.and_then(|m| i64::try_from(m).ok()),
                        job.timeout_minutes.map(i64::from),
                        job.exclusive,
                        job.waited_ms,
                        job.output,
                    ],
                )?;
            }
            Ok(tx.commit()?)
        })
        .await
    }

    async fn ci_run(&self, id: &str) -> AppResult<Option<CiRunRecord>> {
        let id = id.to_owned();
        self.call(move |c| {
            Ok(c.query_row(
                &format!("SELECT {RUN_COLS} FROM ci_runs WHERE id = ?1"),
                [id],
                run_from_row,
            )
            .optional()?)
        })
        .await
    }

    async fn ci_jobs(&self, run: &str, with_output: bool) -> AppResult<Vec<CiJobRecord>> {
        let run = run.to_owned();
        self.call(move |c| {
            let cols = if with_output {
                format!("{JOB_COLS}, output, failures")
            } else {
                JOB_COLS.to_owned()
            };
            let mut stmt = c.prepare(&format!(
                "SELECT {cols} FROM ci_jobs WHERE run_id = ?1 ORDER BY ordinal"
            ))?;
            let rows = stmt
                .query_map([run], |row| job_from_row(row, with_output))?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(rows)
        })
        .await
    }

    async fn ci_runs(&self, sandbox: &SandboxId, limit: u32) -> AppResult<Vec<CiRunRecord>> {
        let sandbox = sandbox.as_str().to_owned();
        self.call(move |c| {
            select_runs(
                c,
                "sandbox_id = ?1 ORDER BY queued_at DESC, rowid DESC LIMIT ?2",
                &[&sandbox, &i64::from(limit)],
            )
        })
        .await
    }

    async fn ci_shas_with_pass(
        &self,
        sandbox: &SandboxId,
        commits: &[String],
    ) -> AppResult<Vec<String>> {
        let sandbox = sandbox.to_string();
        let commits = serde_json::to_string(commits).map_err(AppError::internal)?;
        self.call(move |c| {
            let mut statement = c.prepare(
                "SELECT value FROM json_each(?2) AS history WHERE EXISTS (
                    SELECT 1 FROM ci_runs r WHERE r.sandbox_id = ?1 AND r.sha = history.value AND r.state = 'passed'
                 ) ORDER BY history.key",
            )?;
            Ok(statement.query_map(params![sandbox, commits], |row| row.get(0))?.collect::<Result<Vec<_>, _>>()?)
        })
        .await
    }

    async fn ci_planning_runs(
        &self,
        sandbox: &SandboxId,
        limit: u32,
        integration_head: Option<&yulius_domain::ci::Target>,
    ) -> AppResult<Vec<CiRunRecord>> {
        let sandbox = sandbox.as_str().to_owned();
        let head = integration_head.map(|target| target.sha.clone());
        let branch = integration_head.map(|target| target.reference.key());
        self.call(move |c| {
            select_runs(
                c,
                "rowid IN (SELECT rowid FROM (SELECT rowid FROM ci_runs WHERE sandbox_id = ?1 ORDER BY rowid DESC LIMIT ?2)
                   UNION ALL SELECT r.rowid FROM ci_runs r WHERE r.sandbox_id = ?1 AND r.sha = ?3
                   AND (instr(r.reason, ?5) = 1 OR ((r.reference = ?4 OR r.reference = 'candidate/' || ?3)
                       AND NOT EXISTS (SELECT 1 FROM ci_jobs j WHERE j.run_id = r.id AND j.state = 'skipped'
                           AND instr(j.reason, ?6) = 1)))) ORDER BY rowid DESC",
                &[&sandbox, &i64::from(limit), &head, &branch, &yulius_app::ci::DEV_INVALID_PREFIX, &yulius_domain::ci::AFTER_MERGE_SKIP_REASON],
            )
        })
        .await
    }

    async fn ci_runs_for_sha(&self, sandbox: &SandboxId, sha: &str) -> AppResult<Vec<CiRunRecord>> {
        let (sandbox, sha) = (sandbox.as_str().to_owned(), sha.to_owned());
        self.call(move |c| {
            select_runs(
                c,
                "sandbox_id = ?1 AND sha = ?2 ORDER BY queued_at DESC, rowid DESC",
                &[&sandbox, &sha],
            )
        })
        .await
    }

    async fn ci_runs_for_pull_head(
        &self,
        sandbox: &SandboxId,
        head: PullHead<'_>,
    ) -> AppResult<Vec<CiRunRecord>> {
        let (head, exact_only) = match head {
            PullHead::Exact(sha) => (sha.to_owned(), true),
            PullHead::WaitArgument(input) => (input.to_owned(), input.len() == 64),
        };
        let sandbox = sandbox.as_str().to_owned();
        // A hexadecimal range uses the (sandbox_id, sha) index. Two distinct
        // commits suffice to rule out ambiguity, regardless of the sandbox’s age.
        self.call(move |c| {
            if exact_only || head.len() == 40 {
                let exact = runs_for_exact_head(c, &sandbox, &head)?;
                if exact_only || !exact.is_empty() {
                    return Ok(exact);
                }
            }
            select_runs(
                c,
                "sandbox_id = ?1 AND sha = (
                    SELECT MIN(sha) FROM (
                        SELECT DISTINCT sha FROM ci_runs
                        WHERE sandbox_id = ?1 AND sha >= ?2 AND sha < ?3 LIMIT 2
                    ) HAVING COUNT(*) = 1
                ) ORDER BY rowid DESC",
                &[&sandbox, &head, &format!("{head}g")],
            )
        })
        .await
    }

    async fn ci_run_to_front(&self, id: &str) -> AppResult<bool> {
        let id = id.to_owned();
        self.call(move |c| {
            // The order the dispatcher reads: place, then insertion. A millisecond before the
            // earliest queued run of the sandbox, but never before another run of the same
            // commit: which run of a commit is the latest is read by place (`ci_runs_for_sha`).
            let n = c.execute(
                "UPDATE ci_runs SET queued_at = MAX( \
                       (SELECT MIN(r.queued_at) - 1 FROM ci_runs r \
                        WHERE r.sandbox_id = ci_runs.sandbox_id AND r.state = 'queued'), \
                       COALESCE((SELECT MAX(r.queued_at) + 1 FROM ci_runs r \
                        WHERE r.sandbox_id = ci_runs.sandbox_id AND r.sha = ci_runs.sha \
                        AND r.id != ci_runs.id), -9223372036854775807)) \
                     WHERE id = ?1 AND state = 'queued' AND EXISTS ( \
                       SELECT 1 FROM ci_runs r WHERE r.sandbox_id = ci_runs.sandbox_id \
                       AND r.state = 'queued' \
                       AND (r.queued_at < ci_runs.queued_at \
                         OR (r.queued_at = ci_runs.queued_at AND r.rowid < ci_runs.rowid))) \
                     AND NOT EXISTS ( \
                       SELECT 1 FROM ci_runs r WHERE r.sandbox_id = ci_runs.sandbox_id \
                       AND r.sha = ci_runs.sha AND r.id != ci_runs.id \
                       AND r.queued_at + 1 >= ci_runs.queued_at)",
                [id],
            )?;
            Ok(n == 1)
        })
        .await
    }

    async fn active_ci_runs(&self) -> AppResult<Vec<CiRunRecord>> {
        self.call(move |c| {
            select_runs(
                c,
                "state IN ('queued', 'running') ORDER BY queued_at, rowid",
                &[],
            )
        })
        .await
    }

    async fn has_finishing_ci_run(&self) -> AppResult<bool> {
        self.call(|c| {
            // The IN predicate selects the partial ci_runs_active index. Read only the indexed
            // jobs of a running run, and stop at the first qualifying gate.
            Ok(c.query_row(
                "SELECT EXISTS (SELECT 1 FROM ci_runs r \
                 WHERE r.state IN ('queued', 'running') AND r.state = 'running' AND EXISTS (\
                   SELECT 1 FROM ci_jobs WHERE run_id = r.id GROUP BY run_id \
                   HAVING COUNT(*) FILTER (WHERE state = 'passed' AND started_at IS NOT NULL \
                                             AND exit_code = 0) > 0 \
                      AND COUNT(*) FILTER (WHERE state = 'queued') = 0 \
                      AND COUNT(*) FILTER (WHERE state = 'running') <= 1))",
                [],
                |row| row.get(0),
            )?)
        })
        .await
    }

    async fn transition_ci_run(
        &self,
        id: &str,
        from: &[RunState],
        to: RunState,
        update: &CiRunUpdate,
    ) -> AppResult<bool> {
        let (id, from, update) = (id.to_owned(), states(from), update.clone());
        let id_for_tally = id.clone();
        self.call(move |c| {
            // A reason before a run starts is a queue note (waiting for disk): it goes.
            let sql = format!(
                "UPDATE ci_runs SET state = ?1, \
                 reason = CASE WHEN ?1 = 'running' THEN ?2 ELSE COALESCE(?2, reason) END, \
                 started_at = COALESCE(?3, started_at), ended_at = COALESCE(?4, ended_at) \
                 WHERE id = ?5 AND state IN ({})",
                placeholders(6, from.len())
            );
            let mut args: Vec<Box<dyn rusqlite::ToSql>> = vec![
                Box::new(to.as_str().to_owned()),
                Box::new(update.reason),
                Box::new(update.started_at.map(|t| t.0)),
                Box::new(update.ended_at.map(|t| t.0)),
                Box::new(id),
            ];
            args.extend(
                from.into_iter()
                    .map(|s| Box::new(s) as Box<dyn rusqlite::ToSql>),
            );
            // The new state and the day's tally together (review round 2).
            let tx = c.transaction()?;
            let changed = match tx.execute(&sql, params_from_iter(args.iter().map(AsRef::as_ref))) {
                Ok(changed) => changed == 1,
                Err(rusqlite::Error::SqliteFailure(err, _))
                    if err.code == rusqlite::ErrorCode::ConstraintViolation =>
                {
                    return Err(AppError::Conflict(
                        "a constraint refused the CI run's new state".into(),
                    )
                    .into());
                }
                Err(err) => return Err(err.into()),
            };
            // A run that started and ended adds its minutes to its day's tally (SPEC-497
            // OFC-003, review round 1: runs are pruned, the tally stays).
            if changed
                && matches!(
                    to,
                    RunState::Passed | RunState::Failed | RunState::Error | RunState::Cancelled
                )
            {
                tx.execute(CI_MINUTES_ADD, params![id_for_tally])?;
            }
            tx.commit()?;
            Ok(changed)
        })
        .await
    }

    async fn transition_ci_job(
        &self,
        id: &str,
        from: &[RunState],
        to: RunState,
        update: &CiJobUpdate,
    ) -> AppResult<bool> {
        let (id, from, update) = (id.to_owned(), states(from), update.clone());
        self.call(move |c| {
            let sql = format!(
                "UPDATE ci_jobs SET state = ?1, reason = COALESCE(?2, reason), \
                 exit_code = COALESCE(?3, exit_code), output = COALESCE(?4, output), \
                 started_at = COALESCE(?5, started_at), ended_at = COALESCE(?6, ended_at), \
                 failures = COALESCE(?8, failures), cpu_ms = COALESCE(?9, cpu_ms), \
                 memory_peak_mb = COALESCE(?10, memory_peak_mb), \
                 disk_written_mb = COALESCE(?11, disk_written_mb), \
                 waited_ms = COALESCE(?12, waited_ms), infra = COALESCE(?13, infra), \
                 infra_ms = COALESCE(?14, infra_ms), infra_end = COALESCE(?15, infra_end) \
                 WHERE id = ?7 AND state IN ({})",
                placeholders(16, from.len())
            );
            let mut args: Vec<Box<dyn rusqlite::ToSql>> = vec![
                Box::new(to.as_str().to_owned()),
                Box::new(update.reason),
                Box::new(update.exit_code),
                Box::new(update.output),
                Box::new(update.started_at.map(|t| t.0)),
                Box::new(update.ended_at.map(|t| t.0)),
                Box::new(id),
                Box::new(update.failures),
                Box::new(update.usage.map(|u| u.cpu_ms)),
                Box::new(update.usage.map(|u| u.memory_peak_mb)),
                Box::new(update.usage.and_then(|u| u.disk_written_mb)),
                Box::new(update.waited_ms),
                Box::new(update.infra.map(|(kind, _, _)| kind.to_owned())),
                Box::new(update.infra.map(|(_, ms, _)| ms)),
                Box::new(update.infra.map(|(_, _, end)| i64::from(end))),
            ];
            args.extend(
                from.into_iter()
                    .map(|s| Box::new(s) as Box<dyn rusqlite::ToSql>),
            );
            let changed = c.execute(&sql, params_from_iter(args.iter().map(AsRef::as_ref)))?;
            Ok(changed == 1)
        })
        .await
    }

    async fn ci_minutes(
        &self,
        sandbox: &SandboxId,
        from: Option<Timestamp>,
        to: Option<Timestamp>,
    ) -> AppResult<Vec<CiMinutesRow>> {
        let day = |at: Option<Timestamp>, or: &str| {
            at.map_or_else(
                || or.to_owned(),
                |at| {
                    yulius_domain::time::format_rfc3339(at)
                        .chars()
                        .take(10)
                        .collect()
                },
            )
        };
        let (sandbox, from, to) = (
            sandbox.as_str().to_owned(),
            day(from, "0000-00-00"),
            day(to, "9999-99-99"),
        );
        self.call(move |c| {
            let mut statement = c.prepare(
                "SELECT day, outcome, runs, run_ms, rerun_ms FROM ci_minutes \
                     WHERE sandbox_id = ?1 AND day >= ?2 AND day < ?3 ORDER BY day, outcome",
            )?;
            let rows = statement
                .query_map(params![sandbox, from, to], |row| {
                    Ok(CiMinutesRow {
                        day: row.get(0)?,
                        outcome: row.get(1)?,
                        runs: row.get(2)?,
                        run_ms: row.get(3)?,
                        rerun_ms: row.get(4)?,
                    })
                })?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(rows)
        })
        .await
    }

    async fn ci_jobs_waiting(&self) -> AppResult<u32> {
        self.call(|c| {
            let waiting: i64 = c.query_row(
                "SELECT COUNT(*) FROM ci_jobs j JOIN ci_runs r ON r.id = j.run_id \
                     WHERE r.state = 'running' AND j.state = 'queued'",
                [],
                |row| row.get(0),
            )?;
            Ok(u32::try_from(waiting).unwrap_or(u32::MAX))
        })
        .await
    }

    async fn record_test_failures(
        &self,
        sandbox: &SandboxId,
        job_key: &str,
        (reference, run, sha): (&str, &str, &str),
        tests: &[String],
        at: Timestamp,
        forget_before: Timestamp,
    ) -> AppResult<()> {
        let (sandbox, job_key, reference, run, sha, tests) = (
            sandbox.as_str().to_owned(),
            job_key.to_owned(),
            reference.to_owned(),
            run.to_owned(),
            sha.to_owned(),
            tests.to_vec(),
        );
        self.call(move |c| {
            let tx = c.transaction()?;
            for test in &tests {
                tx.execute(
                    "INSERT INTO ci_test_failures (sandbox_id, job_key, test, reference, run_id, \
                     sha, at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                    params![sandbox, job_key, test, reference, run, sha, at.0],
                )?;
            }
            tx.execute(
                "DELETE FROM ci_test_failures WHERE at < ?1",
                params![forget_before.0],
            )?;
            Ok(tx.commit()?)
        })
        .await
    }

    async fn test_failures(
        &self,
        sandbox: &SandboxId,
        job_key: &str,
        test: &str,
        since: Timestamp,
    ) -> AppResult<Vec<yulius_domain::ci::flaky::TestFailure>> {
        let (sandbox, job_key, test) = (
            sandbox.as_str().to_owned(),
            job_key.to_owned(),
            test.to_owned(),
        );
        self.call(move |c| {
            let mut statement = c.prepare(
                "SELECT reference, sha, at FROM ci_test_failures WHERE sandbox_id = ?1 \
                     AND job_key = ?2 AND test = ?3 AND at >= ?4 ORDER BY at",
            )?;
            let rows = statement
                .query_map(params![sandbox, job_key, test, since.0], |row| {
                    Ok(yulius_domain::ci::flaky::TestFailure {
                        reference: row.get(0)?,
                        sha: row.get(1)?,
                        at: Timestamp(row.get(2)?),
                    })
                })?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(rows)
        })
        .await
    }

    async fn quarantine_test(&self, entry: &Quarantined) -> AppResult<bool> {
        let entry = entry.clone();
        self.call(move |c| {
            let added = c.execute(
                "INSERT OR IGNORE INTO ci_quarantine (sandbox_id, test, job_key, issue_number, \
                     issue_url, since) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    entry.sandbox_id.as_str(),
                    entry.test,
                    entry.job_key,
                    entry
                        .issue
                        .as_ref()
                        .map(|(n, _)| i64::try_from(*n).unwrap_or(i64::MAX)),
                    entry.issue.as_ref().map(|(_, url)| url.clone()),
                    entry.since.0
                ],
            )?;
            Ok(added == 1)
        })
        .await
    }

    async fn quarantined_tests(&self, sandbox: Option<&SandboxId>) -> AppResult<Vec<Quarantined>> {
        let sandbox = sandbox.map(|s| s.as_str().to_owned());
        self.call(move |c| {
            let mut statement = c
                .prepare(
                    "SELECT sandbox_id, test, job_key, issue_number, issue_url, since \
                     FROM ci_quarantine WHERE ?1 IS NULL OR sandbox_id = ?1 ORDER BY since, test",
                )?;
            let rows = statement
                .query_map(params![sandbox], |row| {
                    let sandbox: String = row.get(0)?;
                    let number: Option<i64> = row.get(3)?;
                    let url: Option<String> = row.get(4)?;
                    Ok((sandbox, row.get(1)?, row.get(2)?, number, url, row.get(5)?))
                })?
                .collect::<Result<Vec<(String, String, String, Option<i64>, Option<String>, i64)>, _>>()?;
            rows.into_iter()
                .map(|(sandbox, test, job_key, number, url, since)| {
                    Ok(Quarantined {
                        sandbox_id: SandboxId::parse(&sandbox)
                            .map_err(|_| AppError::Internal("a quarantined test's sandbox".into()))?,
                        test,
                        job_key,
                        issue: number
                            .and_then(|n| u64::try_from(n).ok())
                            .zip(url),
                        since: Timestamp(since),
                    })
                })
                .collect()
        })
        .await
    }

    async fn release_test(&self, sandbox: &SandboxId, test: &str) -> AppResult<()> {
        let (sandbox, test) = (sandbox.as_str().to_owned(), test.to_owned());
        self.call(move |c| {
            let tx = c.transaction()?;
            tx.execute(
                "DELETE FROM ci_quarantine WHERE sandbox_id = ?1 AND test = ?2",
                params![sandbox, test],
            )?;
            tx.execute(
                "DELETE FROM ci_test_failures WHERE sandbox_id = ?1 AND test = ?2",
                params![sandbox, test],
            )?;
            Ok(tx.commit()?)
        })
        .await
    }

    async fn record_ci_load(&self, minute: LoadMinute, forget_before: Timestamp) -> AppResult<()> {
        self.call(move |c| {
            c.execute(
                "INSERT OR REPLACE INTO ci_load (at, busy_permille, held_cpus, measured_cpus, \
                 waiting_jobs) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![
                    minute.at.0,
                    minute.busy_permille,
                    minute.held_cpus,
                    minute.measured_cpus,
                    minute.waiting_jobs
                ],
            )?;
            c.execute(
                "DELETE FROM ci_load WHERE at < ?1",
                params![forget_before.0],
            )?;
            Ok(())
        })
        .await
    }

    async fn ci_load(
        &self,
        from: Option<Timestamp>,
        to: Option<Timestamp>,
    ) -> AppResult<Vec<LoadMinute>> {
        let (from, to) = (from.map_or(i64::MIN, |t| t.0), to.map_or(i64::MAX, |t| t.0));
        self.call(move |c| {
            let mut statement = c.prepare(
                "SELECT at, busy_permille, held_cpus, measured_cpus, waiting_jobs FROM ci_load \
                     WHERE at >= ?1 AND at < ?2 ORDER BY at",
            )?;
            let rows = statement
                .query_map(params![from, to], |row| {
                    Ok(LoadMinute {
                        at: Timestamp(row.get(0)?),
                        busy_permille: row.get(1)?,
                        held_cpus: row.get(2)?,
                        measured_cpus: row.get(3)?,
                        waiting_jobs: row.get(4)?,
                    })
                })?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(rows)
        })
        .await
    }

    async fn set_queued_ci_job_reason(&self, id: &str, reason: Option<&str>) -> AppResult<()> {
        let (id, reason) = (id.to_owned(), reason.map(str::to_owned));
        self.call(move |c| {
            c.execute(
                "UPDATE ci_jobs SET reason = ?2 WHERE id = ?1 AND state = 'queued'",
                params![id, reason],
            )?;
            Ok(())
        })
        .await
    }

    async fn set_ci_job_output(&self, id: &str, output: &str) -> AppResult<()> {
        let (id, output) = (id.to_owned(), output.to_owned());
        self.call(move |c| {
            c.execute(
                "UPDATE ci_jobs SET output = ?2 WHERE id = ?1 AND state = 'running'",
                params![id, output],
            )?;
            Ok(())
        })
        .await
    }

    async fn requeue_ci_run(&self, id: &str, note: &str, at: Timestamp) -> AppResult<bool> {
        let (id, note) = (id.to_owned(), note.to_owned());
        self.call(move |c| {
            let tx = c.transaction()?;
            let changed = tx
                .execute(
                    "UPDATE ci_runs SET state = 'queued', started_at = NULL, ended_at = NULL, \
                     reason = NULL, requeued_at = ?2 WHERE id = ?1 AND state = 'running'",
                    params![id, at.0],
                )?;
            if changed == 1 {
                tx.execute(
                    "UPDATE ci_jobs SET state = 'queued', started_at = NULL, ended_at = NULL, \
                     reason = NULL, exit_code = NULL, output = ?2, failures = NULL, cpu_ms = NULL, \
                     memory_peak_mb = NULL, disk_written_mb = NULL, waited_ms = NULL, infra = NULL, \
                     infra_ms = NULL, infra_end = NULL \
                     WHERE run_id = ?1 AND NOT (state = 'skipped' \
                       AND substr(COALESCE(reason, ''), 1, length(?3)) = ?3)",
                    [&id, &note, &yulius_domain::ci::PATHS_SKIP_REASON.to_owned()],
                )?;
            }
            tx.commit()?;
            Ok(changed == 1)
        })
        .await
    }

    async fn record_ci_status(
        &self,
        id: &str,
        published: Option<RunState>,
        note: Option<&str>,
    ) -> AppResult<()> {
        let (id, note) = (id.to_owned(), note.map(str::to_owned));
        self.call(move |c| {
            match published {
                // Attempts count the failures since the last status GitHub took: the statuses
                // a running run publishes as its jobs end leave its final one every retry
                // (SPEC-198 RCS-004).
                Some(state) => c.execute(
                    "UPDATE ci_runs SET status_for = ?2, status_note = NULL, \
                     status_attempts = 0 WHERE id = ?1",
                    params![id, state.as_str()],
                ),
                None => c.execute(
                    "UPDATE ci_runs SET status_note = ?2, status_attempts = status_attempts + 1 \
                     WHERE id = ?1",
                    params![id, note],
                ),
            }?;
            Ok(())
        })
        .await
    }

    async fn unpublished_ci_runs(
        &self,
        since: Timestamp,
        max_attempts: u32,
        limit: u32,
    ) -> AppResult<Vec<CiRunRecord>> {
        self.call(move |c| {
            select_runs(
                c,
                "queued_at >= ?1 AND status_attempts < ?2 \
                 AND (status_for IS NULL OR status_for <> state) \
                 ORDER BY queued_at, rowid LIMIT ?3",
                &[&since.0, &i64::from(max_attempts), &i64::from(limit)],
            )
        })
        .await
    }

    async fn claim_security_finding(
        &self,
        sandbox: &SandboxId,
        (fingerprint, file, anchored): (&str, &str, bool),
        at: Timestamp,
    ) -> AppResult<bool> {
        let (sandbox, fingerprint, file) = (
            sandbox.as_str().to_owned(),
            fingerprint.to_owned(),
            file.to_owned(),
        );
        self.call(move |c| {
            let added = c.execute(
                "INSERT INTO security_findings \
                     (sandbox_id, fingerprint, file, anchored, claimed_at) \
                     VALUES (?1, ?2, ?3, ?4, ?5) \
                     ON CONFLICT (sandbox_id, fingerprint) DO UPDATE SET \
                     claimed_at = excluded.claimed_at, file = excluded.file, \
                     anchored = excluded.anchored \
                     WHERE security_findings.issue_number IS NULL \
                     AND security_findings.claimed_at < ?6",
                params![
                    sandbox,
                    fingerprint,
                    file,
                    anchored,
                    at.0,
                    at.0.saturating_sub(SECURITY_CLAIM_TTL_MS)
                ],
            )?;
            Ok(added == 1)
        })
        .await
    }

    async fn filed_security_finding(
        &self,
        sandbox: &SandboxId,
        fingerprint: &str,
    ) -> AppResult<Option<FiledFinding>> {
        let (sandbox, fingerprint) = (sandbox.as_str().to_owned(), fingerprint.to_owned());
        self.call(move |c| {
            Ok(c.query_row(
                &format!(
                    "SELECT {FINDING_COLS} FROM security_findings \
                     WHERE sandbox_id = ?1 AND fingerprint = ?2 AND issue_number IS NOT NULL"
                ),
                params![sandbox, fingerprint],
                finding_from_row,
            )
            .optional()?)
        })
        .await
    }

    async fn unresolved_security_findings(
        &self,
        sandbox: &SandboxId,
        limit: u32,
    ) -> AppResult<Vec<FiledFinding>> {
        let sandbox = sandbox.as_str().to_owned();
        self.call(move |c| {
            let mut statement = c.prepare(&format!(
                "SELECT {FINDING_COLS} FROM security_findings \
                     WHERE sandbox_id = ?1 AND issue_number IS NOT NULL AND resolved_at IS NULL \
                     ORDER BY random() LIMIT ?2"
            ))?;
            Ok(statement
                .query_map(params![sandbox, limit], finding_from_row)?
                .collect::<rusqlite::Result<Vec<_>>>()?)
        })
        .await
    }

    async fn forget_security_finding(
        &self,
        sandbox: &SandboxId,
        fingerprint: &str,
        issue: u64,
    ) -> AppResult<()> {
        let (sandbox, fingerprint) = (sandbox.as_str().to_owned(), fingerprint.to_owned());
        let issue = i64::try_from(issue).map_err(AppError::internal)?;
        self.call(move |c| {
            c.execute(
                "DELETE FROM security_findings \
                 WHERE sandbox_id = ?1 AND fingerprint = ?2 AND issue_number = ?3",
                params![sandbox, fingerprint, issue],
            )?;
            Ok(())
        })
        .await
    }

    async fn resolve_security_finding(
        &self,
        sandbox: &SandboxId,
        fingerprint: &str,
        at: Option<Timestamp>,
    ) -> AppResult<()> {
        let (sandbox, fingerprint) = (sandbox.as_str().to_owned(), fingerprint.to_owned());
        self.call(move |c| {
            c.execute(
                "UPDATE security_findings SET resolved_at = ?3 \
                 WHERE sandbox_id = ?1 AND fingerprint = ?2",
                params![sandbox, fingerprint, at.map(|t| t.0)],
            )?;
            Ok(())
        })
        .await
    }

    async fn settle_security_finding(
        &self,
        sandbox: &SandboxId,
        fingerprint: &str,
        issue: Option<u64>,
    ) -> AppResult<()> {
        let (sandbox, fingerprint) = (sandbox.as_str().to_owned(), fingerprint.to_owned());
        let issue = issue
            .map(i64::try_from)
            .transpose()
            .map_err(AppError::internal)?;
        self.call(move |c| {
            match issue {
                Some(number) => c.execute(
                    "UPDATE security_findings SET issue_number = ?3 \
                     WHERE sandbox_id = ?1 AND fingerprint = ?2",
                    params![sandbox, fingerprint, number],
                ),
                None => c.execute(
                    "DELETE FROM security_findings \
                     WHERE sandbox_id = ?1 AND fingerprint = ?2 AND issue_number IS NULL",
                    params![sandbox, fingerprint],
                ),
            }?;
            Ok(())
        })
        .await
    }

    async fn prune_ci_runs(&self, keep: u32, before: Timestamp) -> AppResult<u64> {
        self.call(move |c| {
            let tx = c.transaction()?;
            let prune = "WHERE state NOT IN ('queued', 'running') AND (\
                       queued_at < ?1 OR id IN (\
                         SELECT id FROM (\
                           SELECT id, ROW_NUMBER() OVER (\
                             PARTITION BY sandbox_id ORDER BY queued_at DESC, rowid DESC) AS n \
                           FROM ci_runs) WHERE n > ?2)) \
                     AND id NOT IN (\
                       SELECT id FROM (\
                         SELECT id, ROW_NUMBER() OVER (\
                           PARTITION BY sandbox_id, reference ORDER BY queued_at DESC, rowid DESC) AS v \
                         FROM ci_runs WHERE state IN ('passed', 'failed', 'error') OR (state = 'cancelled' AND reference = 'candidate/' || sha)) WHERE v = 1)";
            let ids = {
                let mut stmt = tx.prepare(&format!("SELECT id FROM ci_runs {prune} AND sandbox_id IN (SELECT id FROM sandboxes) AND (reference LIKE 'branch/%' OR reference LIKE 'candidate/%') AND (state IN ('passed','failed','error') OR (state = 'cancelled' AND reference = 'candidate/' || sha)) AND NOT EXISTS (SELECT 1 FROM ci_jobs WHERE run_id = ci_runs.id AND state = 'skipped' AND instr(reason, ?3) = 1)"))?;
                stmt.query_map(params![before.0, i64::from(keep), yulius_domain::ci::AFTER_MERGE_SKIP_REASON], |row| row.get::<_, String>(0))?.collect::<Result<Vec<_>,_>>()?
            };
            for id in ids {
                let jobs = serde_json::to_string(&gate_jobs(&tx, &id)?).map_err(AppError::internal)?;
                tx.execute(&format!("INSERT INTO archived_gate_verdicts ({RUN_COLS}, sequence, jobs)
                       SELECT {RUN_COLS}, rowid, ?2 FROM ci_runs WHERE id = ?1"), params![id, jobs])?;
            }
            let changed = tx.execute(&format!("DELETE FROM ci_runs {prune}"), params![before.0, i64::from(keep)])?;
            tx.commit()?;
            Ok(u64::try_from(changed).unwrap_or(0))
        })
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    fn run(id: &str, sha: &str, at: i64) -> Result<CiRunRecord, Box<dyn std::error::Error>> {
        Ok(CiRunRecord {
            id: id.into(),
            sandbox_id: SandboxId::parse("s1")?,
            reference: CiRef::PullRequest(3),
            sha: sha.into(),
            source: GateSource::ProjectConfig,
            state: RunState::Queued,
            reason: None,
            requested_by: "yulius".into(),
            status_for: None,
            status_note: None,
            status_attempts: 0,
            queued_at: Timestamp(at),
            started_at: None,
            ended_at: None,
        })
    }

    fn job(id: &str, run: &str, ordinal: u32) -> CiJobRecord {
        CiJobRecord {
            id: id.into(),
            run_id: run.into(),
            ordinal,
            name: format!("job {ordinal}"),
            key: "job".into(),
            command: "true".into(),
            matrix: None,
            state: RunState::Queued,
            reason: None,
            exit_code: None,
            output: String::new(),
            failures: None,
            started_at: None,
            ended_at: None,
            need: Need::default(),
            timeout_minutes: None,
            exclusive: false,
            waited_ms: None,
        }
    }

    /// SPEC-504 review round 1: a run whose source a later release wrote reads as the
    /// integration branch's configuration instead of failing every read of the sandbox's runs.
    #[tokio::test]
    async fn a_run_of_an_unknown_source_still_reads() -> TestResult {
        let store = SqliteStore::open_in_memory()?;
        store
            .insert_ci_run(&run("r1", "a", 1)?, &[job("j1", "r1", 0)])
            .await?;
        store
            .call(|c| {
                c.execute(
                    "UPDATE ci_runs SET source = 'from_a_later_release' WHERE id = 'r1'",
                    [],
                )?;
                Ok(())
            })
            .await?;
        let runs = store.ci_runs(&SandboxId::parse("s1")?, 10).await?;
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].source, GateSource::ProjectConfig);
        Ok(())
    }

    /// SPEC-497 OFC-001, OFC-003 (review round 1): a run that ends adds its minutes to its
    /// day's tally by how it ended: `failed_outside` only when every failed job's end was the
    /// infrastructure's (a job that failed its second run after a thrown-away first counts as
    /// failed), the thrown-away time apart; the tally outlives the runs' prune.
    #[tokio::test]
    async fn a_runs_end_adds_to_its_days_minutes() -> TestResult {
        let store = SqliteStore::open_in_memory()?;
        let min = 60_000;
        // (run, its end, the job's end, the job's infrastructure failure)
        let cases = [
            (
                "passed",
                RunState::Passed,
                RunState::Passed,
                Some(("address_pool", 2 * min, false)),
            ),
            (
                "failed",
                RunState::Failed,
                RunState::Failed,
                Some(("address_pool", min, false)),
            ),
            (
                "outside",
                RunState::Error,
                RunState::Error,
                Some(("docker_socket", 3 * min, true)),
            ),
            ("cancelled", RunState::Cancelled, RunState::Cancelled, None),
            ("never", RunState::Cancelled, RunState::Cancelled, None),
        ];
        for (n, (id, run_end, job_end, infra)) in (0_i64..).zip(cases) {
            let job_id = format!("{id}-job");
            store
                .insert_ci_run(&run(id, id, n * min)?, &[job(&job_id, id, 0)])
                .await?;
            if id == "never" {
                // Cancelled before it started: no minutes.
                store
                    .transition_ci_run(id, &[RunState::Queued], run_end, &CiRunUpdate::default())
                    .await?;
                continue;
            }
            let started = CiRunUpdate {
                started_at: Some(Timestamp(n * min)),
                ..CiRunUpdate::default()
            };
            store
                .transition_ci_run(id, &[RunState::Queued], RunState::Running, &started)
                .await?;
            let job_started = CiJobUpdate {
                started_at: Some(Timestamp(n * min)),
                ..CiJobUpdate::default()
            };
            store
                .transition_ci_job(
                    &job_id,
                    &[RunState::Queued],
                    RunState::Running,
                    &job_started,
                )
                .await?;
            let job_ended = CiJobUpdate {
                ended_at: Some(Timestamp(n * min + 10 * min)),
                infra,
                ..CiJobUpdate::default()
            };
            store
                .transition_ci_job(&job_id, &[RunState::Running], job_end, &job_ended)
                .await?;
            let ended = CiRunUpdate {
                ended_at: Some(Timestamp(n * min + 10 * min)),
                ..CiRunUpdate::default()
            };
            store
                .transition_ci_run(id, &[RunState::Running], run_end, &ended)
                .await?;
        }
        let sandbox = SandboxId::parse("s1")?;
        let read = |store: SqliteStore| {
            let sandbox = sandbox.clone();
            async move {
                let rows = store.ci_minutes(&sandbox, None, None).await?;
                Ok::<_, AppError>(
                    rows.into_iter()
                        .map(|r| (r.day, r.outcome, r.runs, r.run_ms, r.rerun_ms))
                        .collect::<Vec<_>>(),
                )
            }
        };
        let day = "1970-01-01".to_owned();
        let expected = vec![
            (day.clone(), "cancelled".to_owned(), 1, 10 * min, 0),
            (day.clone(), "failed".to_owned(), 1, 10 * min, min),
            (
                day.clone(),
                "failed_outside".to_owned(),
                1,
                10 * min,
                3 * min,
            ),
            (day.clone(), "passed".to_owned(), 1, 10 * min, 2 * min),
        ];
        assert_eq!(read(store.clone()).await?, expected);
        // The runs go; their minutes stay.
        store.prune_ci_runs(0, Timestamp(i64::MAX)).await?;
        assert_eq!(read(store.clone()).await?, expected);
        // The days of the range.
        let none = store
            .ci_minutes(&sandbox, Some(Timestamp(86_400_000)), None)
            .await?;
        assert!(none.is_empty());
        Ok(())
    }

    /// SPEC-497 OFC-002: a test's failures are read by sandbox, job and time and forgotten past
    /// their keep; a test is quarantined once, listed per sandbox or in all, and released.
    #[tokio::test]
    async fn failing_tests_and_the_quarantine_are_kept() -> TestResult {
        let store = SqliteStore::open_in_memory()?;
        let s1 = SandboxId::parse("s1")?;
        let tests = vec!["app tests::flaky".to_owned(), "app tests::other".to_owned()];
        store
            .record_test_failures(
                &s1,
                "quality",
                ("pr/1", "r1", "c1"),
                &tests,
                Timestamp(10),
                Timestamp(0),
            )
            .await?;
        store
            .record_test_failures(
                &s1,
                "quality",
                ("pr/2", "r2", "c2"),
                &tests[..1],
                Timestamp(20),
                Timestamp(0),
            )
            .await?;
        store
            .record_test_failures(
                &s1,
                "e2e",
                ("pr/3", "r3", "c3"),
                &tests[..1],
                Timestamp(30),
                Timestamp(0),
            )
            .await?;
        let failed = store
            .test_failures(&s1, "quality", "app tests::flaky", Timestamp(0))
            .await?;
        let refs: Vec<&str> = failed.iter().map(|f| f.reference.as_str()).collect();
        assert_eq!(refs, ["pr/1", "pr/2"], "only this job's");
        assert_eq!(failed[1].sha, "c2");
        let since = store
            .test_failures(&s1, "quality", "app tests::flaky", Timestamp(15))
            .await?;
        assert_eq!(since.len(), 1);
        // A later record forgets what is older than its keep.
        store
            .record_test_failures(
                &s1,
                "quality",
                ("pr/4", "r4", "c4"),
                &tests[..1],
                Timestamp(40),
                Timestamp(25),
            )
            .await?;
        let kept = store
            .test_failures(&s1, "quality", "app tests::flaky", Timestamp(0))
            .await?;
        assert_eq!(kept.len(), 1, "{kept:?}");
        let entry = Quarantined {
            sandbox_id: s1.clone(),
            test: "app tests::flaky".into(),
            job_key: "quality".into(),
            issue: Some((101, "https://github.test/acme/app/issues/101".into())),
            since: Timestamp(50),
        };
        assert!(store.quarantine_test(&entry).await?);
        assert!(!store.quarantine_test(&entry).await?, "once");
        assert_eq!(
            store.quarantined_tests(Some(&s1)).await?,
            std::slice::from_ref(&entry)
        );
        assert!(
            store
                .quarantined_tests(Some(&SandboxId::parse("s2")?))
                .await?
                .is_empty()
        );
        assert_eq!(store.quarantined_tests(None).await?.len(), 1);
        store
            .record_test_failures(
                &s1,
                "quality",
                ("pr/5", "r5", "c5"),
                &tests,
                Timestamp(45),
                Timestamp(25),
            )
            .await?;
        store.release_test(&s1, "app tests::flaky").await?;
        assert!(store.quarantined_tests(None).await?.is_empty());
        // Its failures are forgotten with it (review round 1); another test's stay.
        assert!(
            store
                .test_failures(&s1, "quality", "app tests::flaky", Timestamp(0))
                .await?
                .is_empty()
        );
        assert_eq!(
            store
                .test_failures(&s1, "quality", "app tests::other", Timestamp(0))
                .await?
                .len(),
            1
        );
        Ok(())
    }

    #[tokio::test]
    async fn a_finishing_ci_run_has_an_executed_pass_and_no_queued_work() -> TestResult {
        use RunState::{Failed, Passed, Queued, Running, Skipped};
        for (run_state, states, eligible) in [
            (Running, vec![Passed, Running], true),
            (Running, vec![Passed, Failed], true), // the executor is committing the red verdict
            (Running, vec![Passed, Running, Skipped], true),
            (Running, vec![Passed, Running, Queued], false),
            (Running, vec![Passed, Running, Running], false),
            (Running, vec![Skipped, Running], false),
            (Queued, vec![Passed, Running], false),
            (Passed, vec![Passed], false),
        ] {
            let store = SqliteStore::open_in_memory()?;
            let mut run = run("r1", "a", 1)?;
            run.state = run_state;
            let jobs = (0_u32..)
                .zip(states.iter())
                .map(|(n, &state)| CiJobRecord {
                    state,
                    started_at: Some(Timestamp(2)),
                    exit_code: (state == Passed).then_some(0),
                    ..job(&format!("j{n}"), &run.id, n)
                })
                .collect::<Vec<_>>();
            store.insert_ci_run(&run, &jobs).await?;
            assert_eq!(
                store.has_finishing_ci_run().await?,
                eligible,
                "{run_state:?} {states:?}"
            );
        }
        Ok(())
    }

    /// SPEC-497 (review round 2): the jobs waiting to start are the queued jobs of running runs;
    /// a queued run's jobs and running jobs are not counted.
    #[tokio::test]
    async fn jobs_waiting_to_start_are_queued_jobs_of_running_runs() -> TestResult {
        let store = SqliteStore::open_in_memory()?;
        store
            .insert_ci_run(
                &run("r1", "a", 1)?,
                &[job("j1", "r1", 0), job("j2", "r1", 1)],
            )
            .await?;
        store
            .insert_ci_run(&run("r2", "b", 2)?, &[job("j3", "r2", 0)])
            .await?;
        assert_eq!(store.ci_jobs_waiting().await?, 0, "no run started");
        let start = CiRunUpdate {
            started_at: Some(Timestamp(5)),
            ..CiRunUpdate::default()
        };
        store
            .transition_ci_run("r1", &[RunState::Queued], RunState::Running, &start)
            .await?;
        assert_eq!(store.ci_jobs_waiting().await?, 2);
        store
            .transition_ci_job(
                "j1",
                &[RunState::Queued],
                RunState::Running,
                &CiJobUpdate::default(),
            )
            .await?;
        assert_eq!(
            store.ci_jobs_waiting().await?,
            1,
            "the running job is not waiting"
        );
        Ok(())
    }

    #[tokio::test]
    async fn runs_jobs_transitions_and_one_running_run_per_sandbox() -> TestResult {
        let store = SqliteStore::open_in_memory()?;
        // What a job declares is kept with it (SPEC-082 RES-002, SPEC-435).
        let declared = CiJobRecord {
            need: Need {
                cpus: Some(6),
                memory_mb: Some(16 * 1024),
            },
            timeout_minutes: Some(120),
            exclusive: true,
            ..job("j2", "r1", 1)
        };
        store
            .insert_ci_run(&run("r1", "a", 1)?, &[job("j1", "r1", 0), declared])
            .await?;
        store
            .insert_ci_run(&run("r2", "b", 2)?, &[job("j3", "r2", 0)])
            .await?;
        let start = CiRunUpdate {
            started_at: Some(Timestamp(5)),
            ..CiRunUpdate::default()
        };
        assert!(
            store
                .transition_ci_run("r1", &[RunState::Queued], RunState::Running, &start)
                .await?
        );
        // A second run of the sandbox may run too (lanes, SPEC-021 CI-106).
        assert!(
            store
                .transition_ci_run("r2", &[RunState::Queued], RunState::Running, &start)
                .await?
        );
        // CAS: not queued any more.
        assert!(
            !store
                .transition_ci_run("r1", &[RunState::Queued], RunState::Cancelled, &start)
                .await?
        );
        // SPEC-435 JTL-004: its wait is recorded with its start and kept with its end.
        assert!(
            store
                .transition_ci_job(
                    "j1",
                    &[RunState::Queued],
                    RunState::Running,
                    &CiJobUpdate {
                        started_at: Some(Timestamp(7)),
                        waited_ms: Some(2_340_000),
                        ..CiJobUpdate::default()
                    }
                )
                .await?
        );
        let done = CiJobUpdate {
            exit_code: Some(0),
            output: Some("ok\n".into()),
            failures: Some("FAIL [ 0.5s] a b\n  stderr ───\n    the message\n".into()),
            ended_at: Some(Timestamp(9)),
            ..CiJobUpdate::default()
        };
        assert!(
            store
                .transition_ci_job("j1", &[RunState::Running], RunState::Passed, &done)
                .await?
        );
        let jobs = store.ci_jobs("r1", true).await?;
        assert_eq!(jobs.len(), 2);
        assert_eq!(jobs[0].state, RunState::Passed);
        assert_eq!(jobs[0].output, "ok\n");
        // SPEC-145: its failing tests' output, read with the output only.
        assert!(
            jobs[0]
                .failures
                .as_deref()
                .is_some_and(|f| f.contains("the message"))
        );
        assert_eq!(jobs[1].failures, None);
        assert_eq!(store.ci_jobs("r1", false).await?[0].failures, None);
        assert_eq!(jobs[0].exit_code, Some(0));
        assert_eq!(jobs[0].need, Need::default());
        assert_eq!(jobs[1].need.cpus, Some(6));
        assert_eq!(jobs[1].need.memory_mb, Some(16 * 1024));
        assert_eq!(jobs[0].waited_ms, Some(2_340_000));
        assert_eq!(
            (
                jobs[0].timeout_minutes,
                jobs[0].exclusive,
                jobs[1].waited_ms
            ),
            (None, false, None)
        );
        assert_eq!(
            (jobs[1].timeout_minutes, jobs[1].exclusive),
            (Some(120), true)
        );
        assert!(store.ci_jobs("r1", false).await?[0].output.is_empty());
        let active = store.active_ci_runs().await?;
        assert_eq!(
            active.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(),
            ["r1", "r2"]
        );
        // SPEC-059 CI-301: a queue note (waiting for disk) goes once the run starts.
        store
            .insert_ci_run(&run("r3", "c", 3)?, &[job("j4", "r3", 0)])
            .await?;
        let note = CiRunUpdate {
            reason: Some("waiting for disk space".into()),
            ..CiRunUpdate::default()
        };
        assert!(
            store
                .transition_ci_run("r3", &[RunState::Queued], RunState::Queued, &note)
                .await?
        );
        assert_eq!(
            store.ci_run("r3").await?.ok_or("r3")?.reason.as_deref(),
            Some("waiting for disk space")
        );
        assert!(
            store
                .transition_ci_run(
                    "r3",
                    &[RunState::Queued],
                    RunState::Running,
                    &CiRunUpdate::default()
                )
                .await?
        );
        assert_eq!(store.ci_run("r3").await?.ok_or("r3")?.reason, None);
        // A restart puts the running run and its jobs back.
        assert!(
            store
                .requeue_ci_run(
                    "r1",
                    "[yulius] queued again after a restart of Yulius\n",
                    Timestamp(5)
                )
                .await?
        );
        let again = store.ci_run("r1").await?.ok_or("r1")?;
        assert_eq!(again.state, RunState::Queued);
        assert_eq!(again.started_at, None);
        assert!(
            store
                .ci_jobs("r1", true)
                .await?
                .iter()
                .all(|j| j.state == RunState::Queued
                    && j.output.contains("restart")
                    && j.failures.is_none()
                    && j.waited_ms.is_none())
        );
        Ok(())
    }

    /// The newest verdict of a reference survives pruning, however old (F 2026-10-13: without it
    /// the planner tested the main head again and its status went back to pending).
    #[tokio::test]
    async fn retention_keeps_each_references_newest_verdict() -> TestResult {
        let store = SqliteStore::open_in_memory()?;
        let start = CiRunUpdate {
            started_at: Some(Timestamp(1)),
            ..CiRunUpdate::default()
        };
        let main = |id: &str, at: i64| -> Result<CiRunRecord, Box<dyn std::error::Error>> {
            Ok(CiRunRecord {
                reference: CiRef::Branch("main".into()),
                ..run(id, id, at)?
            })
        };
        for (id, at) in [("m0", 1), ("m1", 2)] {
            store.insert_ci_run(&main(id, at)?, &[]).await?;
            store
                .transition_ci_run(id, &[RunState::Queued], RunState::Running, &start)
                .await?;
            store
                .transition_ci_run(id, &[RunState::Running], RunState::Passed, &start)
                .await?;
        }
        for i in 0..3 {
            let id = format!("p{i}");
            store.insert_ci_run(&run(&id, &id, 10 + i)?, &[]).await?;
        }
        // Keep the newest 2 and everything since 100: only main's older run goes.
        assert_eq!(store.prune_ci_runs(2, Timestamp(100)).await?, 1);
        assert!(store.ci_run("m1").await?.is_some(), "main's verdict stays");
        assert!(store.ci_run("m0").await?.is_none());
        Ok(())
    }

    /// SPEC-217 VSI-002: a fingerprint is claimed once per sandbox; a failed filing releases it,
    /// a filed one stays.
    #[tokio::test]
    async fn security_findings_are_claimed_once_per_sandbox() -> TestResult {
        let store = SqliteStore::open_in_memory()?;
        let (one, two) = (SandboxId::parse("s1")?, SandboxId::parse("s2")?);
        let (at, print) = (Timestamp(5), "0123456789abcdef");
        let finding = (print, "src/a.rs", true);
        assert!(store.claim_security_finding(&one, finding, at).await?);
        assert!(!store.claim_security_finding(&one, finding, at).await?);
        assert!(
            store.claim_security_finding(&two, finding, at).await?,
            "per sandbox"
        );
        assert_eq!(
            store.filed_security_finding(&one, print).await?,
            None,
            "not filed yet"
        );
        assert!(
            store
                .claim_security_finding(&two, finding, Timestamp(5 + 60 * 60 * 1000 + 1))
                .await?,
            "a claim left without an issue is taken over after an hour"
        );
        store.settle_security_finding(&one, print, None).await?;
        assert!(
            store.claim_security_finding(&one, finding, at).await?,
            "released"
        );
        store.settle_security_finding(&one, print, Some(42)).await?;
        store.settle_security_finding(&one, print, None).await?;
        assert!(
            !store.claim_security_finding(&one, finding, at).await?,
            "filed stays"
        );
        let later = Timestamp(at.0 + 2 * 60 * 60 * 1000);
        assert!(
            !store.claim_security_finding(&one, finding, later).await?,
            "a filed finding is never taken over"
        );
        let filed = FiledFinding {
            fingerprint: print.into(),
            file: "src/a.rs".into(),
            anchored: true,
            issue: 42,
            resolved: false,
        };
        assert_eq!(
            store.filed_security_finding(&one, print).await?,
            Some(filed.clone())
        );
        assert_eq!(
            store.unresolved_security_findings(&one, 10).await?,
            std::slice::from_ref(&filed)
        );
        store
            .resolve_security_finding(&one, print, Some(Timestamp(9)))
            .await?;
        assert_eq!(store.unresolved_security_findings(&one, 10).await?, []);
        assert!(
            store
                .filed_security_finding(&one, print)
                .await?
                .is_some_and(|f| f.resolved)
        );
        store.resolve_security_finding(&one, print, None).await?;
        assert_eq!(store.unresolved_security_findings(&one, 10).await?, [filed]);
        // Another run already filed it anew as #43: forgetting the stale #42 keeps that record.
        store.forget_security_finding(&one, print, 42).await?;
        assert!(
            store.claim_security_finding(&one, finding, at).await?,
            "the issue-42 record is forgotten"
        );
        store.settle_security_finding(&one, print, Some(43)).await?;
        store.forget_security_finding(&one, print, 42).await?;
        assert!(
            store
                .filed_security_finding(&one, print)
                .await?
                .is_some_and(|f| f.issue == 43),
            "a stale forget keeps the newer record"
        );
        store.forget_security_finding(&one, print, 43).await?;
        assert_eq!(store.filed_security_finding(&one, print).await?, None);
        assert!(
            store.claim_security_finding(&one, finding, at).await?,
            "a forgotten finding is claimed anew"
        );
        Ok(())
    }

    #[tokio::test]
    async fn the_scheduler_keeps_the_latest_admissions_even_when_they_inherit_an_old_place()
    -> TestResult {
        let store = SqliteStore::open_in_memory()?;
        let sandbox = SandboxId::parse("s1")?;
        for (id, place) in [("old", 20), ("pass", 30), ("refusal", 2)] {
            store.insert_ci_run(&run(id, "head", place)?, &[]).await?;
        }
        let planning = store.ci_planning_runs(&sandbox, 2, None).await?;
        let ids: Vec<_> = planning.iter().map(|run| run.id.as_str()).collect();
        assert_eq!(ids, ["refusal", "pass"]);
        assert_eq!(store.ci_runs(&sandbox, 1).await?[0].id, "pass");
        Ok(())
    }

    #[tokio::test]
    async fn statuses_retention_and_lookups() -> TestResult {
        let store = SqliteStore::open_in_memory()?;
        for (i, at) in (1..=5).enumerate() {
            let id = format!("r{i}");
            store
                .insert_ci_run(
                    &run(&id, &format!("s{i}"), at)?,
                    &[job(&format!("j{i}"), &id, 0)],
                )
                .await?;
            let end = CiRunUpdate {
                ended_at: Some(Timestamp(at + 1)),
                ..CiRunUpdate::default()
            };
            store
                .transition_ci_run(&id, &[RunState::Queued], RunState::Passed, &end)
                .await?;
        }
        assert_eq!(
            store.unpublished_ci_runs(Timestamp(0), 5, 10).await?.len(),
            5
        );
        store
            .record_ci_status("r0", Some(RunState::Passed), None)
            .await?;
        store
            .record_ci_status("r1", None, Some("not published: x"))
            .await?;
        let pending = store.unpublished_ci_runs(Timestamp(0), 5, 10).await?;
        assert_eq!(pending.len(), 4);
        assert_eq!(
            store
                .ci_run("r1")
                .await?
                .ok_or("r1")?
                .status_note
                .as_deref(),
            Some("not published: x")
        );
        let sandbox = SandboxId::parse("s1")?;
        assert_eq!(store.ci_runs_for_sha(&sandbox, "s3").await?.len(), 1);
        // Keep the newest 2; everything queued before 2 goes too.
        assert_eq!(store.prune_ci_runs(2, Timestamp(2)).await?, 3);
        let left: Vec<String> = store
            .ci_runs(&sandbox, 10)
            .await?
            .into_iter()
            .map(|r| r.id)
            .collect();
        assert_eq!(left, ["r4", "r3"]);
        // Jobs of pruned runs go with them.
        assert!(store.ci_jobs("r0", false).await?.is_empty());
        Ok(())
    }
}
