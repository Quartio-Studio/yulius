//! Agents deliver their own work (SPEC-001 DLV-008, GIT-002; SPEC-002 ADR-19): `delivery_merge`,
//! `delivery_deploy`, `delivery_promote`, and `issue_create` for follow-up work, bugs and flaky
//! tests, refused unless the issue meets the Definition of Ready (one `type:` and one `priority:`
//! label, at least one `area:` label, and acceptance criteria as a checklist).
//!
//! An agent acts with the authority of the person who created its task, through the same
//! delivery paths and machine checks a person's action takes (the delivery profile, the PR's
//! checks on the exact head, a valid spec, no open steps, an up-to-date head, environments in
//! order). On top of them an agent's merge needs an independent review of the exact head it
//! merges (`reviewed_head`, the head commit the fresh-session reviewer approved) and checks that
//! passed, never merely "none" when the profile requires checks. An environment that a person
//! must verify (`verify = "manual"`, or no command) is never confirmed by an agent. Every agent
//! action is also audited as `task:<number>`.
//!
//! On GitHub an agent acts with its task creator's own token, so merges, promotions, issues and
//! labels are theirs; when they have no usable GitHub token (no linked account, an expired
//! sign-in), the App acts and the task's terminal says so once (F 2026-10-11).

use std::sync::Arc;

use serde_json::{Value, json};
use yulius_contracts::api::git::RunEnvironment;

use crate::auth::AuthContext;
use crate::context::App;
use crate::coordination::AgentContext;
use crate::error::{AppError, AppResult};

/// Longest review summary kept in the audit.
const MAX_REVIEW_SUMMARY_CHARS: usize = 500;

/// Why an issue is not ready (Definition of Ready, DLV-008), or `None`.
#[must_use]
pub fn definition_of_ready_problem(labels: &[String], body: &str) -> Option<String> {
    let count = |prefix: &str| {
        labels
            .iter()
            .filter(|l| l.to_ascii_lowercase().starts_with(prefix))
            .count()
    };
    if count("type:") != 1 {
        return Some(
            "give exactly one `type:` label (bug, enhancement, docs, chore, performance)".into(),
        );
    }
    if count("priority:") != 1 {
        return Some("give exactly one `priority:` label (high, medium, low)".into());
    }
    if count("area:") == 0 {
        return Some("give at least one `area:` label".into());
    }
    let mut in_criteria = false;
    let mut items = 0;
    for line in body.lines() {
        let trimmed = line.trim();
        if let Some(heading) = trimmed.strip_prefix('#') {
            in_criteria = heading
                .trim_start_matches('#')
                .trim()
                .eq_ignore_ascii_case("acceptance criteria");
        } else if in_criteria
            && (trimmed.starts_with("- [ ]") || trimmed.starts_with("- [x]"))
            && trimmed.len() > 5
        {
            items += 1;
        }
    }
    (items == 0)
        .then(|| "add a `## Acceptance criteria` section with at least one `- [ ]` item".to_owned())
}

fn str_arg<'a>(args: &'a Value, key: &str) -> AppResult<&'a str> {
    args.get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .ok_or_else(|| AppError::Invalid(format!("missing string argument `{key}`")))
}

/// Whether a full or abbreviated commit SHA names `head` (at least 7 hex characters).
fn same_commit(given: &str, head: &str) -> bool {
    let given = given.to_ascii_lowercase();
    given.len() >= 7
        && given.len() <= head.len()
        && given.bytes().all(|b| b.is_ascii_hexdigit())
        && head.to_ascii_lowercase().starts_with(&given)
}

impl App {
    /// The task creator's authority, under which an agent delivers.
    async fn agent_authority(&self, ctx: &AgentContext) -> AppResult<AuthContext> {
        let user = self
            .deps
            .store
            .user(&ctx.task.created_by)
            .await?
            .filter(|u| u.disabled_at.is_none())
            .ok_or_else(|| {
                AppError::Forbidden(
                    "the person who created this task is no longer active; nobody can deliver \
                     for it"
                        .into(),
                )
            })?;
        Ok(AuthContext {
            user,
            csrf_token: None,
            session_hash: None,
        })
    }

    async fn agent_audit(&self, ctx: &AgentContext, action: &str, target: &str, detail: &str) {
        if let Err(err) = self
            .deps
            .store
            .audit(self.now(), &ctx.actor(), action, Some(target), Some(detail))
            .await
        {
            tracing::warn!(error = %err, task = %ctx.task.id, "agent delivery audit failed");
        }
    }

