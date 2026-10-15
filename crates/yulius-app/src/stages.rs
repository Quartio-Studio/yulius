//! Stages measured of tasks, CI runs and CD runs (SPEC-137, ADR-0112).
//!
//! The store writes the spans that follow a state (its triggers, in the same transaction as the
//! state). This module writes what has no state of its own, as it happens: the stages a job's or a
//! deploy's output names ([`OutputStages`]), a CI run's preparation and job-slot waits, a CD run's
//! promotion, and a task's armed-merge wait and updates onto the integration branch. A span write
//! never fails or holds up the work it measures: an error is logged and the span dropped.
//!
//! It also serves the spans: one subject's timeline, the stages' percentiles, trends and delivery
//! figures over a range, and the export.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Map, Value};
use yulius_contracts::api::stages::{
    DeliveryFiguresView, SpanView, StageDay, StageOptions, StageRow, StageSummary, StageTrend,
    TimelineView,
};
use yulius_domain::ci::CiRef;
use yulius_domain::ids::{SandboxId, TaskId};
use yulius_domain::permission::Action;
use yulius_domain::stages::{
    Deployment, MAX_OUTPUT_STAGES, OutputEvent, OutputScanner, SubjectKind, delivery_figures,
    percentile,
};
use yulius_domain::time::Timestamp;

use crate::analytics::{csv_text, parse_day};
use crate::auth::{AuthContext, rfc3339};
use crate::context::App;
use crate::error::{AppError, AppResult};

/// Most spans one timeline reads (the newest).
pub const MAX_TIMELINE_SPANS: u32 = 2_000;
/// Most spans one summary or export reads.
pub const MAX_SUMMARY_SPANS: u32 = 200_000;
/// Longest range a summary covers, in days.
const MAX_DAYS: i64 = 366;
const DAY_MS: i64 = 86_400_000;
/// How long after a range's end a deployment still counts as delivering its merges.
const LEAD_TIME_HORIZON_MS: i64 = 30 * DAY_MS;

/// One stored span.
#[derive(Debug, Clone, PartialEq)]
pub struct SpanRecord {
    /// Sandbox.
    pub sandbox_id: SandboxId,
    /// What it belongs to.
    pub subject_kind: SubjectKind,
    /// The task's, CI run's or CD run's id.
    pub subject_id: String,
    /// Unique within the subject.
    pub key: String,
    /// Parent span's key.
    pub parent_key: Option<String>,
    /// Sort of stage.
    pub kind: String,
    /// Name.
    pub name: String,
    /// CD environment.
    pub environment: Option<String>,
    /// CI job definition name.
    pub job: Option<String>,
    /// Start.
    pub started_at: Timestamp,
    /// End, while it runs `None`.
    pub ended_at: Option<Timestamp>,
    /// How it ended.
    pub outcome: Option<String>,
    /// `recorded` or `backfilled`.
    pub source: String,
    /// JSON object of figures.
    pub detail: Option<String>,
}

impl SpanRecord {
    /// End minus start, once ended.
    #[must_use]
    pub fn duration_ms(&self) -> Option<i64> {
        self.ended_at
            .map(|end| end.0.saturating_sub(self.started_at.0).max(0))
    }

    fn detail_value(&self) -> Value {
        self.detail
            .as_deref()
            .and_then(|text| serde_json::from_str(text).ok())
            .unwrap_or_else(|| Value::Object(Map::new()))
    }

    fn reused(&self) -> bool {
        self.detail_value().get("reused") == Some(&Value::Bool(true))
    }

    fn failed(&self) -> bool {
        matches!(self.outcome.as_deref(), Some("failed" | "error"))
    }

    /// The span as the API shows it.
    #[must_use]
    pub fn view(&self) -> SpanView {
        SpanView {
            subject_kind: self.subject_kind.as_str().to_owned(),
            subject_id: self.subject_id.clone(),
            key: self.key.clone(),
            parent_key: self.parent_key.clone(),
            kind: self.kind.clone(),
            name: self.name.clone(),
            environment: self.environment.clone(),
            job: self.job.clone(),
            started_at: rfc3339(self.started_at),
            ended_at: self.ended_at.map(rfc3339),
            duration_ms: self.duration_ms(),
            outcome: self.outcome.clone(),
            source: self.source.clone(),
            detail: self.detail_value(),
        }
    }
}

/// A span Yulius starts (ended at once when `ended_at` is given).
#[derive(Debug, Clone, PartialEq)]
pub struct NewSpan {
    /// Sandbox.
    pub sandbox_id: SandboxId,
    /// What it belongs to.
    pub subject_kind: SubjectKind,
    /// Subject id.
    pub subject_id: String,
    /// Unique within the subject.
    pub key: String,
    /// Parent span's key.
    pub parent_key: Option<String>,
    /// Sort of stage.
    pub kind: String,
    /// Name.
    pub name: String,
    /// CD environment.
    pub environment: Option<String>,
    /// CI job definition name.
    pub job: Option<String>,
    /// Start.
    pub started_at: Timestamp,
    /// End, for a span recorded once it ended.
    pub ended_at: Option<Timestamp>,
    /// Outcome, for a span recorded once it ended.
    pub outcome: Option<String>,
    /// JSON object of figures.
    pub detail: Option<String>,
}

