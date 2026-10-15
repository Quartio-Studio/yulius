//! What a Yulius MCP tool tells the agent (SPEC-086): short text a person can read in Claude
//! Code's terminal, rendered from the tool's structured record.
//!
//! The text says what changed and what is next and carries what the agent acts on: numbers,
//! states, reasons and the ids it passes back. Bulky detail stays behind an explicit request
//! (`steps_get` for the whole plan, `ci_status` with `run_id` and `job` for a job's output). A
//! tool without its own rendering is printed as an outline of its record. A record whose shape
//! a rendering does not recognise is printed as an outline too, so nothing is lost.

use serde_json::{Map, Value};

/// Characters of a job command shown in `ci_status`'s list of jobs.
const COMMAND_CHARS: usize = 120;
/// Bytes of failing tests' output `ci_status` shows across a run's jobs (SPEC-145); a job asked
/// for by name shows all of its own.
const FAILURES_SHOWN: usize = 32 * 1024;

/// The text of a tool's successful result: the tool's own rendering, or the outline of its record.
#[must_use]
pub fn render(tool: &str, args: &Value, value: &Value) -> String {
    let text = match tool {
        "steps_set" => plan(value, true),
        "steps_get" => plan(value, false),
        "step_update" => step_update(value),
        "ci_status" if arg(args, "run_id").is_some() => ci_run(value, arg(args, "job")),
        "ci_status" => ci_status(value),
        "ci_rerun" => run_head(value).map(|head| {
            let mut out = format!("Queued again: {head}\n");
            out.push_str(&jobs_line(value).unwrap_or_default());
            out
        }),
        "delivery_status" => delivery_status(value),
        "tasks_list" => tasks_list(value),
        "task_get" => task_get(value),
        "project_context" => value
            .get("context")
            .and_then(Value::as_str)
            .map(str::to_owned),
        "session_read" => session_read(value),
        // The number, the file to create and what to do with it (SPEC-083).
        "record_number" => text(value, "next").map(str::to_owned),
        // CD (SPEC-134): the tool writes its own few lines.
        "cd_status" | "cd_deploy" => text(value, "text").map(str::to_owned),
        _ => None,
    };
    finish(text.unwrap_or_else(|| outline(value)))
}

fn finish(mut text: String) -> String {
    text.truncate(text.trim_end().len());
    text
}

fn arg<'a>(args: &'a Value, key: &str) -> Option<&'a str> {
    args.get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
}

fn text<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    value
        .get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty())
}

fn push_line(out: &mut String, line: &str) {
    out.push_str(line);
    out.push('\n');
}

// ---------------------------------------------------------------- outline

/// Any record as an indented outline: `key: value` lines, list items as `- `, multi-line text on
/// its own lines. Null, empty text and empty lists are left out.
#[must_use]
pub fn outline(value: &Value) -> String {
    let mut out = String::new();
    match value {
        Value::Object(map) => object(&mut out, map, 0),
        Value::Array(items) => list(&mut out, items, 0),
        other => out.push_str(&scalar(other)),
    }
    finish(out)
}

fn is_blank(value: &Value) -> bool {
    match value {
        Value::Null => true,
        Value::String(s) => s.trim().is_empty(),
        Value::Array(items) => items.iter().all(is_blank),
        Value::Object(map) => map.values().all(is_blank),
        Value::Bool(_) | Value::Number(_) => false,
    }
}

fn scalar(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

fn is_inline(value: &Value) -> bool {
    match value {
        Value::Bool(_) | Value::Number(_) => true,
        Value::String(s) => !s.contains('\n'),
        _ => false,
    }
}

fn object(out: &mut String, map: &Map<String, Value>, indent: usize) {
    for (key, value) in map.iter().filter(|(_, v)| !is_blank(v)) {
        field(out, key, value, indent);
    }
}

fn field(out: &mut String, key: &str, value: &Value, indent: usize) {
    let pad = " ".repeat(indent);
    match value {
        Value::Object(map) => {
            push_line(out, &format!("{pad}{key}:"));
            object(out, map, indent + 2);
        }
        Value::Array(items) => {
            let shown: Vec<&Value> = items.iter().filter(|v| !is_blank(v)).collect();
            let joined = shown
                .iter()
                .map(|v| scalar(v))
                .collect::<Vec<_>>()
                .join(", ");
            if shown.iter().all(|v| is_inline(v)) && joined.len() <= 100 {
                push_line(out, &format!("{pad}{key}: {joined}"));
            } else {
                push_line(out, &format!("{pad}{key}:"));
                list(out, items, indent + 2);
            }
        }
        Value::String(s) if s.contains('\n') => {
            push_line(out, &format!("{pad}{key}:"));
            for line in s.lines() {
                push_line(out, &format!("{pad}  {line}"));
            }
        }
        other => push_line(out, &format!("{pad}{key}: {}", scalar(other))),
    }
}

fn list(out: &mut String, items: &[Value], indent: usize) {
    let pad = " ".repeat(indent);
    for item in items.iter().filter(|v| !is_blank(v)) {
        let mut block = String::new();
        match item {
            Value::Object(map) => object(&mut block, map, indent + 2),
            Value::Array(inner) => list(&mut block, inner, indent + 2),
            Value::String(s) => {
                for line in s.lines() {
                    push_line(&mut block, &format!("{pad}  {line}"));
                }
            }
            other => push_line(&mut block, &format!("{pad}  {}", scalar(other))),
        }
        // The item's first line carries the dash.
        let body = block.get(indent + 2..).unwrap_or_default();
        out.push_str(&pad);
        out.push_str("- ");
        out.push_str(body);
    }
}

// ---------------------------------------------------------------- steps

fn status_mark(status: &str) -> &'static str {
    match status {
        "done" => "[x]",
        "in_progress" => "[>]",
        "skipped" => "[-]",
        "blocked" => "[!]",
        _ => "[ ]",
    }
}

