//! Yulius-run CI runs and jobs (SPEC-002 ADR-21, tables `ci_runs` and `ci_jobs`).

use async_trait::async_trait;
use rusqlite::{OptionalExtension, Row, params, params_from_iter};
use yulius_app::ci::{CiJobRecord, CiJobUpdate, CiRunRecord, CiRunUpdate, FiledFinding};
use yulius_app::error::{AppError, AppResult};
use yulius_app::ports::store_ext::CiStore;
use yulius_domain::ci::{CiRef, GateSource, RunState};
use yulius_domain::ids::SandboxId;
use yulius_domain::resources::Need;
use yulius_domain::time::Timestamp;

use crate::{SqliteStore, bad_column, db, to_u32};

const RUN_COLS: &str = "id, sandbox_id, reference, sha, source, state, reason, requested_by, \
    status_for, status_note, status_attempts, queued_at, started_at, ended_at";
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
    exit_code, started_at, ended_at, cpus, memory_mb";

fn state(text: &str) -> rusqlite::Result<RunState> {
    RunState::parse(text).ok_or_else(|| bad_column("CI state", text))
}

fn run_from_row(row: &Row<'_>) -> rusqlite::Result<CiRunRecord> {
    let sandbox: String = row.get("sandbox_id")?;
    let reference: String = row.get("reference")?;
    let source: String = row.get("source")?;
    let run_state: String = row.get("state")?;
    let status_for: Option<String> = row.get("status_for")?;
    Ok(CiRunRecord {
        id: row.get("id")?,
        sandbox_id: SandboxId::parse(&sandbox).map_err(|_| bad_column("sandbox id", &sandbox))?,
        reference: CiRef::parse(&reference)
            .ok_or_else(|| bad_column("CI reference", &reference))?,
        sha: row.get("sha")?,
        source: GateSource::parse(&source).ok_or_else(|| bad_column("CI source", &source))?,
        state: state(&run_state)?,
        reason: row.get("reason")?,
        requested_by: row.get("requested_by")?,
        status_for: status_for.as_deref().map(state).transpose()?,
        status_note: row.get("status_note")?,
        status_attempts: to_u32(row.get("status_attempts")?),
        queued_at: Timestamp(row.get("queued_at")?),
        started_at: row.get::<_, Option<i64>>("started_at")?.map(Timestamp),
        ended_at: row.get::<_, Option<i64>>("ended_at")?.map(Timestamp),
    })
}