    /// `delivery_merge`: merges this task's pull request on green (DLV-008).
    ///
    /// # Errors
    /// `Invalid` arguments; `Conflict` while the review does not cover the head or the checks
    /// have not passed; every refusal of the delivery path (`merge_task_pr`).
    pub async fn tool_delivery_merge(&self, ctx: &AgentContext, args: &Value) -> AppResult<Value> {
        let reviewed = str_arg(args, "reviewed_head")?;
        let summary: String = args
            .get("review_summary")
            .and_then(Value::as_str)
            .unwrap_or("")
            .chars()
            .take(MAX_REVIEW_SUMMARY_CHARS)
            .collect();
        let auth = self.agent_authority(ctx).await?;
        let task_id = ctx.task.id.to_string();
        let worktree = ctx
            .task
            .worktree_path
            .as_deref()
            .ok_or_else(|| AppError::Conflict("this task has no worktree".into()))?;
        let head = self
            .deps
            .git
            .log(std::path::Path::new(worktree), "HEAD", 1)
            .await?
            .into_iter()
            .next()
            .map(|c| c.sha)
            .ok_or_else(|| AppError::Conflict("the task branch has no commit".into()))?;
        if !same_commit(reviewed, &head) {
            return Err(AppError::Conflict(format!(
                "the independent review covered {reviewed}, but the branch is at {head}; review \
                 the new head in a fresh session first"
            )));
        }
        // The agent hears this call's own answer: an earlier armed merge's outcome is old news.
        self.forget_merge_outcome(&ctx.task);
        let before = self.task_delivery(&auth, &task_id).await?;
        let checks = before.pr.as_ref().map_or("none", |p| p.checks.as_str());
        let sandbox = self
            .deps
            .store
            .sandbox(&ctx.task.sandbox_id)
            .await?
            .ok_or(AppError::NotFound)?;
        let require_checks = crate::delivery::delivery_profile(&sandbox).require_checks;
        match checks {
            "success" => {}
            "none" if !require_checks => {}
            "failure" => {
                return Err(AppError::Conflict(
                    "merge on green only: the checks at the head failed; read why (`ci_status`), \
                     fix it test-first and commit"
                        .into(),
                ));
            }
            other => {
                // Still running (or not known yet): Yulius merges this exact head by itself once
                // its checks pass (SPEC-011), so the agent never polls for them.
                self.arm_merge(&ctx.task, &head, &summary);
                self.agent_audit(
                    ctx,
                    "delivery.agent.merge_armed",
                    &format!("{}#{}", sandbox.slug, ctx.task.number),
                    &format!("armed at head {head} (checks {other}); reviewed independently"),
                )
                .await;
                return Ok(json!({
                    "merged": false,
                    "armed": true,
                    "checks": other,
                    "next": "Yulius merges this exact head as soon as its checks pass. Start \
                             `yulius wait` with run_in_background and end your turn: it says how \
                             the merge ended (or why it could not merge); do not call \
                             delivery_merge again for this head.",
                }));
            }
        }
        let view = self.merge_task_pr_for_task(&auth, &task_id).await?;
        // Merged now: nothing is left armed for this task.
        if let Ok(mut armed) = self.runtime(&ctx.task.id).armed_merge.lock() {
            *armed = None;
        }
        self.agent_audit(
            ctx,
            "delivery.agent.merge",
            &format!("{}#{}", sandbox.slug, ctx.task.number),
            &format!(
                "merged at head {head} reviewed independently ({}); for {}",
                if summary.is_empty() {
                    "no summary"
                } else {
                    summary.as_str()
                },
                auth.user.github_login.as_deref().unwrap_or("superadmin")
            ),
        )
        .await;
        Ok(json!({"merged": true, "delivery": view}))
    }

    /// `delivery_deploy`: runs a command-verified environment of the profile (DLV-008).
    ///
    /// # Errors
    /// `Invalid` arguments; `Forbidden` for environments a person must verify; every refusal of
    /// the delivery path (`start_environment_run`).
    pub async fn tool_delivery_deploy(
        self: &Arc<Self>,
        ctx: &AgentContext,
        args: &Value,
    ) -> AppResult<Value> {
        let name = str_arg(args, "environment")?;
        let sandbox = self
            .deps
            .store
            .sandbox(&ctx.task.sandbox_id)
            .await?
            .ok_or(AppError::NotFound)?;
        let profile = crate::delivery::delivery_profile(&sandbox);
        let env = profile
            .environment(name)
            .ok_or_else(|| AppError::Invalid(format!("unknown environment '{name}'")))?;
        if env.command.is_none() || env.verify != yulius_domain::delivery::Verify::Command {
            return Err(AppError::Forbidden(format!(
                "environment '{name}' is verified by a person; give it a deploy command and \
                 `verify = \"command\"` in yulius.toml (skill infra-bootstrap) so agents can \
                 deliver it"
            )));
        }
        let auth = self.agent_authority(ctx).await?;
        let run = self
            .dlv_start_environment_run(
                &auth,
                sandbox.id.as_str(),
                name,
                &RunEnvironment { confirm: true },
                Some(&ctx.task),
            )
            .await?;
        self.agent_audit(
            ctx,
            "delivery.agent.deploy",
            &format!("{}/{name}", sandbox.slug),
            &format!("run {}", run.run_id),
        )
        .await;
        Ok(json!({"run": run}))
    }

