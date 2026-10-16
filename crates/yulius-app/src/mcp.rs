//! Yulius MCP server: JSON-RPC dispatch for agent tools (SPEC-002 §7.4).

use std::sync::Arc;

use serde_json::{Value, json};
use yulius_contracts::mcp::{
    RpcRequest, RpcResponse, SUPPORTED_PROTOCOL_VERSIONS, ToolDescriptor, text_result,
};
use yulius_domain::ids::SandboxId;
use yulius_domain::vault::redact::redact_text;

use crate::context::App;
use crate::coordination::{AgentContext, ProjectCaller};
use crate::error::{AppError, AppResult};
use crate::manage::TaskChange;
use crate::shells::COCKPIT_TOKEN_PREFIX;

/// Maximum tool result text returned to the model.
pub const MAX_RESULT_BYTES: usize = 256 * 1024;

fn schema(properties: Value, required: &[&str]) -> Value {
    json!({"type": "object", "properties": properties, "required": required, "additionalProperties": false})
}

/// The tools a Cockpit is served: managing the project's tasks and automations (SPEC-037
/// MGT-006, SPEC-066 MGT-012) and reading its agent vault (SPEC-178, ADR-0144). Everything else
/// belongs to a task.
pub const COCKPIT_TOOLS: [&str; 17] = [
    "tasks_list",
    "task_get",
    "task_create",
    "task_stop",
    "task_archive",
    "task_delete",
    "automation_create",
    "automations_list",
    "automation_update",
    "automation_delete",
    "automation_run_now",
    "automation_template_enable",
    "workflows_list",
    "sessions_search",
    "session_read",
    "secrets_list",
    "secret_verify",
];

/// Who calls the MCP server.
enum McpCaller {
    /// A task's agent (task token): every tool.
    Agent(AgentContext),
    /// A person's Cockpit (`ck_` token): [`COCKPIT_TOOLS`] only.
    Cockpit(ProjectCaller),
}

impl McpCaller {
    fn project(&self) -> ProjectCaller {
        match self {
            Self::Agent(ctx) => ctx.caller(),
            Self::Cockpit(caller) => caller.clone(),
        }
    }
}

/// Tool descriptors served to a Cockpit by `tools/list`.
#[must_use]
pub fn cockpit_tool_descriptors() -> Vec<ToolDescriptor> {
    tool_descriptors()
        .into_iter()
        .chain(cockpit_only_tool_descriptors())
        .filter(|tool| COCKPIT_TOOLS.contains(&tool.name.as_str()))
        .collect()
}

/// Tools only a Cockpit is served (SPEC-178, ADR-0144): `secret_verify`, whose input is, on a
/// match, the value's SHA-256. The Superagent's conversation is its person's own; a task's
/// transcript is read by every member who can view the project.
fn cockpit_only_tool_descriptors() -> Vec<ToolDescriptor> {
    vec![
        ToolDescriptor {
            name: "secret_verify".into(),
            description: "Check whether a value the person holds (a key in a file, one they pasted into a file) is an agent secret's value, without anyone seeing it: compute the candidate's SHA-256 in a shell without printing the value (sha256sum key.pem, or printf %s \"$VAR\" | sha256sum) and pass that hash (or sha256sum's line). Answers match: exact, trailing_newline (the candidate ends with a line break), escaped_newlines (the candidate has line breaks where the vault holds literal \\n, as a multi-line key is stored) or different, and on a match the lines the value stands for. Never returns the value. Audited.".into(),
            input_schema: schema(json!({
                "secret": {"type": "string", "description": "Secret NAME from secrets_list"},
                "sha256": {"type": "string", "description": "The candidate value's SHA-256, 64 hexadecimal digits"}
            }), &["secret", "sha256"]),
        },
    ]
}

