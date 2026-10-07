//! Analytics over visible sandboxes (SPEC-001 ANL-001..003): summary aggregates and a per-task
//! export (JSON or CSV).
//!
//! The date range selects a cohort of tasks by creation time (UTC days, both ends inclusive);
//! every figure describes that cohort. Wall durations come from attempts, active time and
//! tokens from per-attempt metrics. Known values are summed; runs that reported no usage are
//! counted in `unknown_usage_runs` and never added as zero (ROT-011).

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fmt::Write as _;

use yulius_contracts::api::analytics::{
    AccountTokens, AnalyticsRange, AnalyticsSummary, DayCount, DurationStats, FeedbackCounts,
    HarnessTokens, ModelTokens, SwitchCounts, TASK_ROW_COLUMNS, TaskCounts, TaskRow, TokenTotals,
};
use yulius_domain::harness::Harness;
use yulius_domain::ids::{AccountId, SandboxId, TaskId};
use yulius_domain::permission::Action;
use yulius_domain::task::TaskStatus;
use yulius_domain::time::Timestamp;

use crate::auth::{AuthContext, civil_from_days, rfc3339};
use crate::context::App;
use crate::error::{AppError, AppResult};
use crate::model::{MetricsRecord, TaskOrigin};

/// Most tasks one analytics request reads (the newest are kept).
pub const MAX_ANALYTICS_TASKS: u32 = 50_000;
/// Longest range (days) whose per-day series is filled with zero-task days.
pub const MAX_FILLED_DAYS: i64 = 366;
const DAY_MS: i64 = 86_400_000;

/// What the store reads for analytics.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnalyticsQuery {
    /// Sandboxes to include (empty → nothing).
    pub sandboxes: Vec<SandboxId>,
    /// Tasks created at or after this time.
    pub from: Option<Timestamp>,
    /// Tasks created before this time.
    pub to: Option<Timestamp>,
    /// Maximum tasks (newest first).
    pub limit: u32,
}

/// Task facts used by analytics.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnalyticsTask {
    /// Id.
    pub id: TaskId,
    /// Sandbox.
    pub sandbox_id: SandboxId,
    /// Sandbox slug.
    pub sandbox_slug: String,
    /// Number.
    pub number: u32,
    /// Title.
    pub title: String,
    /// Current harness.
    pub harness: Harness,
    /// Status.
    pub status: TaskStatus,
    /// Origin.
    pub origin: TaskOrigin,
    /// Created.
    pub created_at: Timestamp,
    /// Closed.
    pub closed_at: Option<Timestamp>,
}

/// One attempt with its metrics.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnalyticsRun {
    /// Task.
    pub task_id: TaskId,
    /// Sequence in the task.
    pub seq: u32,
    /// Harness.
    pub harness: Harness,
    /// Account.
    pub account_id: Option<AccountId>,
    /// Account label (`None` when deleted).
    pub account_label: Option<String>,
    /// Started.
    pub started_at: Timestamp,
    /// Ended (`None` while live).
    pub ended_at: Option<Timestamp>,
    /// Metrics row (`None` when never recorded).
    pub metrics: Option<MetricsRecord>,
}

/// One recorded switch decision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnalyticsSwitch {
    /// Task.
    pub task_id: TaskId,
    /// Time.
    pub at: Timestamp,
    /// Decision inputs JSON (`{"harness": target, ...}`).
    pub inputs: String,
}

/// One feedback verdict.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnalyticsFeedback {
    /// Task.
    pub task_id: TaskId,
    /// `correct` or `incorrect`.
    pub verdict: String,
    /// Time.
    pub at: Timestamp,
}

/// Everything analytics needs about a cohort.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AnalyticsData {
    /// Tasks, oldest first.
    pub tasks: Vec<AnalyticsTask>,
    /// Their attempts, by task then sequence.
    pub runs: Vec<AnalyticsRun>,
    /// Their switch decisions.
    pub switches: Vec<AnalyticsSwitch>,
    /// Their feedback.
    pub feedback: Vec<AnalyticsFeedback>,
    /// More tasks matched than [`AnalyticsQuery::limit`].
    pub truncated: bool,
}

