//! Advisory scope claims between parallel tasks (SPEC-001 COM-003, SPEC-002 §7.4): an agent
//! claims repository path globs before changing shared areas; overlaps with the claims of other
//! live tasks are reported to the claiming agent only (nobody else is messaged, MSG-002). Claims never block
//! file access. Claims of tasks that are no longer live are ignored.

use serde_json::{Value, json};
use yulius_domain::scope::{Pattern, ScopeClaim, conflicts};

use crate::context::App;
use crate::coordination::AgentContext;
use crate::error::{AppError, AppResult};
use crate::model::{ScopeClaimRecord, TaskRecord};

/// Most patterns one `scope_claim` call accepts.
pub const MAX_PATTERNS_PER_CLAIM: usize = 20;
/// Most active claims one task holds (#73): claims are compared pairwise.
pub const MAX_CLAIMS_PER_TASK: usize = 50;

fn invalid_pattern(text: &str, err: impl std::fmt::Display) -> AppError {
    let shown: String = text.chars().take(80).collect();
    AppError::Invalid(format!("`{shown}`: {err}"))
}

/// The patterns of a `scope_claim` call: `pattern` (one glob) or `patterns` (a list), deduplicated.
fn requested_patterns(args: &Value) -> AppResult<Vec<Pattern>> {
    let texts: Vec<&str> = match (args.get("patterns"), args.get("pattern")) {
        (Some(Value::Array(items)), None) => items
            .iter()
            .map(|item| {
                item.as_str()
                    .ok_or_else(|| AppError::Invalid("`patterns` must be strings".into()))
            })
            .collect::<AppResult<_>>()?,
        (None, Some(Value::String(one))) => vec![one.as_str()],
        _ => {
            return Err(AppError::Invalid(
                "give `pattern` (one path glob) or `patterns` (a list)".into(),
            ));
        }
    };
    if texts.is_empty() || texts.len() > MAX_PATTERNS_PER_CLAIM {
        return Err(AppError::Invalid(format!(
            "claim 1 to {MAX_PATTERNS_PER_CLAIM} patterns at a time"
        )));
    }
    let mut patterns: Vec<Pattern> = Vec::with_capacity(texts.len());
    for text in texts {
        let pattern = Pattern::parse(text).map_err(|err| invalid_pattern(text, err))?;
        if !patterns.contains(&pattern) {
            patterns.push(pattern);
        }
    }
    Ok(patterns)
}

/// Live tasks of a sandbox (the agent's own task included) and their active claims.
struct LiveClaims {
    tasks: Vec<TaskRecord>,
    claims: Vec<ScopeClaim>,
}

impl LiveClaims {
    fn task(&self, claim: &ScopeClaim) -> Option<&TaskRecord> {
        self.tasks.iter().find(|task| task.id == claim.task)
    }

    fn describe(&self, claim: &ScopeClaim, mine: bool) -> Value {
        let task = self.task(claim);
        json!({
            "task_number": task.map(|t| t.number),
            "task_title": task.map(|t| t.title.as_str()),
            "pattern": claim.pattern.as_str(),
            "note": claim.note,
            "mine": mine,
        })
    }

    /// An overlap between one of this task's patterns and another task's claim.
    fn overlap(&self, pattern: &Pattern, theirs: &ScopeClaim) -> Value {
        let task = self.task(theirs);
        json!({
            "pattern": pattern.as_str(),
            "task_number": task.map(|t| t.number),
            "task_title": task.map(|t| t.title.as_str()),
            "their_pattern": theirs.pattern.as_str(),
            "their_note": theirs.note,
        })
    }
}

/// What a task told of an overlap does (SPEC-493 SCO-003): the same in the claim's answer and
/// the holding task's message.
pub(crate) const OVERLAP_ADVICE: &str = "These paths overlap another live task's claim, and that \
     task is told too. Before editing them, wait for its pull request to merge and merge the \
     integration branch, or agree with it who changes what by a note on the claim (scope_claim \
     with a note).";

/// How often, and how far apart, a holding task is offered its overlap message while its
/// session does not take typed text: about two minutes in all.
const TELL_TRIES: u32 = 12;
const TELL_RETRY: std::time::Duration = std::time::Duration::from_secs(10);

impl App {
    /// What a holding task is changing and where its pull request stands (SCO-001): its step in
    /// progress, and its pull request's stage, number and whether a merge is armed.
    async fn holder_state(&self, holder: &TaskRecord) -> (Option<String>, Value) {
        let steps = self.deps.store.steps(&holder.id).await.unwrap_or_default();
        let step = self
            .step_progress(&holder.id, &steps)
            .await
            .ok()
            .and_then(|progress| progress.current);
        let delivery = self.deps.store.delivery(&holder.id).await.ok().flatten();
        let pull_request = match delivery.as_ref().and_then(|d| d.pr_number) {
            Some(number) => json!({
                "number": number,
                "stage": delivery.as_ref().map(|d| d.stage.to_string()),
                "merge_armed": self.merge_is_armed(&holder.id),
            }),
            None => json!(null),
        };
        (step, pull_request)
    }