/// Tool descriptors served to a task's agent by `tools/list`.
#[must_use]
pub fn tool_descriptors() -> Vec<ToolDescriptor> {
    let task_ref = json!({
        "task_id": {"type": "string", "description": "Task id"},
        "number": {"type": "integer", "description": "Task number within this sandbox"}
    });
    let mut tools = vec![
        ToolDescriptor {
            name: "project_context".into(),
            description: "The project context, rebuilt now: project summary, architecture and decision index, recent changes, delivery rules and the parallel tasks with their scope claims (the same briefing the session started with).".into(),
            input_schema: schema(json!({}), &[]),
        },
        ToolDescriptor {
            name: "tasks_list".into(),
            description: "List previous and parallel tasks of this project (other Claude Code sessions): status, branch, spec and step progress. Use it before starting work to avoid duplicating or conflicting with others.".into(),
            input_schema: schema(json!({"scope": {"type": "string", "enum": ["all", "active", "recent", "archived"]}, "limit": {"type": "integer"}}), &[]),
        },
        ToolDescriptor {
            name: "task_get".into(),
            description: "Get one task's details: prompt, spec path, branch and its step list.".into(),
            input_schema: schema(task_ref.clone(), &[]),
        },
        ToolDescriptor {
            name: "task_create".into(),
            description: "Create and start a new task in this project (another Claude Code session with its own branch), e.g. when the user says \"create a task from that\" or a follow-up should run separately. It is created like New task in the UI, on behalf of the person you work for; write the prompt as a complete request (what, why, where, done when). With after_task it waits until that task is done; with start_rule until the rule is true; prepared, it waits until the person starts it, or until start_at. A supertask (for broad work such as security hardening or one-shotting the product) waits for the project's running tasks, then runs alone: every other task of the project waits for it, so create one only when the person asks. Tools create at most 20 tasks an hour per project.".into(),
            input_schema: schema(json!({
                "prompt": {"type": "string", "description": "The request for the new task"},
                "title": {"type": "string", "description": "Short title (derived from the prompt when absent)"},
                "workflow": {"type": "string", "description": "Workflow id or name from workflows_list, or \"auto\" to let Yulius choose"},
                "mode": {"type": "string", "enum": ["goal", "prompt", "auto"], "description": "goal (default): the task works until its definition of done holds; prompt: one plain prompt"},
                "after_task": {"type": "integer", "description": "Task number to wait for (starts when it is done)"},
                "start_rule": {"type": "string", "description": "Start when this rule over the project's tasks is true, e.g. \"#12 done and #13 done\" (not with after_task)"},
                "supertask": {"type": "boolean", "description": "Run alone in the project: wait for the running tasks, and every other task waits for this one (not with after_task or start_rule)"},
                "prepared": {"type": "boolean", "description": "Prepare it without starting: it waits until the person presses Start now (not with after_task, start_rule or supertask)"},
                "start_at": {"type": "string", "description": "Prepare it and start it by itself at this RFC 3339 time, e.g. \"2026-10-15T09:00:00+02:00\" (at most 366 days ahead)"},
                "model": {"type": "string", "description": "Model override (default Opus)"},
                "effort": {"type": "string", "description": "Effort override (default high)"}
            }), &["prompt"]),
        },
        ToolDescriptor {
            name: "task_stop".into(),
            description: "Stop another task of this project (by task_id or number), as Stop in the UI: a queued or waiting task is cancelled, a running one is interrupted. Not your own task. Acts on behalf of the person you work for and is audited.".into(),
            input_schema: schema(task_ref.clone(), &[]),
        },
        ToolDescriptor {
            name: "task_archive".into(),
            description: "Archive another task of this project (by task_id or number), as Archive in the UI: only a stopped or finished task (a Done one is closed first); archived: false restores it. Not your own task. Audited.".into(),
            input_schema: schema(json!({
                "task_id": {"type": "string", "description": "Task id"},
                "number": {"type": "integer", "description": "Task number within this sandbox"},
                "archived": {"type": "boolean", "description": "false restores an archived task (default true)"}
            }), &[]),
        },
        ToolDescriptor {
            name: "task_delete".into(),
            description: "Delete another task of this project permanently (by task_id or number), as Delete in the UI: its local worktree and branch (uncommitted work included), runs and transcripts go; remote branches and pull requests stay. A running task must be stopped first (a Done one is closed first). Not your own task. Audited.".into(),
            input_schema: schema(task_ref.clone(), &[]),
        },
        ToolDescriptor {
            name: "sessions_search".into(),
            description: "Sessions are this project's docs: search the transcripts of earlier and parallel Yulius tasks and the Claude Code sessions people imported from their computers (full text: words from file names, functions, errors, features). Use it whenever you do not know how or why something was implemented, before guessing or re-deriving. Returns sessions with matching passages and their line numbers; read around a match with session_read.".into(),
            input_schema: schema(json!({
                "query": {"type": "string", "description": "Words to find (all must match; falls back to any)"},
                "kind": {"type": "string", "enum": ["all", "task", "imported"], "description": "Only task sessions or only imported ones (default all)"},
                "limit": {"type": "integer", "minimum": 1, "maximum": crate::session_docs::MAX_SEARCH_RESULTS, "description": "Most sessions (default 8)"}
            }), &["query"]),
        },
        ToolDescriptor {
            name: "session_read".into(),
            description: "Read part of one session of this project: by `id` from sessions_search, or the latest session of another task by `task_number`. Returns messages, tool calls, edits and results from line `from` (bounded); continue with `next_from`.".into(),
            input_schema: schema(json!({
                "id": {"type": "string", "description": "Session id from sessions_search"},
                "task_number": {"type": "integer", "description": "Read this task's latest session instead"},
                "from": {"type": "integer", "minimum": 0, "description": "First transcript line (default 0; use a match's line minus a few)"},
                "limit": {"type": "integer", "minimum": 1, "maximum": 200, "description": "Most entries (default 60)"},
                "max_chars": {"type": "integer", "minimum": 1000, "maximum": crate::session_docs::READ_CHARS_MAX, "description": "Most characters of text (default 24000)"}
            }), &[]),
        },
        ToolDescriptor {
            name: "scope_claim".into(),
            description: "Claim repository paths this task is about to change, so parallel tasks avoid conflicts: path globs relative to the repository (* within a segment, ** across segments, a trailing / claims a directory, e.g. \"src/api/\", \"docs/**/*.md\"). Claims are advisory and never block files. Overlaps with other live tasks' claims are returned to you; nobody else is told.".into(),
            input_schema: schema(json!({
                "pattern": {"type": "string", "description": "One path glob"},
                "patterns": {"type": "array", "items": {"type": "string"}, "maxItems": crate::scopes::MAX_PATTERNS_PER_CLAIM, "description": "Several path globs"},
                "note": {"type": "string", "description": "Why this task needs these paths"}
            }), &[]),
        },
        ToolDescriptor {
            name: "scope_release".into(),
            description: "Release this task's scope claims named by pattern (one glob) or patterns (a list), or all of them when neither is given, e.g. when the work on those paths is done.".into(),
            input_schema: schema(json!({
                "pattern": {"type": "string"},
                "patterns": {"type": "array", "items": {"type": "string"}, "maxItems": crate::scopes::MAX_PATTERNS_PER_CLAIM}
            }), &[]),
        },
        ToolDescriptor {
            name: "scopes_list".into(),
            description: "Current scope claims of the live tasks in this project (yours marked mine) and the overlaps with your claims.".into(),
            input_schema: schema(json!({}), &[]),
        },
        ToolDescriptor {
            name: "record_number".into(),
            description: "The number for a new numbered file (a decision record, a migration, any file named NNNN-… or PREFIX-NNN-…) in dir: Yulius gives each task its own number, past every such file on the integration branch, in every parallel task and every number given out before, and reserves it for this task. Use it instead of counting the files yourself: parallel tasks counting the same files take the same number and collide when both merge. Name the file with the returned number at once; asking again before that file exists returns the same number.".into(),
            input_schema: schema(json!({
                "dir": {"type": "string", "description": "Repository-relative directory of the numbered files, e.g. docs/decisions"},
                "prefix": {"type": "string", "description": "The text before the number when the directory mixes series, e.g. SPEC- (default: the directory's main series)"}
            }), &["dir"]),
        },
        ToolDescriptor {
            name: "spec_check".into(),
            description: "Validate this task's spec in the Yulius format (frontmatter, required sections, a Plan of at most 30 unique steps) and the project documentation; says whether this task needs a spec at all (features and fixes do; research, questions, docs and chores do not) and returns the problems to fix plus nudges about the plan. The same check runs when you stop.".into(),
            input_schema: schema(json!({}), &[]),
        },
        ToolDescriptor {
            name: "steps_set".into(),
            description: "Replace this task's plan with an ordered list of 1-30 steps of similar effort (strings or {title, status, effort}), each a slice that leaves the project working, named by what it delivers. Keep it in sync with the Plan checklist of your spec. effort is 1 (default) to 3, only for a step that cannot be split; progress is weighted by it. Statuses: pending, in_progress, done, skipped (blocked needs step_update with a note). A step matching a current title keeps its status and effort; a finished step stays finished.".into(),
            input_schema: schema(json!({"steps": {"type": "array", "maxItems": crate::coordination::MAX_STEPS, "items": {"anyOf": [{"type": "string"}, {"type": "object", "properties": {"title": {"type": "string"}, "status": {"type": "string", "enum": ["pending", "in_progress", "done", "skipped"]}, "effort": {"type": "integer", "minimum": 1, "maximum": 3}}, "required": ["title"]}]}}}), &["steps"]),
        },
        ToolDescriptor {
            name: "step_update".into(),
            description: "Update one step (by step_id, ordinal or title). Mark a step in_progress when you start it and done the moment it is finished, one at a time (at most 2 in progress), so progress arrives steadily. A finished step stays finished: add a follow-up step with steps_set instead. blocked and skipped need a note saying why. Answers with the step's new state, the progress, the next step and nudges (steps_get shows the whole plan).".into(),
            input_schema: schema(json!({"step_id": {"type": "string"}, "ordinal": {"type": "integer"}, "title": {"type": "string"}, "status": {"type": "string", "enum": ["pending", "in_progress", "done", "skipped", "blocked"]}, "note": {"type": "string"}}), &["status"]),
        },
        ToolDescriptor {
            name: "steps_get".into(),
            description: "Get this task's plan as a checklist (ordinal, status, title, effort, note), the effort-weighted progress (percent) and nudges about the plan and its pace.".into(),
            input_schema: schema(json!({}), &[]),
        },
        ToolDescriptor {
            name: "delivery_status".into(),
            description: "Delivery state of this task: pull request, checks, spec validity, open steps, the project's delivery profile and the next actions. Deliver with delivery_merge; deploys are CD runs (cd_status, cd_deploy).".into(),
            input_schema: schema(json!({}), &[]),
        },
        ToolDescriptor {
            name: "ci_status".into(),
            description: "This project's CI, run by Yulius (the only CI; no GitHub Actions): the jobs in force and where they come from, the latest runs (this task's pull request first) and their state. With run_id, one run with, for each job that failed or errored, its failing tests' output (assertion messages, stdout and stderr, kept from all of its output) and the end of its output, and for each skipped job (it checked nothing; it blocks no merge) the end of its output, which says why; with run_id and job, that job's output whatever its state. A merge that requires checks waits until the latest run at the pull request head passed.".into(),
            input_schema: schema(json!({
                "run_id": {"type": "string", "description": "A run id from the list, to read its jobs' output"},
                "job": {"type": "string", "description": "With run_id: the name of the job whose output to show"}
            }), &[]),
        },
        ToolDescriptor {
            name: "ci_rerun".into(),
            description: "Run the CI again for the commit of a finished run (for example after a failure that was not caused by the code). Refused while a run of that commit is queued or running.".into(),
            input_schema: schema(json!({"run_id": {"type": "string"}}), &["run_id"]),
        },
        ToolDescriptor {
            name: "ci_setup".into(),
            description: "Onboarding: move the project's GitHub Actions into Yulius CI. Returns the workflows in this task's worktree (jobs, run steps, uses steps, matrices), the CI its yulius.toml configures ([[ci.jobs]] or [commands] gate) and the steps: translate every check into [[ci.jobs]], delete .github/workflows/, commit; the pull request is tested with the CI it adds.".into(),
            input_schema: schema(json!({}), &[]),
        },
        ToolDescriptor {
            name: "repo_hygiene".into(),
            description: "Stale branches and worktrees of this project: remote branches whose pull request was merged or closed, branches already merged, local branches no worktree uses, worktrees of deleted, archived or merged tasks. Each item has a reason and whether removing it is safe.".into(),
            input_schema: schema(json!({"refresh": {"type": "boolean", "description": "Scan now instead of returning the last scan"}}), &[]),
        },
        ToolDescriptor {
            name: "repo_cleanup".into(),
            description: "Remove stale branches and worktrees reported by repo_hygiene: the given items, or every safe item when items is omitted. Unmerged branches without a pull request (reason idle) are removed only with force. Live tasks' worktrees, protected branches and branches with an open pull request are never removed; dirty worktrees are kept.".into(),
            input_schema: schema(json!({
                "items": {"type": "array", "items": {"type": "object", "properties": {"kind": {"type": "string", "enum": ["remote_branch", "local_branch", "worktree"]}, "name": {"type": "string"}}, "required": ["kind", "name"]}},
                "force": {"type": "boolean"}
            }), &[]),
        },
        ToolDescriptor {
            name: "delivery_merge".into(),
            description: "Merge this task's pull request on green (the one delivery pipeline: you deliver your own work, no person accepts it). Refused unless the checks passed on the exact head (Yulius CI), the spec is valid when the task needs one, every step is done, the head is current and an independent review in a fresh session approved exactly this head: pass that commit as reviewed_head (and a one-line review_summary). The call first pushes the head and opens the pull request when the task has none, or pushes new commits to it, as the end of a turn would; when the head cannot be delivered yet (an open step, no plan for a task that needs a spec, an invalid spec, uncommitted work) it is refused with why. A review stays valid across clean merges of the integration branch made after it (no conflict resolved, nothing else changed). While the checks at that head still run, the call arms the merge instead: Yulius merges exactly that head as soon as they pass (start `yulius wait` with run_in_background and end your turn: it says how the merge ended; never poll). Acts with the authority of the task's owner and is audited.".into(),
            input_schema: schema(json!({
                "reviewed_head": {"type": "string", "description": "The commit SHA the independent reviewer approved: the branch head, or an earlier head followed only by clean merges of the integration branch"},
                "review_summary": {"type": "string", "description": "One line: the reviewer's verdict and what it checked"}
            }), &["reviewed_head"]),
        },
        ToolDescriptor {
            name: "delivery_deploy".into(),
            description: "Run the next integration environment of the delivery profile's [[delivery.environments]] (projects without CD tables): its deploy command runs and its verification decides. Only environments with a command and verify = \"command\" after the integration stage; a production environment or one a person must verify is refused. Follow the run with delivery_status. Projects with [cd.dev] deploy with cd_deploy.".into(),
            input_schema: schema(json!({"environment": {"type": "string", "description": "Environment name from the delivery profile"}}), &["environment"]),
        },
        ToolDescriptor {
            name: "cd_status".into(),
            description: "This project's CD (deployment, SPEC-134): dev and production as yulius.toml defines them on their protected branches ([cd.dev] from the integration branch, [cd.prod] from what production deploys), each one's state (not defined, empty or ready), its deploy and verify commands, the secrets it needs and whether a person allowed them, what is deployed, and the recent runs. With run_id, one run with the end of its output. Improve CD by changing yulius.toml and scripts/cd/ in a pull request; a task branch never changes what a deploy runs.".into(),
            input_schema: schema(json!({
                "run_id": {"type": "string", "description": "A run id from the list, to read its output"}
            }), &[]),
        },
        ToolDescriptor {
            name: "cd_deploy".into(),
            description: "Deploy dev: Yulius runs [cd.dev]'s deploy then verify commands from the integration branch head, inside the sandbox, with the secrets it names (a person must allow each outside the proxy; one not allowed yet is asked for and nothing runs). Only when your task asks for a dev deploy (merges do not deploy). Production is never yours: only a person deploys it, on the CD page; ask one with needs_you. Start `yulius wait` in the background to wait for the run's end.".into(),
            input_schema: schema(json!({"environment": {"type": "string", "enum": ["dev"], "description": "dev"}}), &["environment"]),
        },
        ToolDescriptor {
            name: "needs_you".into(),
            description: "Ask a person to do something only a person can (add a credential to the vault on the Secrets page, allow a secret, deploy production, decide on a paid plan): the task shows \"Needs you: <action>\" and people are notified, until a person writes to the task or you call it again with resolved: true (for example once cd_status shows the secret). Say exactly what to do and where; go on with other work meanwhile. Not for questions you can decide yourself.".into(),
            input_schema: schema(json!({
                "action": {"type": "string", "description": "What the person should do, imperative, one line (at most 200 characters), e.g. 'Add DEV_SSH_KEY on the Secrets page'"},
                "why": {"type": "string", "description": "Why it is needed (at most 500 characters)"},
                "link": {"type": "string", "description": "Where to do it: a Yulius page URL"},
                "resolved": {"type": "boolean", "description": "true: your request is done (no action needed)"}
            }), &[]),
        },
        ToolDescriptor {
            name: "production_protect".into(),
            description: "Protect this project's production branch from agents (ADR-0114, ADR-0129): Yulius adds a GitHub branch ruleset on it (Restrict updates, Restrict deletions, Block force pushes; only the Repository admin role may bypass) acting as this task's owner, and only in the onboarding task and when GitHub says that person is a repository admin (and their account is enabled). It only adds protection and changes nothing when agents already cannot write the branch. Use it when your onboarding prompt says the production branch is not protected from agents; never change protection from your shell. When it is refused, ask a person with needs_you, quoting the setting it names.".into(),
            input_schema: schema(json!({}), &[]),
        },
        ToolDescriptor {
            name: "issue_create".into(),
            description: "Open a GitHub issue for what lies outside this change, instead of widening this task: a pre-existing bug (in code this change did not introduce), a flaky test this change did not cause, or a cosmetic nit left after 3 review rounds; say why it is outside, with the evidence (e.g. \"present on dev before this change\"). Never a finding in code or tests this change added or modified: fix that in this pull request, test-first. It must meet the Definition of Ready: exactly one `type: …` label (bug, enhancement, docs, chore, performance), one `priority: …` label (high, medium, low), at least one `area: …` label (add `risk: high` for authentication, permissions, data isolation, migrations, money or shared contracts), and a body with a summary, current and expected behavior and a `## Acceptance criteria` checklist. Missing labels are created.".into(),
            input_schema: schema(json!({
                "title": {"type": "string"},
                "body": {"type": "string", "description": "Markdown with a ## Acceptance criteria checklist"},
                "labels": {"type": "array", "items": {"type": "string"}, "maxItems": 10}
            }), &["title", "body", "labels"]),
        },
        ToolDescriptor {
            name: "automation_create".into(),
            description: "Create a scheduled automation in this project: on a five-field cron schedule (minute hour day-of-month month day-of-week, evaluated at utc_offset_minutes from UTC) Yulius creates a new task with the given prompt. Use it when the user asks for recurring work, e.g. \"every day at 9 implement yesterday's customer feedback\". Created tasks start like New task: now (the default), after_task, start_rule, as a supertask, or prepared (waiting until the person presses Start now). The automation appears on the project's Automations page.".into(),
            input_schema: schema(json!({
                "name": {"type": "string", "description": "Short name; also the title of created tasks"},
                "cron": {"type": "string", "description": "Five-field cron expression, e.g. \"0 9 * * *\" (daily at 09:00) or \"0 9 * * MON-FRI\""},
                "utc_offset_minutes": {"type": "integer", "description": "UTC offset of the user's time zone in minutes (e.g. 120 for UTC+2); default 0"},
                "prompt": {"type": "string", "description": "What each created task should do"},
                "harness": {"type": "string", "enum": ["claude"], "description": "Agent for created tasks (Claude Code; the default)"},
                "workflow": {"type": "string", "description": "Workflow id or name from workflows_list (optional)"},
                "enabled": {"type": "boolean", "description": "Default true"},
                "after_task": {"type": "integer", "description": "Created tasks wait until this task number is done (once it has ended: the rule \"#N done\")"},
                "start_rule": {"type": "string", "description": "Created tasks start when this rule over the project's tasks is true, e.g. \"#12 done and #13 merged\" (not with after_task)"},
                "supertask": {"type": "boolean", "description": "Created tasks run alone in the project as supertasks (not with after_task or start_rule); only when the person asks"},
                "prepared": {"type": "boolean", "description": "\"When I decide\": each created task waits, prepared, until the person presses Start now (not with after_task, start_rule or supertask); a run is skipped while the previous one still waits"}
            }), &["name", "cron", "prompt"]),
        },
        ToolDescriptor {
            name: "automations_list".into(),
            description: "List this project's automations with schedule, next run and last run, and under ready_made the ready-made automations Yulius ships (id, what it does, schedule; automation_id is the project's automation of the same name, or null when not added). Enable one with automation_template_enable.".into(),
            input_schema: schema(json!({}), &[]),
        },
        ToolDescriptor {
            name: "automation_template_enable".into(),
            description: "Enable a ready-made automation by its id from automations_list's ready_made (e.g. docs-up-to-date), as Enable on the Automations page: it is added enabled with its catalog name, schedule, workflow and prompt, or, when the project already has an automation of that name, that one is enabled unchanged. Never creates a duplicate.".into(),
            input_schema: schema(json!({
                "template": {"type": "string", "description": "Ready-made automation id"}
            }), &["template"]),
        },
        ToolDescriptor {
            name: "automation_run_now".into(),
            description: "Run an automation of this project once now, as Run now on the Automations page (also when disabled; skipped while its previous task is still active). Returns the run and the task it created.".into(),
            input_schema: schema(json!({"automation_id": {"type": "string"}}), &["automation_id"]),
        },
        ToolDescriptor {
            name: "automation_delete".into(),
            description: "Delete an automation of this project and its run history, as Delete on the Automations page; the tasks it created stay. Audited.".into(),
            input_schema: schema(json!({"automation_id": {"type": "string"}}), &["automation_id"]),
        },
        ToolDescriptor {
            name: "automation_update".into(),
            description: "Change an automation of this project: enable/disable it, or change its cron schedule, UTC offset, prompt, name or how its tasks start (giving any of after_task, start_rule, supertask or prepared replaces the start choice; supertask false alone: start now).".into(),
            input_schema: schema(json!({
                "automation_id": {"type": "string"},
                "enabled": {"type": "boolean"},
                "cron": {"type": "string"},
                "utc_offset_minutes": {"type": "integer"},
                "prompt": {"type": "string"},
                "name": {"type": "string"},
                "after_task": {"type": "integer", "description": "Created tasks wait until this task number is done"},
                "start_rule": {"type": "string", "description": "Created tasks start when this rule is true, e.g. \"#12 done\""},
                "supertask": {"type": "boolean", "description": "Created tasks run alone in the project"},
                "prepared": {"type": "boolean", "description": "Created tasks wait, prepared, until the person presses Start now (\"When I decide\")"}
            }), &["automation_id"]),
        },
        ToolDescriptor {
            name: "workflows_list".into(),
            description: "List the workflows (reusable task recipes with initial steps, instructions and gates) available in this project: Yulius's defaults first (builtin, with when_to_use), then the project's own.".into(),
            input_schema: schema(json!({}), &[]),
        },
    ];
    tools.extend(crate::browser::tool_descriptors());
    tools.extend(secret_tool_descriptors());
    tools
}