/// Token accumulator over runs: sums known values, counts runs without usage.
#[derive(Default)]
struct Acc {
    runs: u32,
    tasks: BTreeSet<TaskId>,
    input: Option<u64>,
    output: Option<u64>,
    cache_read: Option<u64>,
    cache_write: Option<u64>,
    unknown: u32,
}

fn add_known(acc: &mut Option<u64>, value: Option<u64>) {
    if let Some(v) = value {
        *acc = Some(acc.unwrap_or(0).saturating_add(v));
    }
}

fn has_usage(metrics: Option<&MetricsRecord>) -> bool {
    metrics.is_some_and(|m| {
        m.input_tokens.is_some()
            || m.output_tokens.is_some()
            || m.cache_read.is_some()
            || m.cache_write.is_some()
    })
}

impl Acc {
    fn add(&mut self, run: &AnalyticsRun) {
        self.runs = self.runs.saturating_add(1);
        self.tasks.insert(run.task_id.clone());
        match run.metrics.as_ref().filter(|m| has_usage(Some(m))) {
            None => self.unknown = self.unknown.saturating_add(1),
            Some(m) => {
                add_known(&mut self.input, m.input_tokens);
                add_known(&mut self.output, m.output_tokens);
                add_known(&mut self.cache_read, m.cache_read);
                add_known(&mut self.cache_write, m.cache_write);
            }
        }
    }

    fn totals(&self) -> TokenTotals {
        TokenTotals {
            runs: self.runs,
            tasks: u32::try_from(self.tasks.len()).unwrap_or(u32::MAX),
            input: self.input,
            output: self.output,
            cache_read: self.cache_read,
            cache_write: self.cache_write,
            unknown_usage_runs: self.unknown,
        }
    }
}

fn run_wall_ms(run: &AnalyticsRun, now: Timestamp) -> u64 {
    u64::try_from(
        run.ended_at
            .unwrap_or(now)
            .millis_since(run.started_at)
            .max(0),
    )
    .unwrap_or(0)
}

fn day_of(ts: Timestamp) -> i64 {
    ts.0.div_euclid(DAY_MS)
}

fn day_string(day: i64) -> String {
    let (y, m, d) = civil_from_days(day);
    format!("{y:04}-{m:02}-{d:02}")
}

/// Days since 1970-01-01 for a civil date (inverse of [`civil_from_days`]).
fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400);
    let mp = i64::from((m + 9) % 12);
    let doy = (153 * mp + 2) / 5 + i64::from(d) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// Parses `YYYY-MM-DD` (years 1970–9999) into days since the epoch.
///
/// # Errors
/// `Invalid` for anything else, including impossible dates.
pub fn parse_day(text: &str) -> AppResult<i64> {
    let invalid = || AppError::Invalid(format!("{text:?} is not a YYYY-MM-DD date"));
    let bytes = text.as_bytes();
    if bytes.len() != 10 || bytes[4] != b'-' || bytes[7] != b'-' {
        return Err(invalid());
    }
    let num = |s: &str| -> AppResult<u32> {
        if s.bytes().all(|b| b.is_ascii_digit()) {
            s.parse::<u32>().map_err(|_| invalid())
        } else {
            Err(invalid())
        }
    };
    let (y, m, d) = (num(&text[..4])?, num(&text[5..7])?, num(&text[8..])?);
    if !(1970..=9999).contains(&y) || !(1..=12).contains(&m) || d == 0 {
        return Err(invalid());
    }
    let days = days_from_civil(i64::from(y), m, d);
    if civil_from_days(days) != (i64::from(y), m, d) {
        return Err(invalid());
    }
    Ok(days)
}

/// Median (mean of the two middle values for even counts) and nearest-rank p90 of `values`.
fn median_p90(values: &mut [u64]) -> (Option<u64>, Option<u64>) {
    if values.is_empty() {
        return (None, None);
    }
    values.sort_unstable();
    let n = values.len();
    let median = if n % 2 == 1 {
        values[n / 2]
    } else {
        values[n / 2 - 1].midpoint(values[n / 2])
    };
    let rank = (n * 9).div_ceil(10).max(1);
    (Some(median), Some(values[rank - 1]))
}