    /// Types one message into `holder`'s session naming `claimer` and the overlapping paths
    /// (`(claimer's, holder's)`), unless it was told of these already (SCO-001). A session that
    /// does not take typed text now gets it once it does, within a bound.
    fn tell_holder(
        self: &std::sync::Arc<Self>,
        holder: &TaskRecord,
        claimer: &TaskRecord,
        pairs: &[(String, String)],
    ) {
        let mut sorted = pairs.to_vec();
        sorted.sort();
        sorted.dedup();
        let key = format!(
            "{}|{}|{}",
            holder.id,
            claimer.id,
            sorted
                .iter()
                .map(|(mine, theirs)| format!("{mine}>{theirs}"))
                .collect::<Vec<_>>()
                .join(",")
        );
        if !self
            .scope_told
            .lock()
            .is_ok_and(|mut told| told.insert(key.clone()))
        {
            return;
        }
        let paths = sorted
            .iter()
            .map(|(mine, theirs)| format!("`{mine}` (yours: `{theirs}`)"))
            .collect::<Vec<_>>()
            .join(", ");
        let text = format!(
            "Yulius: task #{} ({}) claimed paths that overlap your claims: {paths}. Before \
             either of you edits them, the one whose pull request merges second merges the \
             integration branch first; or agree who changes what by a note on the claim \
             (scope_claim with a note).",
            claimer.number, claimer.title
        );
        let paste = format!("\x1b[200~{text}\x1b[201~");
        let (app, holder_id) = (std::sync::Arc::clone(self), holder.id.clone());
        tokio::spawn(async move {
            for attempt in 1..=TELL_TRIES {
                let Ok(Some(holder)) = app.deps.store.task(&holder_id).await else {
                    break;
                };
                if !holder.status.is_live() {
                    break;
                }
                match app
                    .type_into_session(
                        &holder,
                        paste.as_bytes(),
                        crate::task_message::Typed::Prompt,
                        crate::task_message::Sender::Yulius,
                    )
                    .await
                {
                    Ok(()) => return,
                    Err(err) if attempt == TELL_TRIES => {
                        tracing::warn!(error = %err, task = %holder_id, "a task was not told of an overlapping claim");
                    }
                    Err(_) => tokio::time::sleep(TELL_RETRY).await,
                }
            }
            // Not told: a later claim of the same paths tries again.
            if let Ok(mut told) = app.scope_told.lock() {
                told.remove(&key);
            }
        });
    }

    async fn live_claims(&self, ctx: &AgentContext) -> AppResult<LiveClaims> {
        let tasks: Vec<TaskRecord> = self
            .deps
            .store
            .tasks(&ctx.task.sandbox_id, false)
            .await?
            .into_iter()
            .filter(|task| task.status.is_live() || task.id == ctx.task.id)
            .collect();
        let claims = self
            .deps
            .store
            .active_claims(&ctx.task.sandbox_id)
            .await?
            .into_iter()
            .filter(|record| tasks.iter().any(|task| task.id == record.task_id))
            .filter_map(|record| {
                let pattern = Pattern::parse(&record.pattern).ok()?;
                ScopeClaim::new(
                    record.task_id,
                    pattern,
                    record.note.as_deref().unwrap_or(""),
                )
                .ok()
            })
            .collect();
        Ok(LiveClaims { tasks, claims })
    }