/// Agent-secret tools (SPEC-002 ADR-20): names only, and typing into the sandbox browser.
fn secret_tool_descriptors() -> Vec<ToolDescriptor> {
    vec![
        ToolDescriptor {
            name: "secrets_list".into(),
            description: "List the agent secrets this project may use: names, descriptions, the hosts each is bound to (and requested_hosts waiting for a person to allow, see secret_request_hosts) and how it is added to requests. You never receive values: Yulius' proxy adds a secret to HTTP requests for its bound hosts (call them normally; in code use the environment variable NAME, which holds a placeholder), browser_fill_secret types it into a sign-in page, and any value that would appear in output shows as [secret:NAME].".into(),
            input_schema: schema(json!({}), &[]),
        },
        ToolDescriptor {
            name: "secret_request_hosts".into(),
            description: "Ask for the hosts an agent secret should be sent to or typed into (SPEC-121). A person adds a secret with only a name, a value and a description; you find the service's API or sign-in host from the description and the service's documentation (e.g. api.vidoc.com, *.vidoc.com for subdomains) and request it here. Nothing is bound until a person who manages the secret allows it on the Secrets page: print the returned link for them, go on with other work, and use the secret once secrets_list shows the host bound.".into(),
            input_schema: schema(json!({
                "secret": {"type": "string", "description": "Secret NAME from secrets_list"},
                "hosts": {"type": "array", "items": {"type": "string"}, "description": "Host names like api.example.com, *.example.com or api.example.com:8443"},
                "reason": {"type": "string", "description": "What you need it for (short), recorded in the secrets activity"}
            }), &["secret", "hosts"]),
        },
        ToolDescriptor {
            name: "secret_copy_to_env".into(),
            description: "Copy agent secrets' real values into a git-ignored dotenv file of your worktree (default .env) and their names into the .env.example beside it (SPEC-129), for code that runs outside Yulius' proxy (a deployed app, a deploy script). You never see the values, so never write or edit those lines yourself: you would write [secret:NAME]. A person allows each secret once: until then nothing is written and the result gives the Secrets page link to print. Refused unless Git ignores the file and does not track it.".into(),
            input_schema: schema(json!({
                "secrets": {"type": "array", "items": {"type": "string"}, "description": "Secret NAMEs from secrets_list (1-16)"},
                "path": {"type": "string", "description": "Dotenv file relative to the worktree, e.g. .env (default) or deploy/.env.production"},
                "reason": {"type": "string", "description": "What needs the real value (short), recorded in the secrets activity"}
            }), &["secrets"]),
        },
        ToolDescriptor {
            name: "browser_fill_secret".into(),
            description: "Type an agent secret into the project browser without seeing it: its value (password or key) or, when the secret stores them (Basic credentials, older secrets), its user name or current two-factor (TOTP) code, into the focused field or the element matching selector, in a tab whose page is on one of the secret's bound hosts. Use browser_snapshot first to find the field.".into(),
            input_schema: schema(json!({
                "tab_id": {"type": "string", "description": "Tab from browser_tabs or browser_open"},
                "secret": {"type": "string", "description": "Secret NAME from secrets_list"},
                "field": {"type": "string", "enum": ["value", "password", "username", "totp"], "description": "What to type (default value; password is the same as value)"},
                "selector": {"type": "string", "description": "CSS selector of the field to focus first"}
            }), &["tab_id", "secret"]),
        },
    ]
}