fn target_harness(inputs: &str) -> Option<Harness> {
    serde_json::from_str::<serde_json::Value>(inputs)
        .ok()?
        .get("harness")?
        .as_str()
        .and_then(|h| Harness::parse(h).ok())
}

/// Classifies switches as account (same harness) or provider (other harness) switches. The
/// source harness is the task's latest attempt started at or before the decision.
fn switch_counts(data: &AnalyticsData) -> (SwitchCounts, HashMap<TaskId, u32>) {
    let mut counts = SwitchCounts::default();
    let mut per_task: HashMap<TaskId, u32> = HashMap::new();
    for s in &data.switches {
        counts.total = counts.total.saturating_add(1);
        *per_task.entry(s.task_id.clone()).or_default() += 1;
        let from = data
            .runs
            .iter()
            .filter(|r| r.task_id == s.task_id && r.started_at <= s.at)
            .max_by_key(|r| (r.started_at, r.seq))
            .map(|r| r.harness);
        match (from, target_harness(&s.inputs)) {
            (Some(a), Some(b)) if a != b => counts.provider = counts.provider.saturating_add(1),
            _ => counts.account = counts.account.saturating_add(1),
        }
    }
    (counts, per_task)
}

fn bump(map: &mut BTreeMap<String, u32>, key: &str) {
    *map.entry(key.to_owned()).or_default() += 1;
}

/// Aggregates a cohort.
#[must_use]
pub fn summarize(
    data: &AnalyticsData,
    range: AnalyticsRange,
    days: Option<(i64, i64)>,
    now: Timestamp,
) -> AnalyticsSummary {
    let mut tasks = TaskCounts::default();
    let mut per_day: BTreeMap<i64, u32> = BTreeMap::new();
    for t in &data.tasks {
        tasks.total = tasks.total.saturating_add(1);
        if t.status == TaskStatus::Closed {
            tasks.closed = tasks.closed.saturating_add(1);
        }
        bump(&mut tasks.by_status, t.status.as_str());
        bump(&mut tasks.by_harness, t.harness.as_str());
        bump(&mut tasks.by_origin, t.origin.as_str());
        *per_day.entry(day_of(t.created_at)).or_default() += 1;
    }
    if let Some((first, last)) = days
        && last >= first
        && last - first < MAX_FILLED_DAYS
    {
        for day in first..=last {
            per_day.entry(day).or_default();
        }
    }

    let mut all = Acc::default();
    let mut by_model: BTreeMap<String, Acc> = BTreeMap::new();
    let mut by_harness: BTreeMap<String, Acc> = BTreeMap::new();
    let mut by_account: BTreeMap<Option<String>, (Option<String>, Acc)> = BTreeMap::new();
    let mut wall_by_task: HashMap<TaskId, u64> = HashMap::new();
    let mut total_wall: u64 = 0;
    let mut total_active: Option<u64> = None;
    for run in &data.runs {
        all.add(run);
        let model = run
            .metrics
            .as_ref()
            .and_then(|m| m.model.clone())
            .unwrap_or_else(|| "unknown".into());
        by_model.entry(model).or_default().add(run);
        by_harness
            .entry(run.harness.as_str().to_owned())
            .or_default()
            .add(run);
        let account = by_account
            .entry(run.account_id.as_ref().map(ToString::to_string))
            .or_insert_with(|| (run.account_label.clone(), Acc::default()));
        account.1.add(run);
        let wall = run_wall_ms(run, now);
        total_wall = total_wall.saturating_add(wall);
        let task_wall = wall_by_task.entry(run.task_id.clone()).or_default();
        *task_wall = task_wall.saturating_add(wall);
        add_known(
            &mut total_active,
            run.metrics.as_ref().and_then(|m| m.active_ms),
        );
    }
    let mut walls: Vec<u64> = wall_by_task.into_values().collect();
    let (median, p90) = median_p90(&mut walls);
    let (switches, _) = switch_counts(data);
    let mut feedback = FeedbackCounts::default();
    for f in &data.feedback {
        match f.verdict.as_str() {
            "correct" => feedback.correct = feedback.correct.saturating_add(1),
            "incorrect" => feedback.incorrect = feedback.incorrect.saturating_add(1),
            _ => {}
        }
    }
    AnalyticsSummary {
        range,
        tasks,
        tasks_per_day: per_day
            .into_iter()
            .map(|(day, count)| DayCount {
                date: day_string(day),
                count,
            })
            .collect(),
        duration_ms: DurationStats {
            median,
            p90,
            total_wall,
            total_active,
        },
        tokens: all.totals(),
        tokens_by_model: by_model
            .into_iter()
            .map(|(model, acc)| ModelTokens {
                model,
                tokens: acc.totals(),
            })
            .collect(),
        tokens_by_harness: by_harness
            .into_iter()
            .map(|(harness, acc)| HarnessTokens {
                harness,
                tokens: acc.totals(),
            })
            .collect(),
        tokens_by_account: by_account
            .into_iter()
            .map(|(id, (label, acc))| AccountTokens {
                account_id: id,
                account_label: label,
                tokens: acc.totals(),
            })
            .collect(),
        switches,
        feedback,
        truncated: data.truncated,
    }
}

