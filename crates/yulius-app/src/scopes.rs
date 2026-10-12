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

impl App {
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
    /// claims are returned to the agent (AC-COM-3); no message is sent. A pattern the task
    /// already holds is not repeated.
    ///
    /// # Errors
    /// `Invalid` patterns or note; store failures.
    pub async fn tool_scope_claim(&self, ctx: &AgentContext, args: &Value) -> AppResult<Value> {
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
        // Every overlap is reported to this agent only; no message goes to the other tasks
        // (MSG-002).
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

        let reported: Vec<Value> = overlaps
            .iter()
            .map(|(claim, theirs)| live.overlap(&claim.pattern, theirs))
            .collect();
        let advice = if reported.is_empty() {
            "No overlap with other tasks. Claims are advisory; release them with scope_release when that work is done."
        } else {
            "These paths overlap other tasks' claims. Avoid editing them while those claims are live, or choose different paths; read the other task with task_get."
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