fn status_words(status: &str) -> &str {
    match status {
        "in_progress" => "in progress",
        other => other,
    }
}

/// One step as a checklist line: `3 [>] Title (in progress, effort 2)` and its note.
fn step_line(out: &mut String, ordinal: u64, step: &Value) {
    let status = text(step, "status").unwrap_or("pending");
    let mut extra = Vec::new();
    if matches!(status, "in_progress" | "skipped" | "blocked") {
        extra.push(status_words(status).to_owned());
    }
    if let Some(effort) = step
        .get("effort")
        .and_then(Value::as_u64)
        .filter(|e| *e > 1)
    {
        extra.push(format!("effort {effort}"));
    }
    let extra = if extra.is_empty() {
        String::new()
    } else {
        format!(" ({})", extra.join(", "))
    };
    let title = text(step, "title").unwrap_or("(untitled)");
    push_line(
        out,
        &format!("{ordinal} {} {title}{extra}", status_mark(status)),
    );
    if let Some(note) = text(step, "note") {
        push_line(out, &format!("    note: {}", one_line(note)));
    }
}

fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn ordinal_of(step: &Value, index: usize) -> u64 {
    step.get("ordinal")
        .and_then(Value::as_u64)
        .unwrap_or(index as u64 + 1)
}

fn progress_line(value: &Value) -> Option<String> {
    let progress = value.get("progress")?;
    let done = progress.get("done")?.as_u64()?;
    let total = progress.get("total")?.as_u64()?;
    if total == 0 {
        return Some("No steps yet: set the plan with steps_set.".to_owned());
    }
    let percent = progress.get("percent").and_then(Value::as_u64).unwrap_or(0);
    Some(format!(
        "Progress: {done}/{total} steps done, {percent}% by effort."
    ))
}

fn nudges(out: &mut String, value: &Value) {
    let items: Vec<&str> = value
        .get("nudges")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .filter(|s| !s.trim().is_empty())
                .collect()
        })
        .unwrap_or_default();
    match items.as_slice() {
        [] => {}
        [one] => push_line(out, &format!("Nudge: {one}")),
        many => {
            push_line(out, "Nudges:");
            for nudge in many {
                push_line(out, &format!("- {nudge}"));
            }
        }
    }
}

/// `steps_get` / `steps_set`: the plan as a checklist, the progress and the nudges. Step ids and
/// times are left out: steps are named by ordinal or title.
fn plan(value: &Value, set: bool) -> Option<String> {
    let steps = value.get("steps")?.as_array()?;
    let mut out = String::new();
    if set {
        push_line(
            &mut out,
            &format!(
                "Plan set: {} step{}.",
                steps.len(),
                if steps.len() == 1 { "" } else { "s" }
            ),
        );
    }
    push_line(&mut out, &progress_line(value)?);
    for (index, step) in steps.iter().enumerate() {
        step_line(&mut out, ordinal_of(step, index), step);
    }
    nudges(&mut out, value);
    Some(out)
}

/// `step_update`: the step that changed, the progress, what is next and the nudges.
fn step_update(value: &Value) -> Option<String> {
    let updated = value.get("updated")?;
    let steps = value.get("steps")?.as_array()?;
    let ordinal = updated.get("ordinal")?.as_u64()?;
    let status = text(updated, "status")?;
    let mut out = format!(
        "Step {ordinal} \"{}\" is {}.\n",
        text(updated, "title").unwrap_or("(untitled)"),
        status_words(status)
    );
    push_line(&mut out, &progress_line(value)?);
    let open = |wanted: &[&str]| {
        steps.iter().enumerate().find(|(index, step)| {
            ordinal_of(step, *index) != ordinal
                && wanted.contains(&text(step, "status").unwrap_or("pending"))
        })
    };
    let next = open(&["pending", "blocked"]).or_else(|| open(&["in_progress"]));
    match next {
        Some((index, step)) => push_line(
            &mut out,
            &format!(
                "Next: {} \"{}\" ({}).",
                ordinal_of(step, index),
                text(step, "title").unwrap_or("(untitled)"),
                status_words(text(step, "status").unwrap_or("pending"))
            ),
        ),
        None if matches!(status, "done" | "skipped") => {
            push_line(&mut out, "Every step is finished.");
        }
        None => {}
    }
    nudges(&mut out, value);
    Some(out)
}

// ---------------------------------------------------------------- CI

fn duration(ms: i64) -> String {
    let secs = ms.max(0) / 1000;
    match (secs / 3600, secs / 60 % 60, secs % 60) {
        (0, 0, s) => format!("{s}s"),
        (0, m, s) => format!("{m}m {s}s"),
        (h, m, _) => format!("{h}h {m}m"),
    }
}

fn short_sha(sha: &str) -> &str {
    sha.get(..7).unwrap_or(sha)
}

fn clip(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_owned();
    }
    let mut out: String = text.chars().take(max).collect();
    out.push('…');
    out
}

/// `<id> <label> <sha>: <state> in <duration> — <reason>`.
fn run_head(run: &Value) -> Option<String> {
    let id = text(run, "id")?;
    let state = text(run, "state")?;
    let mut head = format!(
        "{id} {} {}: {state}",
        text(run, "label").unwrap_or("?"),
        short_sha(text(run, "sha").unwrap_or("?"))
    );
    if let Some(ms) = run
        .get("duration_ms")
        .and_then(Value::as_i64)
        .filter(|ms| *ms > 0)
    {
        head.push_str(&format!(" in {}", duration(ms)));
    }
    if let Some(reason) = text(run, "reason") {
        head.push_str(&format!(" — {}", one_line(reason)));
    }
    if let Some(status) = text(run, "status").filter(|s| *s != "published") {
        head.push_str(&format!(" (commit status: {status})"));
    }
    Some(head)
}