fn push_distinct(list: &mut Vec<String>, value: &str) {
    if !list.iter().any(|v| v == value) {
        list.push(value.to_owned());
    }
}

/// One export row per task, oldest first.
#[must_use]
pub fn task_rows(data: &AnalyticsData, now: Timestamp) -> Vec<TaskRow> {
    let (_, switches) = switch_counts(data);
    let mut runs_by_task: HashMap<&TaskId, Vec<&AnalyticsRun>> = HashMap::new();
    for run in &data.runs {
        runs_by_task.entry(&run.task_id).or_default().push(run);
    }
    let mut feedback_by_task: HashMap<&TaskId, Vec<&AnalyticsFeedback>> = HashMap::new();
    for f in &data.feedback {
        feedback_by_task.entry(&f.task_id).or_default().push(f);
    }
    data.tasks
        .iter()
        .map(|t| {
            let mut runs = runs_by_task.remove(&t.id).unwrap_or_default();
            runs.sort_by_key(|r| r.seq);
            let mut acc = Acc::default();
            let (mut harnesses, mut models, mut accounts) = (Vec::new(), Vec::new(), Vec::new());
            let mut wall: u64 = 0;
            let mut active: Option<u64> = None;
            for run in &runs {
                acc.add(run);
                push_distinct(&mut harnesses, run.harness.as_str());
                if let Some(model) = run.metrics.as_ref().and_then(|m| m.model.as_deref()) {
                    push_distinct(&mut models, model);
                }
                if let Some(account) = run
                    .account_label
                    .clone()
                    .or_else(|| run.account_id.as_ref().map(ToString::to_string))
                {
                    push_distinct(&mut accounts, &account);
                }
                wall = wall.saturating_add(run_wall_ms(run, now));
                add_known(&mut active, run.metrics.as_ref().and_then(|m| m.active_ms));
            }
            let feedback = feedback_by_task.remove(&t.id).unwrap_or_default();
            let count = |verdict: &str| {
                u32::try_from(feedback.iter().filter(|f| f.verdict == verdict).count())
                    .unwrap_or(u32::MAX)
            };
            let totals = acc.totals();
            TaskRow {
                task_id: t.id.to_string(),
                sandbox: t.sandbox_slug.clone(),
                number: t.number,
                title: t.title.clone(),
                status: t.status.as_str().to_owned(),
                origin: t.origin.as_str().to_owned(),
                harness: runs
                    .first()
                    .map_or(t.harness, |r| r.harness)
                    .as_str()
                    .to_owned(),
                harnesses: harnesses.join(";"),
                models: models.join(";"),
                accounts: accounts.join(";"),
                created_at: rfc3339(t.created_at),
                closed_at: t.closed_at.map(rfc3339),
                attempts: totals.runs,
                wall_ms: wall,
                active_ms: active,
                input_tokens: totals.input,
                output_tokens: totals.output,
                cache_read_tokens: totals.cache_read,
                cache_write_tokens: totals.cache_write,
                unknown_usage_runs: totals.unknown_usage_runs,
                switches: switches.get(&t.id).copied().unwrap_or(0),
                feedback: feedback
                    .iter()
                    .max_by_key(|f| f.at)
                    .map(|f| f.verdict.clone()),
                feedback_correct: count("correct"),
                feedback_incorrect: count("incorrect"),
            }
        })
        .collect()
}