/// What a summary or an export reads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpanFilter {
    /// Sandbox.
    pub sandbox: SandboxId,
    /// Spans that started at or after this.
    pub from: Timestamp,
    /// … and before this.
    pub to: Timestamp,
    /// Only this subject kind.
    pub subject_kind: Option<SubjectKind>,
    /// Only this span kind.
    pub kind: Option<String>,
    /// Only this name.
    pub name: Option<String>,
    /// Only this CD environment.
    pub environment: Option<String>,
    /// Only this CI job.
    pub job: Option<String>,
    /// Most spans read (the newest).
    pub limit: u32,
}

/// A write the output of a job or deploy asks for.
#[derive(Debug, Clone, PartialEq)]
pub enum SpanOp {
    /// Start a span.
    Start(Box<NewSpan>),
    /// End an open span.
    End {
        /// Its key.
        key: String,
        /// When.
        at: Timestamp,
        /// How, when known.
        outcome: Option<String>,
    },
    /// Set (or, with `add`, add to) figures of a span.
    Figures {
        /// Its key.
        key: String,
        /// Name and value.
        figures: Vec<(String, f64)>,
        /// Add to what is there instead of replacing it.
        add: bool,
    },
}

/// The stages of one job's (or one CD run's) output, as it arrives: turns the scanner's events
/// into span writes under `root` (the job's span, or the CD run's), timed by Yulius's clock.
#[derive(Debug)]
pub struct OutputStages {
    sandbox: SandboxId,
    subject_kind: SubjectKind,
    subject_id: String,
    root: String,
    job: Option<String>,
    environment: Option<String>,
    scanner: OutputScanner,
    phase: Option<String>,
    stage: Option<String>,
    count: usize,
}

impl OutputStages {
    /// The stages of a CI job whose span is `job_key`.
    #[must_use]
    pub fn ci_job(sandbox: SandboxId, run_id: &str, job_key: &str, job: &str) -> Self {
        Self {
            sandbox,
            subject_kind: SubjectKind::CiRun,
            subject_id: run_id.to_owned(),
            root: job_key.to_owned(),
            job: Some(job.to_owned()),
            environment: None,
            scanner: OutputScanner::ci(),
            phase: None,
            stage: None,
            count: 0,
        }
    }

    /// The phases (deploy, verify) and stages of a CD run.
    #[must_use]
    pub fn cd_run(sandbox: SandboxId, run_id: &str, environment: &str) -> Self {
        Self {
            sandbox,
            subject_kind: SubjectKind::CdRun,
            subject_id: run_id.to_owned(),
            root: "run".to_owned(),
            job: None,
            environment: Some(environment.to_owned()),
            scanner: OutputScanner::cd(),
            phase: None,
            stage: None,
            count: 0,
        }
    }

    /// The sandbox and subject its spans belong to.
    #[must_use]
    pub fn subject(&self) -> (SandboxId, (SubjectKind, String)) {
        (
            self.sandbox.clone(),
            (self.subject_kind, self.subject_id.clone()),
        )
    }

    /// The span under which the job's own stages go (the job, or the open CD phase).
    #[must_use]
    pub fn parent(&self) -> &str {
        self.phase.as_deref().unwrap_or(&self.root)
    }

    /// A span of `kind` and `name` under the job, recorded once it ended.
    #[must_use]
    pub fn child(
        &self,
        suffix: &str,
        kind: &str,
        name: &str,
        (started_at, ended_at): (Timestamp, Timestamp),
        outcome: &str,
    ) -> NewSpan {
        NewSpan {
            ended_at: Some(ended_at),
            outcome: Some(outcome.to_owned()),
            ..self.span(
                format!("{}/{suffix}", self.root),
                Some(self.root.clone()),
                kind,
                name,
                started_at,
            )
        }
    }

    fn span(
        &self,
        key: String,
        parent_key: Option<String>,
        kind: &str,
        name: &str,
        started_at: Timestamp,
    ) -> NewSpan {
        NewSpan {
            sandbox_id: self.sandbox.clone(),
            subject_kind: self.subject_kind,
            subject_id: self.subject_id.clone(),
            key,
            parent_key,
            kind: kind.to_owned(),
            name: name.to_owned(),
            environment: self.environment.clone(),
            job: self.job.clone(),
            started_at,
            ended_at: None,
            outcome: None,
            detail: None,
        }
    }

    /// The writes the output `bytes` (escape sequences removed) asks for, read at `now`.
    pub fn feed(&mut self, bytes: &[u8], now: Timestamp) -> Vec<SpanOp> {
        let events = self.scanner.push(bytes);
        self.apply(events, now)
    }

    /// The writes of a last line without a line break, once the output ended.
    pub fn finish(&mut self, now: Timestamp) -> Vec<SpanOp> {
        let events = self.scanner.finish();
        self.apply(events, now)
    }

    fn end_stage(&mut self, now: Timestamp, ops: &mut Vec<SpanOp>) {
        if let Some(key) = self.stage.take() {
            ops.push(SpanOp::End {
                key,
                at: now,
                outcome: None,
            });
        }
    }