impl App {
    /// Handles one JSON-RPC message from an agent; `None` for notifications.
    pub async fn mcp_handle(
        self: &Arc<Self>,
        token: &str,
        request: RpcRequest,
    ) -> Option<RpcResponse> {
        let id = request.id.clone()?;
        let caller = if token.starts_with(COCKPIT_TOKEN_PREFIX) {
            self.cockpit_caller(token).await.map(McpCaller::Cockpit)
        } else {
            self.agent_context(token).await.map(McpCaller::Agent)
        };
        let caller = match caller {
            Ok(caller) => caller,
            Err(_) => {
                return Some(RpcResponse::err(
                    id,
                    -32001,
                    "invalid or expired task token",
                ));
            }
        };
        let result: AppResult<Value> = match request.method.as_str() {
            "initialize" => {
                let requested = request
                    .params
                    .get("protocolVersion")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                let version = SUPPORTED_PROTOCOL_VERSIONS
                    .iter()
                    .find(|v| **v == requested)
                    .copied()
                    .unwrap_or(SUPPORTED_PROTOCOL_VERSIONS[0]);
                Ok(json!({
                    "protocolVersion": version,
                    "capabilities": {"tools": {"listChanged": false}},
                    "serverInfo": {"name": "yulius", "version": env!("CARGO_PKG_VERSION")},
                    "instructions": match caller {
                        McpCaller::Agent(_) => "Yulius coordinates this project's tasks. Use tasks_list and sessions_search before starting, and steps_set/step_update to keep a plan of similar-effort steps whose progress arrives steadily.",
                        McpCaller::Cockpit(_) => "Yulius runs this project's tasks. When the person asks for work to be done as a task (\"create a task from that\"), create it with task_create; for recurring work (\"every day at 4am …\") create an automation with automation_create (ask for their time zone if it matters). tasks_list, automations_list (with the ready-made automations, enabled by automation_template_enable) and workflows_list show what exists; sessions_search finds how earlier tasks did something. secrets_list names the project's agent secrets (never their values) and secret_verify tells whether a value the person holds is one of them, from its SHA-256. task_stop, task_archive and task_delete stop, archive or delete a task, automation_run_now and automation_delete run or delete an automation: do these only when the person asks, naming the task or automation.",
                    },
                }))
            }
            "ping" => Ok(json!({})),
            "tools/list" => Ok(json!({"tools": match caller {
                McpCaller::Agent(_) => tool_descriptors(),
                McpCaller::Cockpit(_) => cockpit_tool_descriptors(),
            }})),
            "tools/call" => Ok(self.mcp_call(&caller, &request.params).await),
            other => {
                return Some(RpcResponse::err(
                    id,
                    -32601,
                    format!("method not found: {other}"),
                ));
            }
        };
        Some(match result {
            Ok(value) => RpcResponse::ok(id, value),
            Err(err) => RpcResponse::err(id, -32603, err.public_message()),
        })
    }