/// Escapes one CSV text cell (RFC 4180 quoting). Cells that a spreadsheet would evaluate as a
/// formula (leading `=`, `+`, `-`, `@`, tab or carriage return) get a leading `'`.
#[must_use]
pub fn csv_text(value: &str) -> String {
    let value = if value.starts_with(['=', '+', '-', '@', '\t', '\r']) {
        format!("'{value}")
    } else {
        value.to_owned()
    };
    if value.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", value.replace('"', "\"\""))
    } else {
        value
    }
}

fn csv_num(value: Option<u64>) -> String {
    value.map(|v| v.to_string()).unwrap_or_default()
}

/// Renders export rows as CSV (header + one CRLF-terminated line per task; unknown → empty).
#[must_use]
pub fn tasks_csv(rows: &[TaskRow]) -> String {
    let mut out = TASK_ROW_COLUMNS.join(",");
    out.push_str("\r\n");
    for r in rows {
        let cells = [
            csv_text(&r.task_id),
            csv_text(&r.sandbox),
            r.number.to_string(),
            csv_text(&r.title),
            csv_text(&r.status),
            csv_text(&r.origin),
            csv_text(&r.harness),
            csv_text(&r.harnesses),
            csv_text(&r.models),
            csv_text(&r.accounts),
            csv_text(&r.created_at),
            csv_text(r.closed_at.as_deref().unwrap_or("")),
            r.attempts.to_string(),
            r.wall_ms.to_string(),
            csv_num(r.active_ms),
            csv_num(r.input_tokens),
            csv_num(r.output_tokens),
            csv_num(r.cache_read_tokens),
            csv_num(r.cache_write_tokens),
            r.unknown_usage_runs.to_string(),
            r.switches.to_string(),
            csv_text(r.feedback.as_deref().unwrap_or("")),
            r.feedback_correct.to_string(),
            r.feedback_incorrect.to_string(),
        ];
        let _ = write!(out, "{}\r\n", cells.join(","));
    }
    out
}

fn non_empty(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|v| !v.is_empty())
}

impl App {
    /// Reads the cohort for a request: visible sandboxes (or one, which must be visible) and
    /// an optional inclusive day range.
    async fn analytics_cohort(
        &self,
        ctx: &AuthContext,
        sandbox: Option<&str>,
        from: Option<&str>,
        to: Option<&str>,
    ) -> AppResult<(AnalyticsData, AnalyticsRange, Option<(i64, i64)>)> {
        let (sandbox, from, to) = (non_empty(sandbox), non_empty(from), non_empty(to));
        let from_day = from.map(parse_day).transpose()?;
        let to_day = to.map(parse_day).transpose()?;
        if let (Some(a), Some(b)) = (from_day, to_day)
            && a > b
        {
            return Err(AppError::Invalid("from must not be after to".into()));
        }
        let sandboxes = match sandbox {
            Some(id) => {
                let sid = SandboxId::parse(id).map_err(|_| AppError::NotFound)?;
                vec![self.require(&ctx.user, &sid, Action::View).await?.0.id]
            }
            None => self
                .visible_sandboxes(&ctx.user)
                .await?
                .into_iter()
                .map(|(s, _)| s.id)
                .collect(),
        };
        let query = AnalyticsQuery {
            sandboxes,
            from: from_day.map(|d| Timestamp(d.saturating_mul(DAY_MS))),
            to: to_day.map(|d| Timestamp(d.saturating_add(1).saturating_mul(DAY_MS))),
            limit: MAX_ANALYTICS_TASKS,
        };
        let data = if query.sandboxes.is_empty() {
            AnalyticsData::default()
        } else {
            self.deps.store.analytics_data(&query).await?
        };
        let range = AnalyticsRange {
            sandbox: sandbox.map(str::to_owned),
            from: from.map(str::to_owned),
            to: to.map(str::to_owned),
        };
        let days = match (from_day, to_day) {
            (Some(a), Some(b)) => Some((a, b)),
            _ => None,
        };
        Ok((data, range, days))
    }