    /// `scope_claim`: claims repository paths for this task. Overlaps with other live tasks'
    /// claims are returned to the agent with what the holding task is changing and where its
    /// pull request stands, and the holding task is told once (SPEC-493 SCO-001, amending
    /// MSG-002's "no message"). A pattern the task already holds is not repeated.
    ///
    /// # Errors
    /// `Invalid` patterns or note; store failures.
    pub async fn tool_scope_claim(
        self: &std::sync::Arc<Self>,
        ctx: &AgentContext,
        args: &Value,
    ) -> AppResult<Value> {
        let patterns = requested_patterns(args)?;
        let note = args.get("note").and_then(Value::as_str).unwrap_or("");
        let claims: Vec<ScopeClaim> = patterns
            .into_iter()
            .map(|pattern| ScopeClaim::new(ctx.task.id.clone(), pattern, note))
            .collect::<Result<_, _>>()
            .map_err(|err| AppError::Invalid(err.to_string()))?;
        let live = self.live_claims(ctx).await?;
        let held_by_me = |claim: &ScopeClaim| {
            live.claims
                .iter()
                .any(|c| c.task == claim.task && c.pattern == claim.pattern)
        };
        let holding = live.claims.iter().filter(|c| c.task == ctx.task.id).count();
        let adding = claims.iter().filter(|c| !held_by_me(c)).count();
        if holding + adding > MAX_CLAIMS_PER_TASK {
            return Err(AppError::Invalid(format!(
                "a task holds at most {MAX_CLAIMS_PER_TASK} scope claims ({holding} now); release some with scope_release or claim a wider glob"
            )));
        }
        let mut overlaps: Vec<(&ScopeClaim, &ScopeClaim)> = Vec::new();
        for claim in &claims {
            overlaps.extend(
                conflicts(claim, &live.claims)
                    .into_iter()
                    .map(|theirs| (claim, theirs)),
            );
            if !held_by_me(claim) {
                let record = ScopeClaimRecord {
                    id: self.deps.crypto.new_id(),
                    task_id: claim.task.clone(),
                    sandbox_id: ctx.task.sandbox_id.clone(),
                    pattern: claim.pattern.as_str().to_owned(),
                    note: (!claim.note.is_empty()).then(|| claim.note.clone()),
                    created_at: self.now(),
                };
                self.deps.store.insert_claim(&record).await?;
            }
        }

        let mut reported: Vec<Value> = Vec::with_capacity(overlaps.len());
        for (claim, theirs) in &overlaps {
            let mut entry = live.overlap(&claim.pattern, theirs);
            if let (Some(map), Some(holder)) = (entry.as_object_mut(), live.task(theirs)) {
                let (step, pull_request) = self.holder_state(holder).await;
                map.insert("their_step".to_owned(), json!(step));
                map.insert("their_pull_request".to_owned(), pull_request);
            }
            reported.push(entry);
        }
        // Each holding task is told once of this task and the paths (SCO-001).
        let mut by_holder: Vec<(&TaskRecord, Vec<(String, String)>)> = Vec::new();
        for (claim, theirs) in &overlaps {
            let Some(holder) = live.task(theirs) else {
                continue;
            };
            let pair = (
                claim.pattern.as_str().to_owned(),
                theirs.pattern.as_str().to_owned(),
            );
            match by_holder.iter_mut().find(|(task, _)| task.id == holder.id) {
                Some((_, pairs)) => pairs.push(pair),
                None => by_holder.push((holder, vec![pair])),
            }
        }
        for (holder, pairs) in by_holder {
            self.tell_holder(holder, &ctx.task, &pairs);
        }
        let advice = if reported.is_empty() {
            "No overlap with other tasks. Claims are advisory; release them with scope_release when that work is done."
        } else {
            OVERLAP_ADVICE
        };
        Ok(json!({
            "claimed": claims.iter().map(|c| c.pattern.as_str()).collect::<Vec<_>>(),
            "conflicts": reported,
            "advice": advice,
        }))
    }

    /// `scope_release`: releases this task's claims named by `pattern` (one glob) or `patterns`
    /// (a list), or all of them when neither is given. Any other shape is refused rather than
    /// read as "all" (#73).
    ///
    /// # Errors
    /// `Invalid` arguments or patterns; store failures.
    pub async fn tool_scope_release(&self, ctx: &AgentContext, args: &Value) -> AppResult<Value> {
        let requested = match (args.get("pattern"), args.get("patterns")) {
            (None, None) => None,
            (Some(_), None) | (None, Some(Value::Array(_))) => Some(requested_patterns(args)?),
            _ => {
                return Err(AppError::Invalid(
                    "give `pattern` (one path glob), `patterns` (a list), or nothing to release every claim".into(),
                ));
            }
        };
        let released = match requested {
            None => {
                self.deps
                    .store
                    .release_claims(&ctx.task.id, None, self.now())
                    .await?
            }
            Some(patterns) => {
                let mut released = 0;
                for pattern in &patterns {
                    released += self
                        .deps
                        .store
                        .release_claims(&ctx.task.id, Some(pattern.as_str()), self.now())
                        .await?;
                }
                released
            }
        };
        Ok(json!({"released": released}))
    }

    /// `scopes_list`: the live tasks' claims (this task's marked) and the overlaps with this
    /// task's claims.
    ///
    /// # Errors
    /// Store failures.
    pub async fn tool_scopes_list(&self, ctx: &AgentContext) -> AppResult<Value> {
        let live = self.live_claims(ctx).await?;
        let claims: Vec<Value> = live
            .claims
            .iter()
            .map(|claim| live.describe(claim, claim.task == ctx.task.id))
            .collect();
        let overlaps: Vec<Value> = live
            .claims
            .iter()
            .filter(|claim| claim.task == ctx.task.id)
            .flat_map(|mine| {
                conflicts(mine, &live.claims)
                    .into_iter()
                    .map(|theirs| live.overlap(&mine.pattern, theirs))
            })
            .collect();
        Ok(json!({"claims": claims, "conflicts": overlaps}))
    }
}