    fn apply(&mut self, events: Vec<OutputEvent>, now: Timestamp) -> Vec<SpanOp> {
        let mut ops = Vec::new();
        // Figures are written once per span and read (review H2): counts added up, set values the
        // last one, after the spans they belong to were started.
        let mut figures: Vec<SpanOp> = Vec::new();
        let mut merge = |key: String, values: Vec<(String, f64)>, add: bool| {
            let found = figures.iter_mut().find_map(|op| match op {
                SpanOp::Figures {
                    key: k,
                    figures: f,
                    add: a,
                } if *k == key && *a == add => Some(f),
                _ => None,
            });
            let Some(current) = found else {
                figures.push(SpanOp::Figures {
                    key,
                    figures: values,
                    add,
                });
                return;
            };
            for (name, value) in values {
                match current.iter_mut().find(|(n, _)| *n == name) {
                    Some((_, v)) if add => *v += value,
                    Some((_, v)) => *v = value,
                    None => current.push((name, value)),
                }
            }
        };
        for event in events {
            match event {
                OutputEvent::Stage(name) => {
                    self.end_stage(now, &mut ops);
                    if self.count >= MAX_OUTPUT_STAGES {
                        continue;
                    }
                    self.count += 1;
                    let parent = self.parent().to_owned();
                    let key = format!("{parent}/stage:{}", self.count);
                    let span = self.span(key.clone(), Some(parent), "stage", &name, now);
                    ops.push(SpanOp::Start(Box::new(span)));
                    self.stage = Some(key);
                }
                OutputEvent::StageEnd => self.end_stage(now, &mut ops),
                OutputEvent::Measures(values) => merge(
                    self.stage
                        .clone()
                        .unwrap_or_else(|| self.parent().to_owned()),
                    values,
                    false,
                ),
                OutputEvent::Tests(counts) => merge(
                    self.stage
                        .clone()
                        .unwrap_or_else(|| self.parent().to_owned()),
                    counts
                        .figures()
                        .into_iter()
                        .filter(|(_, n)| *n > 0)
                        .map(|(name, n)| (name.to_owned(), count_value(n)))
                        .collect(),
                    true,
                ),
                OutputEvent::PhaseStart(phase) => {
                    self.end_stage(now, &mut ops);
                    if let Some(key) = self.phase.take() {
                        ops.push(SpanOp::End {
                            key,
                            at: now,
                            outcome: Some("passed".into()),
                        });
                    }
                    let key = format!("{}/{}", self.root, phase.as_str());
                    let root = Some(self.root.clone());
                    let span = self.span(key.clone(), root, phase.as_str(), phase.as_str(), now);
                    ops.push(SpanOp::Start(Box::new(span)));
                    self.phase = Some(key);
                }
                OutputEvent::PhaseEnd { failed } => {
                    let outcome = if failed { "failed" } else { "passed" };
                    if let Some(key) = self.stage.take() {
                        ops.push(SpanOp::End {
                            key,
                            at: now,
                            outcome: failed.then(|| outcome.to_owned()),
                        });
                    }
                    if let Some(key) = self.phase.take() {
                        ops.push(SpanOp::End {
                            key,
                            at: now,
                            outcome: Some(outcome.into()),
                        });
                    }
                }
            }
        }
        ops.extend(figures);
        ops
    }
}

impl App {
    /// Applies span writes; a failed one is logged and dropped (SPEC-137 STG-003).
    pub(crate) async fn record_spans(
        &self,
        sandbox: &SandboxId,
        subject: (SubjectKind, &str),
        ops: Vec<SpanOp>,
    ) {
        let store = &self.deps.store;
        for op in ops {
            let result = match &op {
                SpanOp::Start(span) => store.insert_span(span).await.map(|_| ()),
                SpanOp::End { key, at, outcome } => store
                    .end_span(sandbox, subject.0, subject.1, key, *at, outcome.as_deref())
                    .await
                    .map(|_| ()),
                SpanOp::Figures { key, figures, add } => {
                    store
                        .span_figures(sandbox, subject.0, subject.1, key, figures, *add)
                        .await
                }
            };
            if let Err(err) = result {
                tracing::warn!(subject = %subject.1, error = %err, "recording a stage failed");
            }
        }
    }

    /// Records one span (best effort).
    pub(crate) async fn record_span(&self, span: NewSpan) {
        let (sandbox, kind, id) = (
            span.sandbox_id.clone(),
            span.subject_kind,
            span.subject_id.clone(),
        );
        self.record_spans(&sandbox, (kind, &id), vec![SpanOp::Start(Box::new(span))])
            .await;
    }

    /// Deletes spans older than [`yulius_domain::stages::RETAIN_MS`] (SPEC-137 STG-005), with
    /// the CI and CD runs' hourly upkeep.
    pub(crate) async fn prune_spans(&self, now: Timestamp) {
        let before = Timestamp(now.0.saturating_sub(yulius_domain::stages::RETAIN_MS));
        match self.deps.store.prune_spans(before).await {
            Ok(0) => {}
            Ok(n) => tracing::info!(spans = n, "old stage spans deleted"),
            Err(err) => tracing::warn!(error = %err, "pruning stage spans failed"),
        }
    }