    /// `GET /api/analytics/summary` (read access; only visible sandboxes count).
    ///
    /// # Errors
    /// `Invalid` dates, `NotFound` for an invisible sandbox.
    pub async fn analytics_summary(
        &self,
        ctx: &AuthContext,
        sandbox: Option<&str>,
        from: Option<&str>,
        to: Option<&str>,
    ) -> AppResult<AnalyticsSummary> {
        let (data, range, days) = self.analytics_cohort(ctx, sandbox, from, to).await?;
        Ok(summarize(&data, range, days, self.now()))
    }

    /// `GET /api/analytics/tasks` rows (read access; only visible sandboxes).
    ///
    /// # Errors
    /// `Invalid` dates, `NotFound` for an invisible sandbox.
    pub async fn analytics_tasks(
        &self,
        ctx: &AuthContext,
        sandbox: Option<&str>,
        from: Option<&str>,
        to: Option<&str>,
    ) -> AppResult<Vec<TaskRow>> {
        let (data, _, _) = self.analytics_cohort(ctx, sandbox, from, to).await?;
        Ok(task_rows(&data, self.now()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(text: &str) -> Result<TaskId, Box<dyn std::error::Error>> {
        Ok(TaskId::parse(text)?)
    }

    #[test]
    fn days_round_trip_and_reject_garbage() -> Result<(), Box<dyn std::error::Error>> {
        assert_eq!(parse_day("1970-01-01")?, 0);
        assert_eq!(parse_day("2026-10-07")?, 1_791_331_200_000 / DAY_MS);
        assert_eq!(day_string(parse_day("2024-02-29")?), "2024-02-29");
        for bad in [
            "2026-02-30",
            "2026-13-01",
            "26-10-07",
            "2026/10/07",
            "2026-1a-07",
            "1969-12-31",
        ] {
            assert!(parse_day(bad).is_err(), "{bad}");
        }
        Ok(())
    }

    #[test]
    fn median_and_p90() {
        assert_eq!(median_p90(&mut []), (None, None));
        assert_eq!(median_p90(&mut [5]), (Some(5), Some(5)));
        assert_eq!(median_p90(&mut [30, 10]), (Some(20), Some(30)));
        let mut ten: Vec<u64> = (1..=10).collect();
        assert_eq!(median_p90(&mut ten), (Some(5), Some(9)));
    }

    #[test]
    fn csv_quotes_and_neutralizes_formulas() {
        assert_eq!(csv_text("plain"), "plain");
        assert_eq!(csv_text("a,b"), "\"a,b\"");
        assert_eq!(csv_text("say \"hi\""), "\"say \"\"hi\"\"\"");
        assert_eq!(csv_text("two\nlines"), "\"two\nlines\"");
        assert_eq!(csv_text("=HYPERLINK(1)"), "'=HYPERLINK(1)");
        assert_eq!(csv_text("-1+2,3"), "\"'-1+2,3\"");
    }

    #[test]
    fn unknown_runs_are_counted_not_zeroed() -> Result<(), Box<dyn std::error::Error>> {
        let t = id("t1")?;
        let run = |seq: u32, metrics: Option<MetricsRecord>| AnalyticsRun {
            task_id: t.clone(),
            seq,
            harness: Harness::ClaudeCode,
            account_id: None,
            account_label: None,
            started_at: Timestamp(0),
            ended_at: Some(Timestamp(1_000)),
            metrics,
        };
        let data = AnalyticsData {
            runs: vec![
                run(1, None),
                run(2, Some(MetricsRecord::default())),
                run(
                    3,
                    Some(MetricsRecord {
                        output_tokens: Some(7),
                        ..MetricsRecord::default()
                    }),
                ),
            ],
            ..AnalyticsData::default()
        };
        let s = summarize(&data, AnalyticsRange::default(), None, Timestamp(0));
        assert_eq!(s.tokens.runs, 3);
        assert_eq!(s.tokens.unknown_usage_runs, 2);
        assert_eq!(s.tokens.output, Some(7));
        assert_eq!(s.tokens.input, None, "never zero-filled");
        assert_eq!(s.duration_ms.total_wall, 3_000);
        assert_eq!(s.duration_ms.total_active, None);
        Ok(())
    }
}