fn job_name(job: &Value) -> String {
    let name = text(job, "name").unwrap_or("?");
    match text(job, "matrix") {
        Some(matrix) => format!("{name} [{matrix}]"),
        None => name.to_owned(),
    }
}

/// One job: `quality failed (exit 1) in 6m 30s: reason`; the run's own reason is not repeated.
fn job_state(job: &Value, run_reason: Option<&str>) -> String {
    let state = text(job, "state").unwrap_or("?");
    let mut out = format!("{} {state}", job_name(job));
    if let Some(code) = job
        .get("exit_code")
        .and_then(Value::as_i64)
        .filter(|c| *c != 0)
    {
        out.push_str(&format!(" (exit {code})"));
    }
    if let Some(ms) = job
        .get("duration_ms")
        .and_then(Value::as_i64)
        .filter(|ms| *ms > 0)
    {
        out.push_str(&format!(" in {}", duration(ms)));
    }
    if let Some(reason) = text(job, "reason").filter(|r| Some(*r) != run_reason) {
        out.push_str(&format!(": {}", one_line(reason)));
    }
    out
}

/// The run's jobs on one line; nothing when they all ended like the run.
fn jobs_line(run: &Value) -> Option<String> {
    let jobs = run.get("jobs")?.as_array()?;
    if jobs.is_empty() {
        return None;
    }
    let state = text(run, "state");
    let reason = text(run, "reason");
    let plain = jobs.iter().all(|job| {
        text(job, "state") == state
            && job.get("exit_code").and_then(Value::as_i64).unwrap_or(0) == 0
            && text(job, "reason").is_none_or(|r| Some(r) == reason)
    });
    if plain {
        return None;
    }
    let parts: Vec<String> = jobs.iter().map(|job| job_state(job, reason)).collect();
    Some(format!("  {}\n", parts.join(" · ")))
}

/// `ci_status` without `run_id`: the jobs in force, the task's pull request and the recent runs,
/// a line each (and its jobs when they did not all end like the run), every command printed once.
fn ci_status(value: &Value) -> Option<String> {
    let runs = value.get("runs")?.as_array()?;
    let mut out = String::new();
    match value.get("ci").filter(|ci| !ci.is_null()) {
        Some(ci) => {
            let jobs = ci.get("jobs").and_then(Value::as_array);
            let source = text(ci, "source").unwrap_or("?");
            push_line(
                &mut out,
                &format!(
                    "CI ({source}): {} job{}",
                    jobs.map_or(0, Vec::len),
                    if jobs.map_or(0, Vec::len) == 1 {
                        ""
                    } else {
                        "s"
                    }
                ),
            );
            for job in jobs.into_iter().flatten() {
                push_line(
                    &mut out,
                    &format!(
                        "- {}: {}",
                        job_name(job),
                        clip(text(job, "command").unwrap_or(""), COMMAND_CHARS)
                    ),
                );
            }
        }
        None => push_line(&mut out, "No CI is configured."),
    }
    if let Some(problem) = text(value, "problem") {
        push_line(&mut out, &format!("Problem: {problem}"));
    }
    if let Some(workflows) = value
        .get("github_actions_workflows")
        .filter(|w| !is_blank(w))
    {
        push_line(
            &mut out,
            "GitHub Actions workflows still in the repository:",
        );
        push_line(&mut out, &outline(workflows));
    }
    let playground = value
        .get("playground")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    match value.get("pull_request").and_then(Value::as_u64) {
        Some(number) => push_line(&mut out, &format!("Your pull request: #{number}.")),
        // SPEC-163: a playground's task is tested as its head merged into the integration branch.
        None if playground => push_line(
            &mut out,
            "This is a playground: `delivery_merge` tests your head merged into the integration \
             branch (the Task runs) before it squashes.",
        ),
        None => push_line(&mut out, "Your task has no pull request yet."),
    }
    if runs.is_empty() {
        push_line(&mut out, "No runs yet.");
    } else {
        push_line(&mut out, "Recent runs (yours first):");
    }
    for run in runs {
        let mine = run
            .get("this_task")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        push_line(
            &mut out,
            &format!("- {}{}", run_head(run)?, if mine { " (yours)" } else { "" }),
        );
        out.push_str(&jobs_line(run).unwrap_or_default());
    }
    if let Some(note) = text(value, "note") {
        push_line(&mut out, note);
    }
    Some(out)
}