    async fn mcp_call(self: &Arc<Self>, caller: &McpCaller, params: &Value) -> Value {
        let name = params.get("name").and_then(Value::as_str).unwrap_or("");
        let args = params
            .get("arguments")
            .cloned()
            .unwrap_or_else(|| json!({}));
        // A client that reads records rather than text asks for them (SPEC-086 TOL-005).
        let structured = params
            .get("_meta")
            .and_then(|meta| meta.get(STRUCTURED_META))
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let project = caller.project();
        let outcome: AppResult<Value> = match name {
            "tasks_list" => self.tool_tasks_list(&project, &args).await,
            "task_get" => self.tool_task_get(&project, &args).await,
            "task_create" => self.tool_task_create(&project, &args).await,
            "task_stop" => {
                self.tool_task_change(&project, &args, TaskChange::Stop)
                    .await
            }
            "task_archive" => match args.get("archived") {
                None | Some(Value::Null) => {
                    self.tool_task_change(&project, &args, TaskChange::Archive(true))
                        .await
                }
                Some(Value::Bool(archived)) => {
                    self.tool_task_change(&project, &args, TaskChange::Archive(*archived))
                        .await
                }
                Some(_) => Err(AppError::Invalid("`archived` must be true or false".into())),
            },
            "task_delete" => {
                self.tool_task_change(&project, &args, TaskChange::Delete)
                    .await
            }
            "sessions_search" => self.tool_sessions_search(&project, &args).await,
            "session_read" => self.tool_session_read(&project, &args).await,
            "automation_create" => self.tool_automation_create(&project, &args).await,
            "automations_list" => self.tool_automations_list(&project).await,
            "automation_update" => self.tool_automation_update(&project, &args).await,
            "automation_delete" => self.tool_automation_delete(&project, &args).await,
            "automation_run_now" => self.tool_automation_run_now(&project, &args).await,
            "automation_template_enable" => {
                self.tool_automation_template_enable(&project, &args).await
            }
            "workflows_list" => self.tool_workflows_list(&project).await,
            "secrets_list" => self.tool_secrets_list(&project).await,
            _ => match caller {
                McpCaller::Cockpit(_) if name == "secret_verify" => {
                    self.tool_secret_verify(&project, &args).await
                }
                McpCaller::Cockpit(_) => Err(AppError::Invalid(format!("unknown tool: {name}"))),
                McpCaller::Agent(ctx) => match self.task_tool(ctx, name, &args).await {
                    TaskToolResult::Value(outcome) => outcome,
                    TaskToolResult::Complete(result) => return result,
                },
            },
        };
        self.tool_result(&project.sandbox, name, &args, outcome, structured)
            .await
    }

