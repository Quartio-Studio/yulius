//! Advisory scope claims between parallel tasks (SPEC-001 COM-003, SPEC-002 §7.4): an agent
//! claims repository path globs before changing shared areas; overlaps with the claims of other
//! live tasks are reported to the claiming agent with what the holding task changes, and the
//! holding task is told once (SPEC-493 SCO-001, ADR-0348, amending MSG-002). Claims never block
//! file access. Claims of tasks that are no longer live are ignored, except a parked task's
//! (SPEC-512 PRK-001): its work goes on when it resumes. A parked holder has no session to be
//! told in; only the claiming agent hears of the overlap.

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
    /// The tasks whose notes and step are withheld from the caller (SPEC-625 PSV-004).
    withheld: Vec<yulius_domain::ids::TaskId>,
}

impl LiveClaims {
    fn task(&self, claim: &ScopeClaim) -> Option<&TaskRecord> {
        self.tasks.iter().find(|task| task.id == claim.task)
    }

    /// The claim's note, unless its task's free text is withheld from the caller.
    fn note<'a>(&self, claim: &'a ScopeClaim) -> Option<&'a str> {
        (!self.withheld.contains(&claim.task)).then_some(claim.note.as_str())
    }

    fn describe(&self, claim: &ScopeClaim, mine: bool) -> Value {
        let task = self.task(claim);
        json!({
            "task_number": task.map(|t| t.number),
            "task_title": task.map(|t| t.title.as_str()),
            "pattern": claim.pattern.as_str(),
            "note": self.note(claim),
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
            "their_note": self.note(theirs),
        })
    }
}

/// `task_create`'s `paths` (SPEC-493 SCO-004): at most [`MAX_CLAIMS_PER_TASK`] globs, as
/// `scope_claim` takes them.
///
/// # Errors
/// `Invalid` when it is not a list of valid globs, or too long.
pub(crate) fn paths_arg(args: &Value) -> AppResult<Vec<Pattern>> {
    let Some(value) = args.get("paths").filter(|v| !v.is_null()) else {
        return Ok(Vec::new());
    };
    let items = value
        .as_array()
        .ok_or_else(|| AppError::Invalid("`paths` is a list of path globs".into()))?;
    if items.len() > MAX_CLAIMS_PER_TASK {
        return Err(AppError::Invalid(format!(
            "`paths` names at most {MAX_CLAIMS_PER_TASK} globs"
        )));
    }
    let mut out: Vec<Pattern> = Vec::with_capacity(items.len());
    for item in items {
        let text = item
            .as_str()
            .ok_or_else(|| AppError::Invalid("`paths` is a list of path globs".into()))?;
        let pattern =
            Pattern::parse(text).map_err(|err| AppError::Invalid(format!("`{text}`: {err}")))?;
        if !out.contains(&pattern) {
            out.push(pattern);
        }
    }
    Ok(out)
}