    /// A span of a task Yulius measured, recorded once it ended (an armed merge's wait, an
    /// update onto the integration branch).
    pub(crate) async fn record_task_span(
        &self,
        task: &TaskId,
        (kind, name): (&str, &str),
        (started_at, ended_at): (Timestamp, Timestamp),
        outcome: &str,
        detail: Value,
    ) {
        let sandbox = match self.deps.store.task(task).await {
            Ok(Some(record)) => record.sandbox_id,
            _ => return,
        };
        self.record_span(NewSpan {
            sandbox_id: sandbox,
            subject_kind: SubjectKind::Task,
            subject_id: task.as_str().to_owned(),
            key: format!("{kind}@{}", started_at.0),
            parent_key: None,
            kind: kind.to_owned(),
            name: name.to_owned(),
            environment: None,
            job: None,
            started_at,
            ended_at: Some(ended_at),
            outcome: Some(outcome.to_owned()),
            detail: Some(detail.to_string()),
        })
        .await;
    }

    async fn stage_sandbox(&self, ctx: &AuthContext, sandbox: &str) -> AppResult<SandboxId> {
        let sid = SandboxId::parse(sandbox).map_err(|_| AppError::NotFound)?;
        Ok(self.require(&ctx.user, &sid, Action::View).await?.0.id)
    }

    /// `GET /api/sandboxes/{sid}/stages/timeline?subject=<kind>:<id>` (read access).
    ///
    /// # Errors
    /// `NotFound` for an invisible sandbox, `Invalid` subject.
    pub async fn stage_timeline(
        &self,
        ctx: &AuthContext,
        sandbox: &str,
        subject: &str,
    ) -> AppResult<TimelineView> {
        let sandbox = self.stage_sandbox(ctx, sandbox).await?;
        let (kind, id) = subject
            .split_once(':')
            .and_then(|(kind, id)| Some((SubjectKind::parse(kind)?, id)))
            .filter(|(_, id)| !id.is_empty() && id.len() <= 128)
            .ok_or_else(|| {
                AppError::Invalid("subject must be task:<id>, ci_run:<id> or cd_run:<id>".into())
            })?;
        let spans = self
            .deps
            .store
            .subject_spans(&sandbox, kind, id, MAX_TIMELINE_SPANS)
            .await?;
        let truncated = u32::try_from(spans.len()).unwrap_or(u32::MAX) >= MAX_TIMELINE_SPANS;
        let mut related = Vec::new();
        // Only a task of this sandbox has its pull request's CI runs here (review L9).
        if kind == SubjectKind::Task
            && let Ok(task) = TaskId::parse(id)
            && matches!(self.deps.store.task(&task).await?, Some(t) if t.sandbox_id == sandbox)
            && let Some(number) = self
                .deps
                .store
                .delivery(&task)
                .await?
                .and_then(|record| record.pr_number)
        {
            related = self
                .deps
                .store
                .ci_reference_spans(&sandbox, number)
                .await?
                .into_iter()
                .filter(|span| {
                    let reference = span
                        .detail_value()
                        .get("reference")
                        .and_then(Value::as_str)
                        .map(str::to_owned);
                    match reference.as_deref().and_then(CiRef::parse) {
                        Some(CiRef::PullRequest(n)) => n == number,
                        Some(CiRef::Train(numbers)) => numbers.contains(&number),
                        _ => false,
                    }
                })
                .map(|span| span.view())
                .collect();
        }
        Ok(TimelineView {
            subject_kind: kind.as_str().to_owned(),
            subject_id: id.to_owned(),
            spans: spans.iter().map(SpanRecord::view).collect(),
            related,
            truncated,
        })
    }

    fn stage_filter(
        sandbox: SandboxId,
        query: &StageQuery,
        (from_day, to_day): (i64, i64),
    ) -> AppResult<SpanFilter> {
        let pick = |value: &Option<String>| {
            value
                .as_deref()
                .map(str::trim)
                .filter(|v| !v.is_empty())
                .map(str::to_owned)
        };
        Ok(SpanFilter {
            sandbox,
            from: Timestamp(from_day * DAY_MS),
            to: Timestamp((to_day + 1) * DAY_MS),
            subject_kind: match pick(&query.subject) {
                Some(text) => Some(
                    SubjectKind::parse(&text)
                        .ok_or_else(|| AppError::Invalid("unknown subject kind".into()))?,
                ),
                None => None,
            },
            kind: pick(&query.kind),
            name: pick(&query.stage),
            environment: pick(&query.environment),
            job: pick(&query.job),
            limit: MAX_SUMMARY_SPANS,
        })
    }

    fn stage_days(&self, query: &StageQuery) -> AppResult<(i64, i64)> {
        let today = self.now().0.div_euclid(DAY_MS);
        let to = match query.to.as_deref().filter(|v| !v.is_empty()) {
            Some(day) => parse_day(day)?,
            None => today,
        };
        let from = match query.from.as_deref().filter(|v| !v.is_empty()) {
            Some(day) => parse_day(day)?,
            None => to - 29,
        };
        if from > to {
            return Err(AppError::Invalid("from must not be after to".into()));
        }
        if to - from >= MAX_DAYS {
            return Err(AppError::Invalid(format!(
                "a range covers at most {MAX_DAYS} days"
            )));
        }
        Ok((from, to))
    }