    /// A tool only a task's agent is served.
    async fn task_tool(
        self: &Arc<Self>,
        ctx: &AgentContext,
        name: &str,
        args: &Value,
    ) -> TaskToolResult {
        let args = args.clone();
        TaskToolResult::Value(match name {
            "project_context" => self.tool_project_context(ctx).await,
            "scope_claim" => self.tool_scope_claim(ctx, &args).await,
            "scope_release" => self.tool_scope_release(ctx, &args).await,
            "scopes_list" => self.tool_scopes_list(ctx).await,
            "record_number" => self.tool_record_number(ctx, &args).await,
            "spec_check" => self.tool_spec_check(ctx).await,
            "steps_set" => self.tool_steps_set(ctx, &args).await,
            "step_update" => self.tool_step_update(ctx, &args).await,
            "steps_get" => self.tool_steps_get(ctx).await,
            "delivery_status" => self.tool_delivery_status(ctx).await,
            "ci_status" => self.tool_ci_status(ctx, &args).await,
            "ci_rerun" => self.tool_ci_rerun(ctx, &args).await,
            "ci_setup" => self.tool_ci_setup(ctx).await,
            "repo_hygiene" => self.tool_repo_hygiene(ctx, &args).await,
            "repo_cleanup" => self.tool_repo_cleanup(ctx, &args).await,
            "delivery_merge" => self.tool_delivery_merge(ctx, &args).await,
            "delivery_deploy" => self.tool_delivery_deploy(ctx, &args).await,
            "cd_status" => self.tool_cd_status(ctx, &args).await,
            "cd_deploy" => self.tool_cd_deploy(ctx, &args).await,
            "needs_you" => self.tool_needs_you(ctx, &args).await,
            "production_protect" => self.tool_production_protect(ctx).await,
            "issue_create" => self.tool_issue_create(ctx, &args).await,
            "browser_tabs" => self.tool_browser_tabs(ctx, &args).await,
            "browser_open" => self.tool_browser_open(ctx, &args).await,
            "browser_navigate" => self.tool_browser_navigate(ctx, &args).await,
            "browser_click" => self.tool_browser_click(ctx, &args).await,
            "browser_type" => self.tool_browser_type(ctx, &args).await,
            "browser_snapshot" => self.tool_browser_snapshot(ctx, &args).await,
            // Returns a complete result with an image item (size-bounded by the tool itself).
            "browser_screenshot" => {
                return TaskToolResult::Complete(self.tool_browser_screenshot(ctx, &args).await);
            }
            "browser_request_user_action" => {
                self.tool_browser_request_user_action(ctx, &args).await
            }
            "browser_fill_secret" => self.tool_browser_fill_secret(ctx, &args).await,
            "secret_request_hosts" => self.tool_secret_request_hosts(ctx, &args).await,
            "secret_copy_to_env" => self.tool_secret_copy_to_env(ctx, &args).await,
            other => Err(AppError::Invalid(format!("unknown tool: {other}"))),
        })
    }