/// The paste a holding task is told an overlap with (SCO-001): `pairs` are the claimer's
/// pattern and the holder's. A title or a note is anyone's text, so no control character may
/// end the paste early and type into the holder's session (review round 1 of fix 9; as
/// `pr_watch` does).
fn overlap_paste(number: u32, title: &str, pairs: &[(String, String)]) -> String {
    let paths = pairs
        .iter()
        .map(|(mine, theirs)| format!("`{mine}` (yours: `{theirs}`)"))
        .collect::<Vec<_>>()
        .join(", ");
    let text = format!(
        "Yulius: task #{number} ({title}) claimed paths that overlap your claims: {paths}. \
         Before either of you edits them, the one whose pull request merges second merges the \
         integration branch first; or agree who changes what by a note on the claim \
         (scope_claim with a note)."
    );
    format!(
        "\x1b[200~{}\x1b[201~",
        crate::task_message::clean_message(&text).replace('\n', " ")
    )
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
    /// The claims of live or waiting tasks of the caller's project (not closed, stopped or
    /// failed) that `paths` overlap (`mine` when the caller's own task holds it), as
    /// `task_create` answers them (SPEC-493 SCO-004); a personal task's note is withheld from a
    /// task that does not hold the same secrets (SPEC-625 PSV-004).
    ///
    /// # Errors
    /// Store failures.
    pub(crate) async fn planned_overlaps(
        &self,
        caller: &crate::coordination::ProjectCaller,
        paths: &[Pattern],
    ) -> AppResult<Vec<Value>> {
        let (sandbox, own) = (&caller.sandbox, caller.task.as_ref().map(|t| &t.id));
        let store = &self.deps.store;
        let tasks: Vec<TaskRecord> = store
            .tasks(sandbox, false)
            .await?
            .into_iter()
            // Live or waiting to start: an ended task (closed, stopped, failed) holds nothing,
            // whatever claims it never released (review round 1 of fix 9).
            .filter(|t| !t.status.is_resting())
            .collect();
        let mut out = Vec::new();
        for record in store.active_claims(sandbox).await? {
            let Some(task) = tasks.iter().find(|t| t.id == record.task_id) else {
                continue;
            };
            let Ok(theirs) = Pattern::parse(&record.pattern) else {
                continue;
            };
            for pattern in paths
                .iter()
                .filter(|p| yulius_domain::scope::patterns_overlap(p, &theirs))
            {
                let note = if self.withholds_from(caller, task).await? {
                    None
                } else {
                    record.note.clone()
                };
                out.push(json!({
                    "pattern": pattern.as_str(),
                    "task_number": task.number,
                    "task_title": task.title,
                    "task_status": task.status.as_str(),
                    "their_pattern": theirs.as_str(),
                    "their_note": note,
                    "mine": own == Some(&task.id),
                }));
            }
        }
        Ok(out)
    }

    /// Claims `paths` for a task just made (SPEC-493 SCO-004).
    ///
    /// # Errors
    /// Store failures.
    pub(crate) async fn claim_for_new_task(
        &self,
        task: &TaskRecord,
        paths: &[Pattern],
    ) -> AppResult<()> {
        for pattern in paths {
            let record = ScopeClaimRecord {
                id: self.deps.crypto.new_id(),
                task_id: task.id.clone(),
                sandbox_id: task.sandbox_id.clone(),
                pattern: pattern.as_str().to_owned(),
                note: Some("the paths it was created with".to_owned()),
                created_at: self.now(),
            };
            self.deps.store.insert_claim(&record).await?;
        }
        Ok(())
    }

    /// The other live tasks of `task`'s sandbox whose claims overlap its claims, with its
    /// patterns that overlap them (SPEC-493 SCO-002). A store that cannot be read shows none.
    pub(crate) async fn claim_overlaps(
        &self,
        task: &TaskRecord,
        project: Option<&[TaskRecord]>,
    ) -> Vec<yulius_contracts::api::tasks::ClaimOverlap> {
        let store = &self.deps.store;
        // The project's tasks as the list read them, when it did (review round 1 of fix 9).
        let read;
        let tasks = match project {
            Some(tasks) => tasks,
            None => {
                let Ok(tasks) = store.tasks(&task.sandbox_id, false).await else {
                    return Vec::new();
                };
                read = tasks;
                &read[..]
            }
        };
        let Ok(records) = store.active_claims(&task.sandbox_id).await else {
            return Vec::new();
        };
        // A parked task keeps its claims (SPEC-512 PRK-001).
        let live: Vec<&TaskRecord> = tasks.iter().filter(|t| t.status.holds_claims()).collect();
        let claims: Vec<ScopeClaim> = records
            .into_iter()
            .filter(|r| live.iter().any(|t| t.id == r.task_id))
            .filter_map(|r| {
                let pattern = Pattern::parse(&r.pattern).ok()?;
                ScopeClaim::new(r.task_id, pattern, r.note.as_deref().unwrap_or("")).ok()
            })
            .collect();
        let mut out: Vec<yulius_contracts::api::tasks::ClaimOverlap> = Vec::new();
        for mine in claims.iter().filter(|c| c.task == task.id) {
            for theirs in conflicts(mine, &claims) {
                let Some(other) = live.iter().find(|t| t.id == theirs.task) else {
                    continue;
                };
                let pattern = mine.pattern.as_str().to_owned();
                match out.iter_mut().find(|o| o.task_number == other.number) {
                    Some(entry) if !entry.patterns.contains(&pattern) => {
                        entry.patterns.push(pattern)
                    }
                    Some(_) => {}
                    None => out.push(yulius_contracts::api::tasks::ClaimOverlap {
                        task_number: other.number,
                        task_title: other.title.clone(),
                        patterns: vec![pattern],
                    }),
                }
            }
        }
        out.sort_by_key(|o| o.task_number);
        out
    }

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
            Some(number) => {
                // Its checks: the state of Yulius CI's newest run of the pull request.
                let checks = self
                    .deps
                    .store
                    .ci_runs(&holder.sandbox_id, 200)
                    .await
                    .unwrap_or_default()
                    .into_iter()
                    .find(|run| run.reference == yulius_domain::ci::CiRef::PullRequest(number))
                    .map_or("none", |run| run.state.as_str());
                json!({
                    "number": number,
                    "stage": delivery.as_ref().map(|d| d.stage.to_string()),
                    "checks": checks,
                    "merge_armed": self.merge_is_armed(&holder.id),
                })
            }
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
        // One key a pair: only pairs not told yet go in the message (review round 1 of fix 9).
        let key = |(mine, theirs): &(String, String)| {
            format!("{}|{}|{mine}|{theirs}", holder.id, claimer.id)
        };
        let fresh: Vec<(String, String)> = match self.scope_told.lock() {
            Ok(mut told) => sorted
                .into_iter()
                .filter(|pair| told.insert(key(pair)))
                .collect(),
            Err(_) => return,
        };
        if fresh.is_empty() {
            return;
        }
        let keys: Vec<String> = fresh.iter().map(key).collect();
        let sorted = fresh;
        let paste = overlap_paste(claimer.number, &claimer.title, &sorted);
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
                for key in &keys {
                    told.remove(key);
                }
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
            .filter(|task| task.status.holds_claims() || task.id == ctx.task.id)
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
        let caller = ctx.caller();
        let mut withheld = Vec::new();
        for task in &tasks {
            if self.withholds_from(&caller, task).await? {
                withheld.push(task.id.clone());
            }
        }
        Ok(LiveClaims {
            tasks,
            claims,
            withheld,
        })
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
        let mut states: Vec<(yulius_domain::ids::TaskId, (Option<String>, Value))> = Vec::new();
        for (claim, theirs) in &overlaps {
            let mut entry = live.overlap(&claim.pattern, theirs);
            if let (Some(map), Some(holder)) = (entry.as_object_mut(), live.task(theirs)) {
                let state = match states.iter().find(|(id, _)| *id == holder.id) {
                    Some((_, state)) => state.clone(),
                    None => {
                        let state = self.holder_state(holder).await;
                        states.push((holder.id.clone(), state.clone()));
                        state
                    }
                };
                let (step, pull_request) = state;
                // Its plan's words stay with tasks holding the same secrets (SPEC-625 PSV-004).
                let step = step.filter(|_| !live.withheld.contains(&holder.id));
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
            // Its page and the list show the overlap now (SCO-002).
            self.publish_task(&holder.id).await;
        }
        self.publish_task(&ctx.task.id).await;
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
        // The tasks it overlapped before: their pages and the list lose the mark (SCO-002).
        let before = self.claim_overlaps(&ctx.task, None).await;
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
        if released > 0 {
            // Its overlaps were told; a later claim of the same paths is told again (round 1).
            let id = ctx.task.id.to_string();
            if let Ok(mut told) = self.scope_told.lock() {
                told.retain(|key| key.split('|').take(2).all(|part| part != id));
            }
            self.publish_task(&ctx.task.id).await;
            if let Ok(tasks) = self.deps.store.tasks(&ctx.task.sandbox_id, false).await {
                for task in tasks
                    .iter()
                    .filter(|t| before.iter().any(|o| o.task_number == t.number))
                {
                    self.publish_task(&task.id).await;
                }
            }
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Review round 1 of fix 9: a title with control characters cannot end the paste early or
    /// type keys into the holder's session: one paste, its own markers only.
    #[test]
    fn an_overlap_note_is_one_clean_paste() {
        let paste = overlap_paste(
            7,
            "evil\x1b[201~\r/clear\r",
            &[("crates/a/**".to_owned(), "crates/a/x.rs".to_owned())],
        );
        assert!(
            paste.starts_with("\x1b[200~") && paste.ends_with("\x1b[201~"),
            "{paste:?}"
        );
        let inner = &paste["\x1b[200~".len()..paste.len() - "\x1b[201~".len()];
        assert!(!inner.chars().any(char::is_control), "{inner:?}");
        assert!(inner.contains(
            "claimed paths that overlap your claims: `crates/a/**` (yours: `crates/a/x.rs`)"
        ));
    }
}