    /// `GET /api/sandboxes/{sid}/stages/summary` (read access).
    ///
    /// # Errors
    /// `NotFound` for an invisible sandbox, `Invalid` dates or filters.
    pub async fn stage_summary(
        &self,
        ctx: &AuthContext,
        sandbox: &str,
        query: &StageQuery,
    ) -> AppResult<StageSummary> {
        let sandbox = self.stage_sandbox(ctx, sandbox).await?;
        let days = self.stage_days(query)?;
        let filter = Self::stage_filter(sandbox.clone(), query, days)?;
        let spans = self.deps.store.spans_between(&filter).await?;
        // Delivery figures use every deployment and merge of the range, whatever the filters,
        // and deployments up to a month after it for the lead time of its last merges.
        let deliveries = SpanFilter {
            subject_kind: None,
            kind: None,
            name: None,
            environment: None,
            job: None,
            to: Timestamp(filter.to.0 + LEAD_TIME_HORIZON_MS),
            ..filter.clone()
        };
        let mut cd = self
            .deps
            .store
            .spans_between(&SpanFilter {
                subject_kind: Some(SubjectKind::CdRun),
                kind: Some("run".into()),
                ..deliveries.clone()
            })
            .await?;
        let merges: Vec<i64> = self
            .deps
            .store
            .spans_between(&SpanFilter {
                subject_kind: Some(SubjectKind::Task),
                kind: Some("delivery".into()),
                name: Some("pr_open".into()),
                from: Timestamp(filter.from.0 - LEAD_TIME_HORIZON_MS),
                ..deliveries
            })
            .await?
            .into_iter()
            .filter(|span| span.outcome.as_deref() == Some("merged_integration"))
            .filter_map(|span| span.ended_at.map(|t| t.0))
            .filter(|at| (filter.from.0..filter.to.0).contains(at))
            .collect();
        cd.retain(|span| span.ended_at.is_some() && span.outcome.as_deref() != Some("cancelled"));
        let truncated = u32::try_from(spans.len()).unwrap_or(u32::MAX) >= filter.limit;
        Ok(summarize(
            &spans,
            &cd,
            &merges,
            days,
            query.trend.as_deref(),
            truncated,
        ))
    }

    /// `GET /api/sandboxes/{sid}/stages/spans` rows (read access).
    ///
    /// # Errors
    /// As [`App::stage_summary`].
    pub async fn stage_spans(
        &self,
        ctx: &AuthContext,
        sandbox: &str,
        query: &StageQuery,
    ) -> AppResult<Vec<SpanView>> {
        let sandbox = self.stage_sandbox(ctx, sandbox).await?;
        let days = self.stage_days(query)?;
        let filter = Self::stage_filter(sandbox, query, days)?;
        Ok(self
            .deps
            .store
            .spans_between(&filter)
            .await?
            .iter()
            .map(SpanRecord::view)
            .collect())
    }
}

/// The query of a summary or an export.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Deserialize)]
pub struct StageQuery {
    /// First day (`YYYY-MM-DD`, UTC), default 29 days before `to`.
    #[serde(default)]
    pub from: Option<String>,
    /// Last day, default today.
    #[serde(default)]
    pub to: Option<String>,
    /// `task`, `ci_run` or `cd_run`.
    #[serde(default)]
    pub subject: Option<String>,
    /// Span kind.
    #[serde(default)]
    pub kind: Option<String>,
    /// Span name.
    #[serde(default)]
    pub stage: Option<String>,
    /// CD environment.
    #[serde(default)]
    pub environment: Option<String>,
    /// CI job.
    #[serde(default)]
    pub job: Option<String>,
    /// The stage whose trend is drawn: `<subject>:<kind>:<name>`.
    #[serde(default)]
    pub trend: Option<String>,
    /// Export format (`json`, `csv`).
    #[serde(default)]
    pub format: Option<String>,
}

/// What a stage row is: subject, kind, name, job, environment.
type RowKey = (String, String, String, Option<String>, Option<String>);

/// A count as a figure's value.
#[allow(clippy::cast_precision_loss)] // Counts far below 2^52.
fn count_value(n: u64) -> f64 {
    n as f64
}

/// The row a span counts in: a task's steps together, a job by its definition, a queue also by why
/// it waited.
fn row_keys(span: &SpanRecord) -> Vec<RowKey> {
    let subject = span.subject_kind.as_str().to_owned();
    let name = match span.kind.as_str() {
        "step" => "step".to_owned(),
        "job" => span.job.clone().unwrap_or_else(|| span.name.clone()),
        _ => span.name.clone(),
    };
    let job = if span.kind == "job" {
        None
    } else {
        span.job.clone()
    };
    let mut keys = vec![(
        subject.clone(),
        span.kind.clone(),
        name,
        job.clone(),
        span.environment.clone(),
    )];
    if span.kind == "queue" && span.duration_ms().unwrap_or(0) >= 1000 {
        let disk = span
            .detail_value()
            .get("reason")
            .and_then(Value::as_str)
            .is_some_and(|reason| reason.starts_with("waiting for disk"));
        let why = if disk {
            "queue · waiting for disk"
        } else {
            "queue · waiting for a lane"
        };
        keys.push((
            subject,
            span.kind.clone(),
            why.to_owned(),
            job,
            span.environment.clone(),
        ));
    }
    keys
}

fn day_string(day: i64) -> String {
    rfc3339(Timestamp(day * DAY_MS))[..10].to_owned()
}