    /// The `tools/call` result of a tool's outcome: short text rendered from the tool's record
    /// (SPEC-086), redacted and bounded; with `structured`, the record too.
    async fn tool_result(
        &self,
        sandbox: &SandboxId,
        name: &str,
        args: &Value,
        outcome: AppResult<Value>,
        structured: bool,
    ) -> Value {
        match outcome {
            Ok(value) => {
                // No tool result carries an agent secret's value (SPEC-002 ADR-20): page text,
                // messages, task prompts and everything else are redacted before the agent. Each
                // text of the record is redacted before rendering, which indents multi-line text
                // and could split what the matcher looks for, and the rendered text again.
                let Some(value) = self.redact_strings(sandbox, value).await else {
                    return text_result(WITHHELD, false);
                };
                let text = crate::mcp_text::render(name, args, &value);
                let mut text = self.redact_for_sandbox(sandbox, &text).await;
                if text.len() > MAX_RESULT_BYTES {
                    let mut cut = MAX_RESULT_BYTES;
                    while !text.is_char_boundary(cut) {
                        cut -= 1;
                    }
                    text.truncate(cut);
                    text.push_str("\n…(truncated)");
                }
                let mut result = text_result(text, false);
                // Never by default: Claude Code gives the model a result's structuredContent
                // instead of its text when both are present (ADR-0069).
                if structured {
                    result["structuredContent"] = structured_record(value);
                }
                result
            }
            Err(err) => text_result(
                self.redact_for_sandbox(sandbox, &err.public_message())
                    .await,
                true,
            ),
        }
    }