/// `ci_status` with `run_id`: the run, each job's state and, for the jobs that failed or errored
/// (or the job asked for by `job` whatever its state), its failing tests' output kept from all of
/// its output, then the end of its output.
fn ci_run(value: &Value, wanted: Option<&str>) -> Option<String> {
    let jobs = value.get("jobs")?.as_array()?;
    let reason = text(value, "reason");
    let mut out = format!("Run {}\n", run_head(value)?);
    let named = |job: &Value| {
        wanted.is_some_and(|w| {
            text(job, "name") == Some(w) || job_name(job) == w || text(job, "id") == Some(w)
        })
    };
    if let Some(wanted) = wanted
        && !jobs.iter().any(&named)
    {
        let names: Vec<String> = jobs.iter().map(job_name).collect();
        push_line(
            &mut out,
            &format!(
                "This run has no job named \"{wanted}\"; its jobs: {}.",
                names.join(", ")
            ),
        );
    }
    let mut hidden = false;
    let mut failures_left = FAILURES_SHOWN;
    for job in jobs {
        push_line(&mut out, &format!("- {}", job_state(job, reason)));
        let failed = matches!(text(job, "state"), Some("failed" | "error"));
        let show = if wanted.is_some() { named(job) } else { failed };
        match text(job, "output") {
            Some(output) if show => {
                push_line(
                    &mut out,
                    &format!("  $ {}", text(job, "command").unwrap_or("")),
                );
                // Its failing tests, wherever they were in its output (SPEC-145).
                if let Some(failures) = text(job, "failures").filter(|f| !f.trim().is_empty()) {
                    if wanted.is_some() || failures.len() <= failures_left {
                        failures_left = failures_left.saturating_sub(failures.len());
                        push_line(
                            &mut out,
                            &format!(
                                "  Failing tests of {} (kept from all of its output):",
                                job_name(job)
                            ),
                        );
                        push_line(&mut out, failures.trim_end());
                    } else {
                        push_line(
                            &mut out,
                            &format!(
                                "  Failing tests of {}: not shown here; ci_status with this run_id \
                                 and job set to its name.",
                                job_name(job)
                            ),
                        );
                    }
                }
                push_line(
                    &mut out,
                    &format!("  Output of {} (its end):", job_name(job)),
                );
                push_line(&mut out, output.trim_end());
            }
            Some(_) => hidden = true,
            None => {}
        }
    }
    if hidden {
        push_line(
            &mut out,
            "Another job's output: ci_status with this run_id and job set to its name.",
        );
    }
    Some(out)
}

// ---------------------------------------------------------------- delivery

/// `delivery_status`: the stage, the pull request, the spec, what blocks and what is next. The
/// delivery rules themselves are in the session briefing and the tools' descriptions.
fn delivery_status(value: &Value) -> Option<String> {
    let stage = text(value, "stage")?;
    let mut out = match text(value, "environment") {
        Some(env) => format!("Stage: {stage} ({env}).\n"),
        None => format!("Stage: {stage}.\n"),
    };
    match value.get("pull_request").filter(|pr| !pr.is_null()) {
        Some(pr) => {
            push_line(
                &mut out,
                &format!(
                    "Pull request #{} {} into {}, checks {}: {}",
                    pr.get("number").and_then(Value::as_u64)?,
                    text(pr, "state").unwrap_or("?"),
                    text(pr, "base").unwrap_or("?"),
                    text(pr, "checks").unwrap_or("?"),
                    text(pr, "title").unwrap_or("")
                ),
            );
            if let Some(url) = text(pr, "url") {
                push_line(&mut out, &format!("  {url}"));
            }
        }
        None => push_line(&mut out, "No pull request yet."),
    }
    let problems: Vec<&str> = value
        .get("spec_problems")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    let valid = value
        .get("spec_valid")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    match (text(value, "spec_path"), valid) {
        (Some(path), true) => push_line(&mut out, &format!("Spec {path}: valid.")),
        (Some(path), false) => push_line(&mut out, &format!("Spec {path}: not valid.")),
        (None, true) => {}
        (None, false) => push_line(&mut out, "Spec: missing."),
    }
    for problem in problems {
        push_line(&mut out, &format!("- {problem}"));
    }
    if let Some(open) = value
        .get("steps_open")
        .and_then(Value::as_u64)
        .filter(|n| *n > 0)
    {
        push_line(&mut out, &format!("Open steps: {open}."));
    }
    let blocked = value.get("blocked").and_then(Value::as_array);
    if let Some(blocked) = blocked.filter(|b| !b.is_empty()) {
        push_line(&mut out, "Refused now:");
        for item in blocked {
            push_line(
                &mut out,
                &format!(
                    "- {}: {}",
                    text(item, "action").unwrap_or("?"),
                    text(item, "reason").unwrap_or("")
                ),
            );
        }
    }
    let lines = |key: &str| -> Vec<String> {
        value
            .get(key)
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default()
    };
    for note in lines("notes") {
        push_line(&mut out, &format!("Note: {note}"));
    }
    let people = lines("next_actions_for_people");
    if !people.is_empty() {
        push_line(
            &mut out,
            &format!("Next for people: {}.", people.join(", ")),
        );
    }
    if let Some(access) = text(value, "checks_access") {
        push_line(&mut out, &format!("Checks access: {access}"));
    }
    if let Some(profile) = value.get("profile").filter(|p| !p.is_null()) {
        out.push_str(&profile_line(profile));
    }
    Some(out)
}

fn profile_line(profile: &Value) -> String {
    let flag = |key: &str| profile.get(key).and_then(Value::as_bool);
    let mut out = format!(
        "Profile {}: {} merge into {}, {} promotion to {}",
        text(profile, "preset").unwrap_or("?"),
        text(profile, "merge").unwrap_or("?"),
        text(profile, "integration_branch").unwrap_or("?"),
        text(profile, "promote").unwrap_or("?"),
        text(profile, "production_branch").unwrap_or("?"),
    );
    if flag("auto_promote") == Some(true) {
        out.push_str(" (automatic)");
    }
    match flag("require_checks") {
        Some(true) => out.push_str("; checks required"),
        Some(false) => out.push_str("; checks not required"),
        None => {}
    }
    out.push_str(".\n");
    for env in profile
        .get("environments")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let mut line = format!(
            "- environment {} after {}, verified by {}",
            text(env, "name").unwrap_or("?"),
            text(env, "after").unwrap_or("?"),
            text(env, "verify").unwrap_or("?")
        );
        if let Some(command) = text(env, "command") {
            line.push_str(&format!(": {}", clip(command, COMMAND_CHARS)));
        }
        push_line(&mut out, &line);
    }
    out
}

// ---------------------------------------------------------------- tasks