/// The summary of `spans` (pure; `cd` the finished CD runs, `merges` the merge times).
#[must_use]
pub fn summarize(
    spans: &[SpanRecord],
    cd: &[SpanRecord],
    merges: &[i64],
    (from_day, to_day): (i64, i64),
    trend: Option<&str>,
    truncated: bool,
) -> StageSummary {
    let mut rows: BTreeMap<RowKey, (Vec<i64>, StageRow)> = BTreeMap::new();
    let mut options = (BTreeSet::new(), BTreeSet::new(), BTreeSet::new());
    let mut reused_runs = 0u64;
    let wanted: Option<(String, String, String)> = trend.and_then(|text| {
        let mut parts = text.splitn(3, ':');
        Some((
            parts.next()?.to_owned(),
            parts.next()?.to_owned(),
            parts.next()?.to_owned(),
        ))
    });
    let wanted = wanted.unwrap_or_else(|| ("ci_run".into(), "run".into(), "run".into()));
    let mut trend_days: BTreeMap<i64, Vec<i64>> = BTreeMap::new();
    for span in spans {
        if let Some(job) = &span.job {
            options.0.insert(job.clone());
        }
        if let Some(env) = &span.environment {
            options.1.insert(env.clone());
        }
        options.2.insert(span.kind.clone());
        if span.reused() {
            if span.kind == "run" {
                reused_runs += 1;
            }
            continue;
        }
        for key in row_keys(span) {
            let is_trend = (key.0.as_str(), key.1.as_str(), key.2.as_str())
                == (wanted.0.as_str(), wanted.1.as_str(), wanted.2.as_str());
            let (durations, row) = rows.entry(key.clone()).or_insert_with(|| {
                (
                    Vec::new(),
                    StageRow {
                        subject_kind: key.0.clone(),
                        kind: key.1.clone(),
                        name: key.2.clone(),
                        job: key.3.clone(),
                        environment: key.4.clone(),
                        count: 0,
                        failed: 0,
                        running: 0,
                        backfilled: 0,
                        p50_ms: None,
                        p90_ms: None,
                        max_ms: None,
                        total_ms: 0,
                    },
                )
            });
            match (span.duration_ms(), span.ended_at) {
                (Some(ms), Some(end)) => {
                    durations.push(ms);
                    row.count += 1;
                    row.total_ms = row.total_ms.saturating_add(ms);
                    row.failed += u64::from(span.failed());
                    row.backfilled += u64::from(span.source == "backfilled");
                    if is_trend {
                        trend_days
                            .entry(end.0.div_euclid(DAY_MS))
                            .or_default()
                            .push(ms);
                    }
                }
                _ => row.running += 1,
            }
        }
    }
    let mut stages: Vec<StageRow> = rows
        .into_values()
        .map(|(mut durations, mut row)| {
            durations.sort_unstable();
            row.p50_ms = percentile(&durations, 50);
            row.p90_ms = percentile(&durations, 90);
            row.max_ms = durations.last().copied();
            row
        })
        .collect();
    stages.sort_by(|a, b| {
        b.total_ms
            .cmp(&a.total_ms)
            .then_with(|| a.name.cmp(&b.name))
    });
    let days = (from_day..=to_day)
        .map(|day| {
            let mut values = trend_days.remove(&day).unwrap_or_default();
            values.sort_unstable();
            StageDay {
                date: day_string(day),
                count: values.len() as u64,
                p50_ms: percentile(&values, 50),
                p90_ms: percentile(&values, 90),
            }
        })
        .collect();
    let (range_from, range_to) = (from_day * DAY_MS, (to_day + 1) * DAY_MS);
    let delivery = ["dev", "prod"]
        .into_iter()
        .map(|env| {
            let all: Vec<Deployment> = cd
                .iter()
                .filter(|span| span.environment.as_deref() == Some(env))
                .filter_map(|span| {
                    Some(Deployment {
                        started_at: span.started_at.0,
                        ended_at: span.ended_at?.0,
                        passed: span.outcome.as_deref() == Some("passed"),
                    })
                })
                .collect();
            let in_range: Vec<Deployment> = all
                .iter()
                .copied()
                .filter(|d| (range_from..range_to).contains(&d.started_at))
                .collect();
            let counts = delivery_figures(&in_range, &[], range_to - range_from);
            let lead = delivery_figures(&all, merges, range_to - range_from);
            DeliveryFiguresView {
                environment: env.to_owned(),
                deployments: counts.deployments,
                passed: counts.passed,
                per_week: counts.per_week,
                lead_time_p50_ms: lead.lead_time_p50,
                lead_time_p90_ms: lead.lead_time_p90,
                lead_time_count: lead.lead_time_count,
                change_failure_rate: counts.change_failure_rate,
                time_to_restore_p50_ms: counts.time_to_restore_p50,
                restores: counts.restores,
            }
        })
        .collect();
    StageSummary {
        from: day_string(from_day),
        to: day_string(to_day),
        stages,
        trend: StageTrend {
            subject_kind: wanted.0,
            kind: wanted.1,
            name: wanted.2,
            days,
        },
        delivery,
        options: StageOptions {
            jobs: options.0.into_iter().collect(),
            environments: options.1.into_iter().collect(),
            kinds: options.2.into_iter().collect(),
        },
        reused_runs,
        truncated,
    }
}