    /// The record with every text in it redacted of the sandbox's agent secrets; `None` when
    /// they could not be loaded.
    async fn redact_strings(&self, sandbox: &SandboxId, value: Value) -> Option<Value> {
        match self.agent_secret_matcher(sandbox).await {
            Ok(matcher) => Some(map_strings(value, &|text| {
                redact_text(matcher.as_ref(), text).0
            })),
            Err(err) => {
                tracing::error!(error = %err, "agent secrets could not be loaded for redaction");
                None
            }
        }
    }
}

/// What a result says when its record could not be checked for agent secrets.
const WITHHELD: &str = "[withheld: Yulius could not check this text for agent secrets]";

/// A (redacted) record as `structuredContent`: an object, so another value is wrapped as
/// `{"value": …}`, and one larger than [`MAX_RESULT_BYTES`] is withheld (results stay bounded).
fn structured_record(value: Value) -> Value {
    let record = match value {
        Value::Object(_) => value,
        other => json!({ "value": other }),
    };
    if serde_json::to_vec(&record).map_or(usize::MAX, |bytes| bytes.len()) > MAX_RESULT_BYTES {
        return json!({ "withheld": "the record is larger than 256 KiB; its text is the result" });
    }
    record
}

/// The `tools/call` `_meta` key with which a client asks for each result's record as
/// `structuredContent` besides its text (SPEC-086 TOL-005).
pub const STRUCTURED_META: &str = "yulius/structured";

fn map_strings(value: Value, f: &dyn Fn(&str) -> String) -> Value {
    match value {
        Value::String(text) => Value::String(f(&text)),
        Value::Array(items) => Value::Array(items.into_iter().map(|v| map_strings(v, f)).collect()),
        Value::Object(map) => Value::Object(
            map.into_iter()
                .map(|(key, v)| (key, map_strings(v, f)))
                .collect(),
        ),
        other => other,
    }
}

/// What a task-only tool gives back.
enum TaskToolResult {
    /// A value to render as text.
    Value(AppResult<Value>),
    /// A complete result (an image).
    Complete(Value),
}

#[cfg(test)]
mod tests {
    use super::{COCKPIT_TOOLS, cockpit_tool_descriptors, tool_descriptors};
    use yulius_domain::briefing::YULIUS_TOOLS;

    #[test]
    fn a_cockpit_is_served_every_management_tool_and_nothing_else() {
        let served: Vec<String> = cockpit_tool_descriptors()
            .into_iter()
            .map(|t| t.name)
            .collect();
        assert_eq!(served.len(), COCKPIT_TOOLS.len());
        for name in COCKPIT_TOOLS {
            assert!(served.iter().any(|s| s == name), "`{name}` is not served");
        }
    }

    /// SPEC-174 (ADR-0141): `issue_create` is for what lies outside the change, never for a
    /// finding in the change's own code.
    #[test]
    fn issue_create_is_only_for_what_lies_outside_the_change() {
        let tool = tool_descriptors()
            .into_iter()
            .find(|t| t.name == "issue_create");
        let description = tool.map(|t| t.description).unwrap_or_default();
        for stated in [
            "outside this change",
            "pre-existing",
            "flaky test this change did not cause",
            "Never a finding in code or tests this change added or modified",
        ] {
            assert!(
                description.contains(stated),
                "missing {stated:?}: {description}"
            );
        }
        assert!(!description.contains("follow-up work"), "{description}");
    }

    /// SPEC-188 OSP-002: `task_create` names the supertask's examples as the start menu does.
    #[test]
    fn task_create_names_the_supertask_examples_of_the_start_menu() {
        let tool = tool_descriptors()
            .into_iter()
            .find(|t| t.name == "task_create");
        let description = tool.map(|t| t.description).unwrap_or_default();
        assert!(
            description.contains(
                "A supertask (for broad work such as security hardening or one-shotting the product)"
            ),
            "{description}"
        );
    }

    #[test]
    fn every_tool_the_briefing_announces_is_served() {
        let served: Vec<String> = tool_descriptors().into_iter().map(|t| t.name).collect();
        for (name, _) in YULIUS_TOOLS {
            assert!(
                served.iter().any(|s| s == name),
                "the briefing announces `{name}`, which tools/list does not serve"
            );
        }
    }
}