/// `#63 Title · running (done) · branch b · steps 7/7, 100% · after #60 · spec p · created c ·
/// updated u`.
fn task_line(task: &Value) -> Option<String> {
    let number = task.get("number")?.as_u64()?;
    let mut parts = vec![format!("#{number} {}", text(task, "title").unwrap_or(""))];
    let status = text(task, "status").unwrap_or("?");
    parts.push(match text(task, "activity") {
        Some(activity) if activity != status => format!("{status} ({activity})"),
        _ => status.to_owned(),
    });
    if let Some(harness) = text(task, "harness").filter(|h| *h != "claude") {
        parts.push(format!("agent {harness}"));
    }
    if let Some(branch) = text(task, "branch") {
        parts.push(format!("branch {branch}"));
    }
    if let Some(steps) = task.get("steps") {
        let total = steps.get("total").and_then(Value::as_u64).unwrap_or(0);
        if total > 0 {
            let mut s = format!(
                "steps {}/{total}, {}%",
                steps.get("done").and_then(Value::as_u64).unwrap_or(0),
                steps.get("percent").and_then(Value::as_u64).unwrap_or(0)
            );
            if let Some(current) = text(steps, "current") {
                s.push_str(&format!(", now \"{current}\""));
            }
            parts.push(s);
        }
    }
    if let Some(after) = task.get("runs_after").and_then(Value::as_u64) {
        parts.push(format!("after #{after}"));
    }
    if let Some(spec) = text(task, "spec_path") {
        parts.push(format!("spec {spec}"));
    }
    // When it was created matters to an automation that reviews what merged since (docs).
    if let Some(created) = text(task, "created_at") {
        parts.push(format!("created {created}"));
    }
    if let Some(updated) = text(task, "updated_at") {
        parts.push(format!("updated {updated}"));
    }
    Some(parts.join(" · "))
}

/// `tasks_list`: one line per task, the caller's own first. Tasks are named by number in every
/// tool, so their ids are left out.
fn tasks_list(value: &Value) -> Option<String> {
    let tasks = value.get("tasks")?.as_array()?;
    let mut out = String::new();
    if let Some(you) = value.get("you").filter(|v| !v.is_null()) {
        push_line(&mut out, &format!("You: {}", task_line(you)?));
    }
    if tasks.is_empty() {
        push_line(&mut out, "No other tasks match.");
        return Some(out);
    }
    push_line(
        &mut out,
        &format!("Tasks ({}, most recently updated first):", tasks.len()),
    );
    for task in tasks {
        push_line(&mut out, &format!("- {}", task_line(task)?));
    }
    Some(out)
}

/// `task_get`: the task's line, its steps as a checklist and its prompt.
fn task_get(value: &Value) -> Option<String> {
    let mut out = format!("{}\n", task_line(value)?);
    if let Some(steps) = value
        .get("steps_list")
        .and_then(Value::as_array)
        .filter(|s| !s.is_empty())
    {
        push_line(&mut out, "Steps:");
        for (index, step) in steps.iter().enumerate() {
            step_line(&mut out, ordinal_of(step, index), step);
        }
    }
    if let Some(prompt) = text(value, "prompt") {
        push_line(&mut out, "Prompt:");
        push_line(&mut out, prompt.trim_end());
    }
    Some(out)
}

// ---------------------------------------------------------------- sessions