/// Export columns, in order.
pub const SPAN_COLUMNS: [&str; 14] = [
    "subject_kind",
    "subject_id",
    "key",
    "parent_key",
    "kind",
    "name",
    "environment",
    "job",
    "started_at",
    "ended_at",
    "duration_ms",
    "outcome",
    "source",
    "detail",
];

/// Spans as CSV (header + one CRLF-terminated line each; unknown → empty).
#[must_use]
pub fn spans_csv(rows: &[SpanView]) -> String {
    let mut out = SPAN_COLUMNS.join(",");
    out.push_str("\r\n");
    for row in rows {
        let cells = [
            csv_text(&row.subject_kind),
            csv_text(&row.subject_id),
            csv_text(&row.key),
            csv_text(row.parent_key.as_deref().unwrap_or("")),
            csv_text(&row.kind),
            csv_text(&row.name),
            csv_text(row.environment.as_deref().unwrap_or("")),
            csv_text(row.job.as_deref().unwrap_or("")),
            csv_text(&row.started_at),
            csv_text(row.ended_at.as_deref().unwrap_or("")),
            row.duration_ms.map(|v| v.to_string()).unwrap_or_default(),
            csv_text(row.outcome.as_deref().unwrap_or("")),
            csv_text(&row.source),
            csv_text(&row.detail.to_string()),
        ];
        out.push_str(&cells.join(","));
        out.push_str("\r\n");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sid() -> SandboxId {
        SandboxId::parse("s1").unwrap_or_else(|_| unreachable!())
    }

    fn span(
        kind: &str,
        name: &str,
        start: i64,
        end: Option<i64>,
        outcome: Option<&str>,
    ) -> SpanRecord {
        SpanRecord {
            sandbox_id: sid(),
            subject_kind: SubjectKind::CiRun,
            subject_id: "r".into(),
            key: format!("{kind}@{start}"),
            parent_key: None,
            kind: kind.into(),
            name: name.into(),
            environment: None,
            job: None,
            started_at: Timestamp(start),
            ended_at: end.map(Timestamp),
            outcome: outcome.map(str::to_owned),
            source: "recorded".into(),
            detail: None,
        }
    }

    #[test]
    fn a_job_s_output_becomes_stages_under_its_span_timed_by_yulius() {
        let mut stages = OutputStages::ci_job(sid(), "r1", "job:j1@5", "quality");
        let ops = stages.feed(b"::yulius-stage lint\nok\n::yulius-stage un", Timestamp(10));
        assert_eq!(ops.len(), 1);
        let SpanOp::Start(lint) = &ops[0] else {
            panic!("{ops:?}")
        };
        assert_eq!(
            (
                lint.key.as_str(),
                lint.parent_key.as_deref(),
                lint.kind.as_str(),
                lint.name.as_str()
            ),
            ("job:j1@5/stage:1", Some("job:j1@5"), "stage", "lint")
        );
        assert_eq!(
            (lint.started_at, lint.job.as_deref()),
            (Timestamp(10), Some("quality"))
        );
        let ops = stages.feed(
            b"it\n     Summary [ 1s] 3 tests run: 3 passed (1 flaky)\n",
            Timestamp(20),
        );
        assert_eq!(
            ops,
            vec![
                SpanOp::End {
                    key: "job:j1@5/stage:1".into(),
                    at: Timestamp(20),
                    outcome: None
                },
                SpanOp::Start(Box::new(NewSpan {
                    started_at: Timestamp(20),
                    ..stages.span(
                        "job:j1@5/stage:2".into(),
                        Some("job:j1@5".into()),
                        "stage",
                        "unit",
                        Timestamp(20)
                    )
                })),
                SpanOp::Figures {
                    key: "job:j1@5/stage:2".into(),
                    figures: vec![("tests_passed".into(), 3.0), ("tests_flaky".into(), 1.0)],
                    add: true
                },
            ]
        );
    }

    #[test]
    fn a_job_s_stages_are_bounded() {
        let mut stages = OutputStages::ci_job(sid(), "r1", "job:j1@5", "quality");
        let text: String = (0..MAX_OUTPUT_STAGES + 50)
            .map(|i| format!("::yulius-stage s{i}\n"))
            .collect();
        let starts = stages
            .feed(text.as_bytes(), Timestamp(1))
            .into_iter()
            .filter(|op| matches!(op, SpanOp::Start(_)))
            .count();
        assert_eq!(starts, MAX_OUTPUT_STAGES);
    }

    /// Review H2: output that repeats figures (a flood of summaries and measures) becomes one
    /// write per span and read, added up or the last value kept, so it never floods the store.
    #[test]
    fn repeated_figures_become_one_write_per_span_and_read() {
        let mut stages = OutputStages::ci_job(sid(), "r1", "job:j1@5", "quality");
        let mut text = String::from("::yulius-stage unit\n");
        for _ in 0..10_000 {
            text.push_str("1 passed\n::yulius-measure cache_hits=1\n");
        }
        text.push_str("::yulius-measure cache_hits=7\n");
        let ops = stages.feed(text.as_bytes(), Timestamp(1));
        let figures: Vec<&SpanOp> = ops
            .iter()
            .filter(|op| matches!(op, SpanOp::Figures { .. }))
            .collect();
        assert_eq!(
            figures,
            [
                &SpanOp::Figures {
                    key: "job:j1@5/stage:1".into(),
                    figures: vec![("tests_passed".into(), 10_000.0)],
                    add: true
                },
                &SpanOp::Figures {
                    key: "job:j1@5/stage:1".into(),
                    figures: vec![("cache_hits".into(), 7.0)],
                    add: false
                },
            ]
        );
    }

    #[test]
    fn a_cd_run_s_phases_hold_their_stages() {
        let mut stages = OutputStages::cd_run(sid(), "cd1", "dev");
        let ops = stages.feed(b"[yulius] deploy: d\n::yulius-stage build\n[yulius] verify: v\n[yulius] verify failed with status 1\n", Timestamp(7));
        let keys: Vec<(String, Option<String>)> = ops
            .iter()
            .map(|op| match op {
                SpanOp::Start(s) => (format!("start {}", s.key), s.parent_key.clone()),
                SpanOp::End { key, outcome, .. } => (format!("end {key}"), outcome.clone()),
                SpanOp::Figures { key, .. } => (format!("figures {key}"), None),
            })
            .collect();
        assert_eq!(
            keys,
            vec![
                ("start run/deploy".into(), Some("run".into())),
                ("start run/deploy/stage:1".into(), Some("run/deploy".into())),
                ("end run/deploy/stage:1".into(), None),
                ("end run/deploy".into(), Some("passed".into())),
                ("start run/verify".into(), Some("run".into())),
                ("end run/verify".into(), Some("failed".into())),
            ]
        );
    }

    #[test]
    fn the_summary_has_percentiles_trends_and_leaves_reused_results_out() {
        let day = DAY_MS;
        let mut spans: Vec<SpanRecord> = (1..=10)
            .map(|i| {
                span(
                    "run",
                    "run",
                    day + i * 1000,
                    Some(day + i * 1000 + i * 60_000),
                    Some(if i == 10 { "failed" } else { "passed" }),
                )
            })
            .collect();
        let mut reused = span("run", "run", day, Some(day), Some("passed"));
        reused.detail = Some(r#"{"reused":true}"#.into());
        spans.push(reused);
        spans.push(span("run", "run", 2 * day, None, None));
        let mut queue = span("queue", "queue", day, Some(day + 120_000), Some("passed"));
        queue.detail = Some(r#"{"reason":"waiting for disk space: 9 of 8 GiB used"}"#.into());
        spans.push(queue);
        let summary = summarize(&spans, &[], &[], (1, 2), None, false);
        let run = summary
            .stages
            .iter()
            .find(|r| r.kind == "run")
            .unwrap_or_else(|| unreachable!());
        assert_eq!((run.count, run.failed, run.running), (10, 1, 1));
        assert_eq!(
            (run.p50_ms, run.p90_ms, run.max_ms),
            (Some(300_000), Some(540_000), Some(600_000))
        );
        assert_eq!(summary.reused_runs, 1);
        assert!(
            summary
                .stages
                .iter()
                .any(|r| r.name == "queue · waiting for disk" && r.count == 1)
        );
        assert_eq!(summary.trend.days.len(), 2);
        assert_eq!(summary.trend.days[0].date, "1970-01-02");
        assert_eq!(
            (summary.trend.days[0].count, summary.trend.days[0].p50_ms),
            (10, Some(300_000))
        );
        assert_eq!(summary.trend.days[1].count, 0);
        assert_eq!(summary.delivery.len(), 2);
        assert_eq!(summary.delivery[0].change_failure_rate, None);
    }

    #[test]
    fn delivery_figures_come_from_finished_cd_runs_and_merges() {
        let h = 3_600_000;
        let mut runs = Vec::new();
        for (start, outcome) in [
            (DAY_MS + 2 * h, "passed"),
            (DAY_MS + 5 * h, "failed"),
            (DAY_MS + 6 * h, "passed"),
        ] {
            let mut run = span("run", "run", start, Some(start + h), Some(outcome));
            run.subject_kind = SubjectKind::CdRun;
            run.environment = Some("dev".into());
            runs.push(run);
        }
        let summary = summarize(&[], &runs, &[DAY_MS + h], (1, 1), None, false);
        let dev = &summary.delivery[0];
        assert_eq!(
            (dev.environment.as_str(), dev.deployments, dev.passed),
            ("dev", 3, 2)
        );
        assert_eq!(dev.lead_time_p50_ms, Some(2 * h));
        assert_eq!(dev.time_to_restore_p50_ms, Some(h));
        assert_eq!(summary.delivery[1].deployments, 0);
    }

    #[test]
    fn the_export_escapes_formulas_and_keeps_unknowns_empty() {
        let mut row = span("stage", "=cmd", 0, None, None).view();
        row.detail = serde_json::json!({"a": 1});
        let csv = spans_csv(&[row]);
        let line = csv.lines().nth(1).unwrap_or_default();
        assert!(line.contains(",'=cmd,"), "{line}");
        assert!(line.contains("\"{\"\"a\"\":1}\""), "{line}");
        assert!(csv.starts_with("subject_kind,subject_id,key"));
    }
}