fn job_from_row(row: &Row<'_>, with_output: bool) -> rusqlite::Result<CiJobRecord> {
    let job_state: String = row.get("state")?;
    Ok(CiJobRecord {
        id: row.get("id")?,
        run_id: row.get("run_id")?,
        ordinal: to_u32(row.get("ordinal")?),
        name: row.get("name")?,
        key: row.get("job_key")?,
        command: row.get("command")?,
        matrix: row.get("matrix")?,
        state: state(&job_state)?,
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
        started_at: row.get::<_, Option<i64>>("started_at")?.map(Timestamp),
        ended_at: row.get::<_, Option<i64>>("ended_at")?.map(Timestamp),
        need: Need {
            cpus: row
                .get::<_, Option<i64>>("cpus")?
                .and_then(|n| u32::try_from(n).ok()),
            memory_mb: row
                .get::<_, Option<i64>>("memory_mb")?
                .and_then(|n| u64::try_from(n).ok()),
        },
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
) -> AppResult<Vec<CiRunRecord>> {
    let mut stmt = c
        .prepare(&format!("SELECT {RUN_COLS} FROM ci_runs WHERE {filter}"))
        .map_err(db)?;
    let rows = stmt
        .query_map(args, run_from_row)
        .map_err(db)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(db)?;
    Ok(rows)
}

#[async_trait]
impl CiStore for SqliteStore {
    async fn insert_ci_run(&self, run: &CiRunRecord, jobs: &[CiJobRecord]) -> AppResult<()> {
        let (run, jobs) = (run.clone(), jobs.to_vec());
        self.call(move |c| {
            let tx = c.transaction().map_err(db)?;
            tx.execute(
                &format!(
                    "INSERT INTO ci_runs ({RUN_COLS}) VALUES ({})",
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
            )
            .map_err(db)?;
            for job in &jobs {
                tx.execute(
                    &format!(
                        "INSERT INTO ci_jobs ({JOB_COLS}, output) VALUES ({})",
                        placeholders(1, 15)
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
                        job.output,
                    ],
                )
                .map_err(db)?;
            }
            tx.commit().map_err(db)
        })
        .await
    }

    async fn ci_run(&self, id: &str) -> AppResult<Option<CiRunRecord>> {
        let id = id.to_owned();
        self.call(move |c| {
            c.query_row(
                &format!("SELECT {RUN_COLS} FROM ci_runs WHERE id = ?1"),
                [id],
                run_from_row,
            )
            .optional()
            .map_err(db)
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
            let mut stmt = c
                .prepare(&format!(
                    "SELECT {cols} FROM ci_jobs WHERE run_id = ?1 ORDER BY ordinal"
                ))
                .map_err(db)?;
            let rows = stmt
                .query_map([run], |row| job_from_row(row, with_output))
                .map_err(db)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(db)?;
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

    async fn transition_ci_run(
        &self,
        id: &str,
        from: &[RunState],
        to: RunState,
        update: &CiRunUpdate,
    ) -> AppResult<bool> {
        let (id, from, update) = (id.to_owned(), states(from), update.clone());
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
            match c.execute(&sql, params_from_iter(args.iter().map(AsRef::as_ref))) {
                Ok(changed) => Ok(changed == 1),
                Err(rusqlite::Error::SqliteFailure(err, _))
                    if err.code == rusqlite::ErrorCode::ConstraintViolation =>
                {
                    Err(AppError::Conflict(
                        "a constraint refused the CI run's new state".into(),
                    ))
                }
                Err(err) => Err(db(err)),
            }
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
                 disk_written_mb = COALESCE(?11, disk_written_mb) \
                 WHERE id = ?7 AND state IN ({})",
                placeholders(12, from.len())
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
            ];
            args.extend(
                from.into_iter()
                    .map(|s| Box::new(s) as Box<dyn rusqlite::ToSql>),
            );
            let changed = c
                .execute(&sql, params_from_iter(args.iter().map(AsRef::as_ref)))
                .map_err(db)?;
            Ok(changed == 1)
        })
        .await
    }

    async fn set_ci_job_output(&self, id: &str, output: &str) -> AppResult<()> {
        let (id, output) = (id.to_owned(), output.to_owned());
        self.call(move |c| {
            c.execute(
                "UPDATE ci_jobs SET output = ?2 WHERE id = ?1 AND state = 'running'",
                params![id, output],
            )
            .map_err(db)?;
            Ok(())
        })
        .await
    }

    async fn requeue_ci_run(&self, id: &str, note: &str, at: Timestamp) -> AppResult<bool> {
        let (id, note) = (id.to_owned(), note.to_owned());
        self.call(move |c| {
            let tx = c.transaction().map_err(db)?;
            let changed = tx
                .execute(
                    "UPDATE ci_runs SET state = 'queued', started_at = NULL, ended_at = NULL, \
                     reason = NULL, requeued_at = ?2 WHERE id = ?1 AND state = 'running'",
                    params![id, at.0],
                )
                .map_err(db)?;
            if changed == 1 {
                tx.execute(
                    "UPDATE ci_jobs SET state = 'queued', started_at = NULL, ended_at = NULL, \
                     reason = NULL, exit_code = NULL, output = ?2, failures = NULL, cpu_ms = NULL, \
                     memory_peak_mb = NULL, disk_written_mb = NULL WHERE run_id = ?1",
                    [&id, &note],
                )
                .map_err(db)?;
            }
            tx.commit().map_err(db)?;
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
            }
            .map_err(db)?;
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
            let added = c
                .execute(
                    "INSERT OR IGNORE INTO security_findings \
                     (sandbox_id, fingerprint, file, anchored, claimed_at) \
                     VALUES (?1, ?2, ?3, ?4, ?5)",
                    params![sandbox, fingerprint, file, anchored, at.0],
                )
                .map_err(db)?;
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
            c.query_row(
                &format!(
                    "SELECT {FINDING_COLS} FROM security_findings \
                     WHERE sandbox_id = ?1 AND fingerprint = ?2 AND issue_number IS NOT NULL"
                ),
                params![sandbox, fingerprint],
                finding_from_row,
            )
            .optional()
            .map_err(db)
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
            let mut statement = c
                .prepare(&format!(
                    "SELECT {FINDING_COLS} FROM security_findings \
                     WHERE sandbox_id = ?1 AND issue_number IS NOT NULL AND resolved_at IS NULL \
                     ORDER BY claimed_at, fingerprint LIMIT ?2"
                ))
                .map_err(db)?;
            statement
                .query_map(params![sandbox, limit], finding_from_row)
                .map_err(db)?
                .collect::<rusqlite::Result<Vec<_>>>()
                .map_err(db)
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
            )
            .map_err(db)?;
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
            }
            .map_err(db)?;
            Ok(())
        })
        .await
    }

    async fn prune_ci_runs(&self, keep: u32, before: Timestamp) -> AppResult<u64> {
        self.call(move |c| {
            let changed = c
                .execute(
                    // The newest verdict of every reference (a branch, a pull request) is kept,
                    // however many runs came since: without it the planner tests that head again
                    // and its commit status goes back to pending (F 2026-10-13, main re-queued).
                    "DELETE FROM ci_runs WHERE state NOT IN ('queued', 'running') AND (\
                       queued_at < ?1 OR id IN (\
                         SELECT id FROM (\
                           SELECT id, ROW_NUMBER() OVER (\
                             PARTITION BY sandbox_id ORDER BY queued_at DESC, rowid DESC) AS n \
                           FROM ci_runs) WHERE n > ?2)) \
                     AND id NOT IN (\
                       SELECT id FROM (\
                         SELECT id, ROW_NUMBER() OVER (\
                           PARTITION BY sandbox_id, reference ORDER BY queued_at DESC, rowid DESC) AS v \
                         FROM ci_runs WHERE state IN ('passed', 'failed', 'error')) WHERE v = 1)",
                    params![before.0, i64::from(keep)],
                )
                .map_err(db)?;
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
        }
    }

    #[tokio::test]
    async fn runs_jobs_transitions_and_one_running_run_per_sandbox() -> TestResult {
        let store = SqliteStore::open_in_memory()?;
        // What a job declares is kept with it (SPEC-082 RES-002).
        let declared = CiJobRecord {
            need: Need {
                cpus: Some(6),
                memory_mb: Some(16 * 1024),
            },
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
        let done = CiJobUpdate {
            exit_code: Some(0),
            output: Some("ok\n".into()),
            failures: Some("FAIL [ 0.5s] a b\n  stderr ───\n    the message\n".into()),
            ended_at: Some(Timestamp(9)),
            ..CiJobUpdate::default()
        };
        assert!(
            store
                .transition_ci_job("j1", &[RunState::Queued], RunState::Passed, &done)
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
                    && j.failures.is_none())
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