/// `session_read`: which session and range, then the transcript text as it was rendered.
fn session_read(value: &Value) -> Option<String> {
    let body = text(value, "text")?;
    let mut head = format!(
        "Session {} \"{}\"",
        text(value, "id")?,
        text(value, "title").unwrap_or("")
    );
    if let Some(number) = value.get("task_number").and_then(Value::as_u64) {
        head.push_str(&format!(" (task #{number})"));
    } else if let Some(author) = text(value, "author") {
        head.push_str(&format!(" (imported by @{author})"));
    }
    head.push_str(&format!(
        ", from line {}",
        value.get("from").and_then(Value::as_u64).unwrap_or(0)
    ));
    match value.get("next_from").and_then(Value::as_u64) {
        Some(next) => head.push_str(&format!("; continue with from: {next}.")),
        None => head.push_str("; this is the end of the session."),
    }
    Some(format!("{head}\n{}", body.trim_end()))
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};
    use yulius_contracts::api::ci::{CiJob, CiRun};
    use yulius_contracts::api::git::{BlockedAction, Delivery, PullView};

    use super::{outline, render};

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    const UUID: &str = "d528553f-0fa1-4966-b79f-0b7ea7799dfc";

    fn plan_record() -> Value {
        json!({
            "progress": {"done": 1, "total": 3, "percent": 40, "current": "Build the slice"},
            "steps": [
                {"id": UUID, "ordinal": 1, "title": "Write the spec", "status": "done", "effort": 1, "note": null, "started_at": "2026-10-13T11:34:59Z", "finished_at": "2026-10-13T11:36:06Z"},
                {"id": "bd3b4607-3e1c-4012-9034-6e00301802bb", "ordinal": 2, "title": "Build the slice", "status": "in_progress", "effort": 2, "note": null, "started_at": "2026-10-13T11:36:06Z", "finished_at": null},
                {"id": "af54fc7f-9474-4ff9-a831-ef1765d7e11d", "ordinal": 3, "title": "Review", "status": "pending", "effort": 1, "note": "waits for CI", "started_at": null, "finished_at": null}
            ],
            "nudges": ["Mark \"Review\" in_progress when you start it."]
        })
    }

    /// TOL-002: the plan reads as a checklist; ids and times are not printed.
    #[test]
    fn a_plan_reads_as_a_checklist_without_ids_or_times() {
        let text = render("steps_get", &json!({}), &plan_record());
        assert_eq!(
            text,
            "Progress: 1/3 steps done, 40% by effort.\n\
             1 [x] Write the spec\n\
             2 [>] Build the slice (in progress, effort 2)\n\
             3 [ ] Review\n    note: waits for CI\n\
             Nudge: Mark \"Review\" in_progress when you start it."
        );
        let set = render("steps_set", &json!({}), &plan_record());
        assert!(
            set.starts_with("Plan set: 3 steps.\nProgress: 1/3"),
            "{set}"
        );
        for text in [text, set] {
            assert!(
                !text.contains(UUID) && !text.contains("2026-10-13"),
                "{text}"
            );
            assert!(!text.contains('{'), "{text}");
        }
    }

    /// TOL-002: an update says which step changed, the progress and what is next.
    #[test]
    fn a_step_update_says_what_changed_and_what_is_next() {
        let mut record = plan_record();
        record["updated"] = json!({"ordinal": 1, "title": "Write the spec", "status": "done"});
        let text = render(
            "step_update",
            &json!({"ordinal": 1, "status": "done"}),
            &record,
        );
        assert_eq!(
            text,
            "Step 1 \"Write the spec\" is done.\n\
             Progress: 1/3 steps done, 40% by effort.\n\
             Next: 3 \"Review\" (pending).\n\
             Nudge: Mark \"Review\" in_progress when you start it."
        );
        assert!(
            !text.contains("Build the slice (in"),
            "the whole plan is not repeated"
        );

        let mut last = plan_record();
        for step in last["steps"].as_array_mut().into_iter().flatten() {
            step["status"] = json!("done");
        }
        last["nudges"] = json!([]);
        last["updated"] = json!({"ordinal": 3, "title": "Review", "status": "done"});
        let text = render("step_update", &json!({}), &last);
        assert!(text.ends_with("Every step is finished."), "{text}");
    }

    /// TOL-006: a record a rendering does not recognise is printed whole, as an outline.
    #[test]
    fn an_unrecognised_record_falls_back_to_its_outline() {
        let odd = json!({"steps": "not a list", "extra": 1});
        assert_eq!(
            render("steps_get", &json!({}), &odd),
            "extra: 1\nsteps: not a list"
        );
    }

    /// TOL-001: the outline leaves out empty values and prints text on its own lines.
    #[test]
    fn the_outline_is_readable_text() {
        let value = json!({
            "armed": true,
            "merged": false,
            "checks": "pending",
            "nothing": null,
            "empty": [],
            "none": {"a": null, "b": []},
            "labels": ["type: bug", "priority: high"],
            "next": "Yulius merges this head\nwhen the checks pass.",
            "items": [{"kind": "worktree", "name": "/w/1", "safe": true}, {"kind": "local_branch", "name": "feat/x"}]
        });
        assert_eq!(
            outline(&value),
            "armed: true\nchecks: pending\n\
             items:\n  - kind: worktree\n    name: /w/1\n    safe: true\n  - kind: local_branch\n    name: feat/x\n\
             labels: type: bug, priority: high\nmerged: false\n\
             next:\n  Yulius merges this head\n  when the checks pass."
        );
        assert_eq!(outline(&json!("plain")), "plain");
        assert_eq!(outline(&json!(["a", "b\nc"])), "- a\n- b\n  c");
    }

    fn job(
        name: &str,
        state: &str,
        exit: Option<i32>,
        reason: Option<&str>,
        output: &str,
    ) -> CiJob {
        CiJob {
            id: format!("{name}-id"),
            name: name.into(),
            command: format!("scripts/{name} --all"),
            matrix: None,
            state: state.into(),
            reason: reason.map(str::to_owned),
            exit_code: exit,
            duration_ms: Some(390_638),
            output: Some(output.into()),
            failures: None,
        }
    }

    fn run(id: &str, label: &str, state: &str, jobs: Vec<CiJob>) -> CiRun {
        CiRun {
            id: id.into(),
            reference: "pr/370".into(),
            label: label.into(),
            pr_number: Some(370),
            sha: "e793eabd07fd37f7cd6d8aa7fad010e1e4b07cd2".into(),
            state: state.into(),
            source: "yulius_toml".into(),
            jobs,
            reason: None,
            queued_at: "2026-10-13T19:28:19Z".into(),
            started_at: Some("2026-10-13T19:28:20Z".into()),
            ended_at: Some("2026-10-13T19:34:51Z".into()),
            duration_ms: Some(391_397),
            requested_by: "yulius".into(),
            status: "published".into(),
        }
    }

    /// TOL-003: one run with a failed job: the failed job's output, not the passed job's.
    #[test]
    fn a_run_shows_the_output_of_the_jobs_that_failed() -> TestResult {
        let value = serde_json::to_value(run(
            "89154695-bd84-4359-8fdc-8c72fcbb6f38",
            "PR #370",
            "failed",
            vec![
                job(
                    "quality",
                    "failed",
                    Some(1),
                    None,
                    "…test a ... FAILED\nerror: 1 test failed",
                ),
                job("e2e", "passed", Some(0), None, "PASS a\nPASS b"),
            ],
        ))?;
        let args = json!({"run_id": "89154695-bd84-4359-8fdc-8c72fcbb6f38"});
        let text = render("ci_status", &args, &value);
        assert!(text.starts_with("Run 89154695-bd84-4359-8fdc-8c72fcbb6f38 PR #370 e793eab: failed in 6m 31s\n- quality failed (exit 1) in 6m 30s\n  $ scripts/quality --all\n"), "{text}");
        assert!(text.contains("error: 1 test failed"), "{text}");
        assert!(text.contains("- e2e passed in 6m 30s"), "{text}");
        assert!(!text.contains("PASS a"), "{text}");
        assert!(
            text.ends_with(
                "Another job's output: ci_status with this run_id and job set to its name."
            ),
            "{text}"
        );

        // Asked for by name, a passed job's output is shown.
        let args = json!({"run_id": "89154695-bd84-4359-8fdc-8c72fcbb6f38", "job": "e2e"});
        let text = render("ci_status", &args, &value);
        assert!(text.contains("PASS a\nPASS b"), "{text}");
        assert!(!text.contains("1 test failed"), "{text}");
        let args = json!({"run_id": "89154695-bd84-4359-8fdc-8c72fcbb6f38", "job": "lint"});
        let text = render("ci_status", &args, &value);
        assert!(
            text.contains("This run has no job named \"lint\"; its jobs: quality, e2e."),
            "{text}"
        );
        Ok(())
    }

    /// SPEC-145: a failed job's failing tests, kept from its whole output, come before the end
    /// of its output, which no longer holds them.
    #[test]
    fn a_failed_job_shows_its_failing_tests_before_the_end_of_its_output() -> TestResult {
        let mut quality = job(
            "quality",
            "failed",
            Some(1),
            None,
            "…PASS [   0.1s] (1670/1670) b\nerror: test run failed",
        );
        quality.failures = Some(
            "FAIL [   2.159s] (290/1670) yulius::main local_ci::waits\n  stderr ───\n    \
             assertion `left == right` failed: the message\n\n"
                .into(),
        );
        let value = serde_json::to_value(run("r1", "PR #370", "failed", vec![quality]))?;
        let text = render("ci_status", &json!({"run_id": "r1"}), &value);
        let failing = text
            .find("  Failing tests of quality (kept from all of its output):\nFAIL [   2.159s]")
            .ok_or_else(|| format!("no failing tests: {text}"))?;
        assert!(
            text.contains("assertion `left == right` failed: the message"),
            "{text}"
        );
        let end = text.find("Output of quality (its end):").ok_or("no end")?;
        assert!(failing < end, "{text}");
        assert!(text.ends_with("error: test run failed"), "{text}");

        // A job without kept failures shows its end as before.
        let value = serde_json::to_value(run(
            "r2",
            "PR #370",
            "failed",
            vec![job("lint", "failed", Some(1), None, "lint error")],
        ))?;
        let text = render("ci_status", &json!({"run_id": "r2"}), &value);
        assert!(!text.contains("Failing tests"), "{text}");

        // Across a run's failed jobs at most FAILURES_SHOWN bytes of them; the rest by name.
        let big = |name: &str| {
            let mut job = job(name, "failed", Some(1), None, "end");
            job.failures = Some(format!("FAIL [ 1.0s] {name}\n{}\n", "x".repeat(20 * 1024)));
            job
        };
        let value =
            serde_json::to_value(run("r3", "PR #370", "failed", vec![big("one"), big("two")]))?;
        let text = render("ci_status", &json!({"run_id": "r3"}), &value);
        assert!(text.contains("FAIL [ 1.0s] one"), "{text}");
        assert!(!text.contains("FAIL [ 1.0s] two"), "{text}");
        assert!(
            text.contains("  Failing tests of two: not shown here; ci_status with this run_id and job set to its name."),
            "{text}"
        );
        let text = render("ci_status", &json!({"run_id": "r3", "job": "two"}), &value);
        assert!(text.contains("FAIL [ 1.0s] two"), "{text}");
        Ok(())
    }

    /// TOL-003: the list prints each command once and each run on a line, with its jobs only when
    /// they did not all end like the run.
    #[test]
    fn the_ci_list_prints_each_command_once_and_a_line_per_run() -> TestResult {
        let mut failed = serde_json::to_value(run(
            "89154695-bd84-4359-8fdc-8c72fcbb6f38",
            "PR #370",
            "failed",
            vec![
                job("quality", "failed", Some(1), None, ""),
                job(
                    "e2e",
                    "cancelled",
                    None,
                    Some("stopped early: job quality failed"),
                    "",
                ),
            ],
        ))?;
        failed["this_task"] = json!(true);
        let mut reused = run(
            "fa71c732-f143-48e1-80de-bbfb16612e7c",
            "dev",
            "passed",
            vec![
                job(
                    "quality",
                    "passed",
                    Some(0),
                    Some("not run again: same tree"),
                    "",
                ),
                job(
                    "e2e",
                    "passed",
                    Some(0),
                    Some("not run again: same tree"),
                    "",
                ),
            ],
        );
        reused.reason = Some("not run again: same tree".into());
        reused.duration_ms = Some(0);
        let mut reused = serde_json::to_value(reused)?;
        reused["this_task"] = json!(false);
        let value = json!({
            "ci": {"source": "yulius_toml", "jobs": [
                {"name": "quality", "command": "scripts/quality --all", "matrix": null},
                {"name": "e2e", "command": "scripts/e2e --all", "matrix": null}
            ]},
            "problem": null,
            "github_actions_workflows": [],
            "pull_request": 370,
            "runs": [failed, reused],
            "note": "Read a run's output with run_id."
        });
        let text = render("ci_status", &json!({}), &value);
        assert_eq!(
            text,
            "CI (yulius_toml): 2 jobs\n\
             - quality: scripts/quality --all\n\
             - e2e: scripts/e2e --all\n\
             Your pull request: #370.\n\
             Recent runs (yours first):\n\
             - 89154695-bd84-4359-8fdc-8c72fcbb6f38 PR #370 e793eab: failed in 6m 31s (yours)\n  \
             quality failed (exit 1) in 6m 30s · e2e cancelled in 6m 30s: stopped early: job quality failed\n\
             - fa71c732-f143-48e1-80de-bbfb16612e7c dev e793eab: passed — not run again: same tree\n\
             Read a run's output with run_id."
        );
        let rerun = render("ci_rerun", &json!({"run_id": "x"}), &value["runs"][1]);
        assert!(
            rerun.starts_with("Queued again: fa71c732-f143-48e1-80de-bbfb16612e7c dev"),
            "{rerun}"
        );
        Ok(())
    }

    /// TOL-004: delivery status says the stage, the pull request, the spec and what blocks.
    #[test]
    fn delivery_status_reads_as_a_few_lines() -> TestResult {
        let delivery = Delivery {
            stage: "pr_open".into(),
            environment: None,
            pr: Some(PullView {
                number: 370,
                url: "https://github.com/o/r/pull/370".into(),
                title: "fix(prompt): thumbnails load".into(),
                state: "open".into(),
                base: "dev".into(),
                head: "feat/63".into(),
                checks: "pending".into(),
            }),
            spec_path: Some(".ai/specs/SPEC-080.md".into()),
            spec_valid: false,
            spec_problems: vec!["Plan: step 2 is not checked".into()],
            steps_open: 1,
            next_actions: vec![],
            blocked: vec![BlockedAction {
                action: "merge".into(),
                reason: "the checks have not passed".into(),
            }],
            notes: vec![],
            checks_access: None,
        };
        let value = json!({
            "stage": delivery.stage,
            "environment": delivery.environment,
            "pull_request": delivery.pr,
            "spec_path": delivery.spec_path,
            "spec_valid": delivery.spec_valid,
            "spec_problems": delivery.spec_problems,
            "steps_open": delivery.steps_open,
            "next_actions_for_people": delivery.next_actions,
            "blocked": delivery.blocked,
            "notes": delivery.notes,
            "checks_access": null,
            "profile": {"preset": "standard", "integration_branch": "dev", "production_branch": "main", "merge": "squash", "promote": "merge_commit", "auto_promote": false, "require_checks": true, "environments": [], "gate": null},
            "policy": "You deliver your own work (DLV-008): a long paragraph.",
        });
        let text = render("delivery_status", &json!({}), &value);
        assert_eq!(
            text,
            "Stage: pr_open.\n\
             Pull request #370 open into dev, checks pending: fix(prompt): thumbnails load\n  \
             https://github.com/o/r/pull/370\n\
             Spec .ai/specs/SPEC-080.md: not valid.\n\
             - Plan: step 2 is not checked\n\
             Open steps: 1.\n\
             Refused now:\n\
             - merge: the checks have not passed\n\
             Profile standard: squash merge into dev, merge_commit promotion to main; checks required."
        );
        Ok(())
    }

    /// TOL-004: tasks are one line each, named by number; their ids are left out.
    #[test]
    fn tasks_read_as_one_line_each() {
        let task = |number: u64, title: &str| {
            json!({
                "task_id": UUID, "number": number, "title": title, "harness": "claude",
                "status": "running", "activity": "working", "branch": format!("feat/{number}-x"),
                "spec_path": null, "steps": {"done": 2, "total": 4, "percent": 50, "current": "Build"},
                "runs_after": 60, "created_at": "2026-10-13T06:44:27Z", "updated_at": "2026-10-13T20:10:29Z"
            })
        };
        let value = json!({"you": task(72, "Mine"), "tasks": [task(63, "Theirs")]});
        let text = render("tasks_list", &json!({}), &value);
        assert_eq!(
            text,
            "You: #72 Mine · running (working) · branch feat/72-x · steps 2/4, 50%, now \"Build\" · after #60 · created 2026-10-13T06:44:27Z · updated 2026-10-13T20:10:29Z\n\
             Tasks (1, most recently updated first):\n\
             - #63 Theirs · running (working) · branch feat/63-x · steps 2/4, 50%, now \"Build\" · after #60 · created 2026-10-13T06:44:27Z · updated 2026-10-13T20:10:29Z"
        );
        assert!(!text.contains(UUID));
        let mut one = task(47, "Fix issues");
        one["prompt"] = json!("Fix all open issues\nand close them");
        one["steps_list"] = json!([{"id": UUID, "title": "Reproduce", "status": "done", "note": null}, {"id": UUID, "title": "Fix", "status": "pending", "note": null}]);
        let text = render("task_get", &json!({"number": 47}), &one);
        assert!(
            text.ends_with(
                "Steps:\n1 [x] Reproduce\n2 [ ] Fix\nPrompt:\nFix all open issues\nand close them"
            ),
            "{text}"
        );
        assert!(!text.contains(UUID));
    }

    /// TOL-001: text a tool returns (a briefing, a transcript) is printed as written, not escaped.
    #[test]
    fn long_text_is_printed_as_written() {
        let context = json!({"context": "# Project\n\n- rule one"});
        assert_eq!(
            render("project_context", &json!({}), &context),
            "# Project\n\n- rule one"
        );
        let page = json!({"id": "ses_1", "kind": "task", "title": "Checkout", "task_number": 12, "author": null, "from": 40, "next_from": 100, "text": "[41 user] hi\n[42 assistant] hello"});
        assert_eq!(
            render("session_read", &json!({"id": "ses_1"}), &page),
            "Session ses_1 \"Checkout\" (task #12), from line 40; continue with from: 100.\n[41 user] hi\n[42 assistant] hello"
        );
    }

    /// SPEC-083 NUM-001: a record number reads as the one line saying the number and the file.
    #[test]
    fn a_record_number_reads_as_its_next_step() {
        let next =
            "0066 is reserved for this task: name the file `docs/decisions/0066-<slug>.md` now.";
        let record = json!({"dir": "docs/decisions", "number": "0066",
            "file": "docs/decisions/0066-<slug>.md", "again": false, "next": next});
        assert_eq!(
            render("record_number", &json!({"dir": "docs/decisions"}), &record),
            next
        );
    }
}