    /// `delivery_promote`: promotes the integration branch (DLV-008). With merge-commit
    /// promotion the first call opens the promotion pull request and a later call, once its
    /// checks passed, merges it.
    ///
    /// # Errors
    /// Every refusal of the delivery path (`promote_sandbox`).
    pub async fn tool_delivery_promote(&self, ctx: &AgentContext) -> AppResult<Value> {
        let auth = self.agent_authority(ctx).await?;
        let sandbox = ctx.task.sandbox_id.to_string();
        let result = self.promote_sandbox_for_task(&auth, &ctx.task).await?;
        self.agent_audit(
            ctx,
            "delivery.agent.promote",
            &sandbox,
            &format!(
                "{} {}",
                result.result,
                result.sha.as_deref().unwrap_or_default()
            ),
        )
        .await;
        Ok(json!({"promotion": result}))
    }
}

impl App {
    /// `issue_create`: opens a GitHub issue that meets the Definition of Ready (DLV-008), and
    /// creates its missing labels, as the task's creator, else as the App (F 2026-10-11).
    ///
    /// # Errors
    /// `Invalid` input or an issue that is not ready; GitHub failures.
    pub async fn tool_issue_create(&self, ctx: &AgentContext, args: &Value) -> AppResult<Value> {
        let title = str_arg(args, "title")?.to_owned();
        let body = args
            .get("body")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_owned();
        let labels: Vec<String> = args
            .get("labels")
            .and_then(Value::as_array)
            .map(|l| {
                l.iter()
                    .filter_map(Value::as_str)
                    .map(|s| s.trim().to_owned())
                    .collect()
            })
            .unwrap_or_default();
        if let Some(problem) = definition_of_ready_problem(&labels, &body) {
            return Err(AppError::Invalid(format!(
                "the issue is not ready: {problem}"
            )));
        }
        let sandbox = self
            .deps
            .store
            .sandbox(&ctx.task.sandbox_id)
            .await?
            .ok_or(AppError::NotFound)?;
        let token = match self.creator_github_token(&ctx.task).await {
            Some(token) => token,
            None => self.repository_installation_token(&sandbox).await?,
        };
        let issue = crate::ports::github::NewIssue {
            title,
            body: format!(
                "{body}\n\n---\nOpened by task #{} in Yulius.",
                ctx.task.number
            ),
            labels,
        };
        let opened = self
            .deps
            .github
            .create_issue(&token, &sandbox.repo_owner, &sandbox.repo_name, &issue)
            .await?;
        self.agent_audit(
            ctx,
            "issue.agent.create",
            &format!("{}#{}", sandbox.slug, opened.number),
            &issue.title,
        )
        .await;
        Ok(json!({"number": opened.number, "url": opened.html_url}))
    }
}

#[cfg(test)]
mod tests {
    use super::{definition_of_ready_problem, same_commit};

    #[test]
    fn issues_need_the_definition_of_ready() {
        let labels = |l: &[&str]| l.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>();
        let ready = labels(&["type: bug", "priority: high", "area: backend"]);
        let body = "Flaky.\n\n## Acceptance criteria\n- [ ] Passes 100 runs under load\n";
        assert_eq!(definition_of_ready_problem(&ready, body), None);
        assert!(
            definition_of_ready_problem(&labels(&["priority: high", "area: x"]), body)
                .is_some_and(|p| p.contains("`type:`"))
        );
        assert!(
            definition_of_ready_problem(
                &labels(&["type: bug", "type: docs", "priority: low", "area: x"]),
                body
            )
            .is_some()
        );
        assert!(
            definition_of_ready_problem(&labels(&["type: bug", "area: x"]), body)
                .is_some_and(|p| p.contains("`priority:`"))
        );
        assert!(
            definition_of_ready_problem(&labels(&["type: bug", "priority: low"]), body)
                .is_some_and(|p| p.contains("`area:`"))
        );
        assert!(
            definition_of_ready_problem(&ready, "No criteria here.\n- [ ] loose item")
                .is_some_and(|p| p.contains("Acceptance criteria"))
        );
        assert!(definition_of_ready_problem(&ready, "### Acceptance Criteria\n- [ ]\n").is_some());
    }

    #[test]
    fn reviewed_heads_match_full_or_abbreviated_shas() {
        let head = "0123456789abcdef0123456789abcdef01234567";
        assert!(same_commit(head, head));
        assert!(same_commit("0123456", head));
        assert!(same_commit("0123456789ABCDEF", head));
        assert!(!same_commit("012345", head), "too short to be unambiguous");
        assert!(!same_commit("1234567", head));
        assert!(!same_commit("0123456z", head));
        assert!(!same_commit(&format!("{head}0"), head));
    }
}
