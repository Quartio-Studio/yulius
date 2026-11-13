//! A dynamic workflow (the Workflow tool) as Claude Code 2.1.323 records one (SPEC-486 WFL-007).
//! The shapes are those of runs of the CLI against a local stand-in for the model API.
//!
//! - The launch is a tool result in the session's transcript (`toolUseResult` `{status:
//!   "async_launched", taskType: "local_workflow", taskId, runId, scriptPath, …}`); the turn
//!   then ends with a `turn_duration` that counts it (`pendingWorkflowCount`).
//! - The run's agents write to `<session>/subagents/workflows/<run>/agent-<id>.jsonl`, beside
//!   the run's `journal.jsonl` (each finished agent's result).
//! - A usage limit that refuses an agent is written to that agent's file only: the workflow
//!   pauses and the session's own thread is not woken (`--workflow-limit`).
//! - A process that resumes the session finds the run unfinished and says so in a notification
//!   for the transcript only (`queueTranscriptOnly`); nothing resumes the run by itself.
//! - `Workflow({scriptPath, resumeFromRunId})` relaunches it under a new task id: only the
//!   unfinished agents run, then the run's snapshot (`<session>/workflows/<run>.json`) is
//!   written and a task notification wakes the session's thread.

use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use serde_json::{Value, json};

use super::{EchoState, Hooks, Options, Transcript, cli_version, now_ms, rfc3339, say};

/// The file of a run's folder that names the task id of its last launch (the fake agent's own:
/// Claude Code keeps that in memory and in the session's transcript).
const TASK_FILE: &str = "fake-task";

/// How long an agent of a run takes.
const AGENT_TIME: std::time::Duration = std::time::Duration::from_millis(30);

/// The workflow's name (`meta.name` of its script).
const NAME: &str = "fake-wave";

/// The folders and files of the workflows of the session whose transcript is `transcript`.
struct Layout {
    session: PathBuf,
}

impl Layout {
    fn of(transcript: &Path) -> Self {
        Self {
            session: transcript.with_extension(""),
        }
    }

    fn runs(&self) -> PathBuf {
        self.session.join("subagents").join("workflows")
    }

    fn run(&self, run: &str) -> PathBuf {
        self.runs().join(run)
    }

    fn snapshot(&self, run: &str) -> PathBuf {
        self.session.join("workflows").join(format!("{run}.json"))
    }

    fn script(&self, run: &str) -> PathBuf {
        self.session
            .join("workflows")
            .join("scripts")
            .join(format!("{NAME}-{run}.js"))
    }

    /// The runs launched and not finished, with the task id of their last launch.
    fn unfinished(&self) -> Vec<(String, String)> {
        let mut out = Vec::new();
        let Ok(entries) = std::fs::read_dir(self.runs()) else {
            return out;
        };
        for entry in entries.flatten() {
            let run = entry.file_name().to_string_lossy().into_owned();
            let task = std::fs::read_to_string(entry.path().join(TASK_FILE));
            if let (Ok(task), false) = (task, self.snapshot(&run).exists()) {
                out.push((run, task.trim().to_owned()));
            }
        }
        out.sort();
        out
    }
}

fn append(path: &Path, records: &[Value]) -> io::Result<()> {
    use std::io::Write as _;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    for record in records {
        writeln!(file, "{record}")?;
    }
    Ok(())
}

/// A line of a workflow agent's transcript: the session's, on the side.
fn agent_record(session: &str, agent: &str, message: &Value) -> Value {
    json!({
        "parentUuid": null,
        "isSidechain": true,
        "agentId": agent,
        "type": "assistant",
        "uuid": format!("fake-{agent}-{}", now_ms()),
        "timestamp": rfc3339(now_ms()),
        "userType": "external",
        "entrypoint": "cli",
        "sessionId": session,
        "version": cli_version(),
        "message": message,
    })
}

/// An agent of `run` that finished with `label done`: its transcript and the journal's lines.
fn agent_finishes(layout: &Layout, session: &str, run: &str, label: &str) -> io::Result<()> {
    let agent = format!("a{label}{:x}", now_ms());
    let answer = format!("{label} done");
    let message = json!({
        "id": format!("msg_fake_{agent}"), "type": "message", "role": "assistant",
        "model": "fake-model", "content": [{"type": "text", "text": answer}],
        "stop_reason": "end_turn",
        "usage": {"input_tokens": 7, "cache_creation_input_tokens": 0,
            "cache_read_input_tokens": 0, "output_tokens": 3},
    });
    let dir = layout.run(run);
    append(
        &dir.join(format!("agent-{agent}.jsonl")),
        &[agent_record(session, &agent, &message)],
    )?;
    let key = format!("v2:{label}");
    append(
        &dir.join("journal.jsonl"),
        &[
            json!({"type": "started", "key": key, "agentId": agent, "label": label,
                "phase": "Wave"}),
            json!({"type": "result", "key": key, "agentId": agent, "result": answer}),
        ],
    )
}

/// An agent of `run` that the usage limit refuses: the record is in its file only, and the
/// journal has its start without a result.
fn agent_is_refused(layout: &Layout, session: &str, run: &str, label: &str) -> io::Result<()> {
    let agent = format!("a{label}{:x}", now_ms());
    let resets_at = now_ms() / 1000 + 3600;
    let info = json!({"status": "rejected", "resetsAt": resets_at, "rateLimitType": "five_hour",
        "overageStatus": "rejected", "overageDisabledReason": "org_level_disabled",
        "isUsingOverage": false});
    let message = json!({
        "id": format!("msg_fake_{agent}"), "type": "message", "role": "assistant",
        "model": "<synthetic>",
        "content": [{"type": "text", "text": "You've hit your 5-hour limit · resets in 1 hour"}],
        "stop_reason": "stop_sequence", "stop_sequence": "",
        "usage": {"input_tokens": 0, "cache_creation_input_tokens": 0,
            "cache_read_input_tokens": 0, "output_tokens": 0},
    });
    let mut record = agent_record(session, &agent, &message);
    record["apiError"] = json!("usage_limit_reached");
    record["apiErrorParams"] = json!({"rate_limit_info": info.clone()});
    record["quotaLimits"] = info;
    record["error"] = json!("rate_limit");
    record["isApiErrorMessage"] = json!(true);
    record["apiErrorStatus"] = json!(429);
    let dir = layout.run(run);
    append(&dir.join(format!("agent-{agent}.jsonl")), &[record])?;
    append(
        &dir.join("journal.jsonl"),
        &[
            json!({"type": "started", "key": format!("v2:{label}"), "agentId": agent,
            "label": label, "phase": "Wave"}),
        ],
    )
}

impl Transcript {
    /// The Workflow tool's result for a launch of `run` as task `task`.
    fn workflow_launched(&mut self, layout: &Layout, task: &str, run: &str) -> io::Result<()> {
        let dir = layout.run(run);
        let script = layout.script(run);
        self.tool_result(
            &format!(
                "Workflow launched in background. Task ID: {task}\nSummary: A wave of agents\n\
                 Transcript dir: {}\nScript file: {}\nTo resume after editing the script: \
                 Workflow({{scriptPath: \"{}\", resumeFromRunId: \"{run}\"}})",
                dir.display(),
                script.display(),
                script.display()
            ),
            json!({"status": "async_launched", "taskId": task, "taskType": "local_workflow",
                "workflowName": NAME, "runId": run, "summary": "A wave of agents",
                "transcriptDir": dir.display().to_string(),
                "scriptPath": script.display().to_string()}),
        )
    }

    /// The records closing a turn that leaves `pending` workflows unfinished; `stopped` says its
    /// Stop hooks ran (a turn a usage limit cut runs none).
    fn close_workflow_turn(&mut self, pending: u32, stopped: bool) -> io::Result<()> {
        let timestamp = rfc3339(now_ms());
        let mut records = Vec::new();
        if stopped {
            let uuid = format!("fake-{}-{}-stop", self.session_id, self.turns);
            let mut summary = self.claude_envelope("system", &uuid, &timestamp);
            summary["subtype"] = json!("stop_hook_summary");
            summary["hookCount"] = json!(2);
            summary["hookErrors"] = json!([]);
            summary["preventedContinuation"] = json!(false);
            records.push(summary);
        }
        let uuid = format!("fake-{}-{}-duration", self.session_id, self.turns);
        let mut duration = self.claude_envelope("system", &uuid, &timestamp);
        duration["subtype"] = json!("turn_duration");
        duration["durationMs"] = json!(169);
        duration["messageCount"] = json!(18);
        if pending > 0 {
            duration["pendingWorkflowCount"] = json!(pending);
        }
        self.last_uuid = Some(uuid);
        records.push(duration);
        self.append(&records)
    }

    /// A task notification about the workflow task `task`: the wake-up of its end, or, `only`
    /// for the transcript, what a resumed process says of a run the ended one left unfinished.
    fn workflow_notified(&mut self, task: &str, body: &str, only: bool) -> io::Result<()> {
        self.turns += 1;
        let uuid = format!("fake-{}-{}-notified", self.session_id, self.turns);
        let mut user = self.claude_envelope("user", &uuid, &rfc3339(now_ms()));
        user["message"] = json!({"role": "user", "content": format!(
            "<task-notification>\n<task-id>{task}</task-id>\n<tool-use-id>toolu_fake</tool-use-id>\n{body}\n</task-notification>"
        )});
        user["origin"] = json!({"kind": "task-notification"});
        user["promptSource"] = json!("system");
        user["queueSkipAttachments"] = json!(true);
        if only {
            user["queueTranscriptOnly"] = json!(true);
        }
        self.last_uuid = Some(uuid);
        self.append(&[user])
    }
}

/// What a process that resumes a session writes about each workflow the ended process left
/// unfinished: a notification for the transcript only. It starts no turn.
pub(super) fn orphaned(transcript: &Arc<Mutex<Transcript>>) -> io::Result<()> {
    let Ok(mut t) = transcript.lock() else {
        return Ok(());
    };
    let layout = Layout::of(&t.path);
    for (run, task) in layout.unfinished() {
        t.workflow_notified(
            &task,
            &format!(
                "<status>stopped</status>\n<summary>Background workflow \"{NAME}\" (run {run}) \
                 didn't finish before the previous session ended</summary>\n<note>No completion \
                 record was found for it in the previous session. To pick up where it left off, \
                 relaunch with Workflow({{scriptPath, resumeFromRunId}}) using the run id from \
                 the summary — completed agent() calls return cached.</note>"
            ),
            true,
        )?;
        say(&format!("WORKFLOW {run} ORPHANED"))?;
    }
    Ok(())
}

/// `/workflow`: the session's agent launches a workflow and ends its turn waiting for it. Its
/// first agent finishes. With `--workflow-limit` the second is refused by a usage limit and the
/// workflow pauses (`WORKFLOW <run> PAUSED`); with `--limit-after` reached, the session's own
/// thread is refused as well, after the launch (`LIMIT REACHED`), and its turn ends unanswered.
pub(super) fn launch(
    options: &Options,
    line: &str,
    transcript: Option<&Arc<Mutex<Transcript>>>,
    hooks: Option<&Hooks>,
    state: &mut EchoState,
) -> io::Result<()> {
    if let Some(reason) = hooks.and_then(|h| h.prompt(line)) {
        return say(&format!("PROMPT BLOCKED: {reason}"));
    }
    let Some(transcript) = transcript else {
        return say("WORKFLOW NEEDS A TRANSCRIPT");
    };
    state.turns += 1;
    let (layout, session, run, task) = {
        let Ok(mut t) = transcript.lock() else {
            return Ok(());
        };
        let layout = Layout::of(&t.path);
        let n = std::fs::read_dir(layout.runs()).map_or(0, Iterator::count) + 1;
        let run = format!("wf_fake{n:04}-{:03x}", now_ms() & 0xfff);
        let task = format!("w{:08x}", now_ms() & 0xffff_ffff);
        t.begin(line)?;
        t.tool_use()?;
        std::fs::create_dir_all(layout.run(&run))?;
        std::fs::write(layout.run(&run).join(TASK_FILE), &task)?;
        append(
            &layout.run(&run).join("journal.jsonl"),
            &[json!({"type": "launched"})],
        )?;
        t.workflow_launched(&layout, &task, &run)?;
        (layout, t.session_id.clone(), run, task)
    };
    say(&format!("WORKFLOW {run} LAUNCHED task={task}"))?;
    let cut = options
        .limit_after
        .is_some_and(|limit| state.turns >= limit);
    if cut {
        // The session's own request after the launch is refused: no answer, no Stop hook.
        super::limit_reached(options, Some(transcript), state)?;
    } else if let Ok(mut t) = transcript.lock() {
        t.answer(line, "The workflow runs in the background; I wait for it.")?;
    }
    if let (false, Some(hooks)) = (cut, hooks) {
        hooks.stop()?;
    }
    if let Ok(mut t) = transcript.lock() {
        t.close_workflow_turn(1, !cut)?;
    }
    // The agents work after the turn that launched them ended.
    std::thread::sleep(AGENT_TIME);
    agent_finishes(&layout, &session, &run, "alpha")?;
    if options.workflow_limit {
        std::thread::sleep(AGENT_TIME);
        agent_is_refused(&layout, &session, &run, "beta")?;
        say(&format!("WORKFLOW {run} PAUSED"))?;
    }
    Ok(())
}

/// The run ids `text` names (`wf_…`), in its order, each once.
fn runs_named(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for (start, _) in text.match_indices("wf_") {
        let run: String = text[start..]
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-'))
            .collect();
        if !out.contains(&run) {
            out.push(run);
        }
    }
    out
}

/// A prompt that tells the agent to resume unfinished runs by their ids (`resumeFromRunId` and
/// the ids): each named run that is unfinished is relaunched under a new task id, its remaining
/// agents finish, its snapshot is written and its notification wakes the session's thread
/// (`WORKFLOW <run> RESUMED`, then `WORKFLOW <run> COMPLETED`). `false` when `prompt` is no such
/// prompt or names no unfinished run: it is then a message like any other.
pub(super) fn resume(
    prompt: &str,
    transcript: Option<&Arc<Mutex<Transcript>>>,
    hooks: Option<&Hooks>,
    state: &mut EchoState,
) -> io::Result<bool> {
    let Some(transcript) = transcript else {
        return Ok(false);
    };
    let (layout, session) = match transcript.lock() {
        Ok(t) => (Layout::of(&t.path), t.session_id.clone()),
        Err(_) => return Ok(false),
    };
    let unfinished = layout.unfinished();
    let runs: Vec<String> = runs_named(prompt)
        .into_iter()
        .filter(|run| unfinished.iter().any(|(known, _)| known == run))
        .collect();
    if !prompt.contains("resumeFromRunId") || runs.is_empty() {
        return Ok(false);
    }
    if let Some(reason) = hooks.and_then(|h| h.prompt(prompt)) {
        say(&format!("PROMPT BLOCKED: {reason}"))?;
        return Ok(true);
    }
    state.turns += 1;
    let mut tasks = Vec::new();
    if let Ok(mut t) = transcript.lock() {
        t.begin(prompt)?;
        for run in &runs {
            let task = format!("w{:08x}", (now_ms() + tasks.len() as i64) & 0xffff_ffff);
            t.tool_use()?;
            std::fs::write(layout.run(run).join(TASK_FILE), &task)?;
            t.workflow_launched(&layout, &task, run)?;
            tasks.push(task);
        }
        t.answer(prompt, "The workflow is resumed; I wait for it.")?;
    }
    for run in &runs {
        say(&format!("WORKFLOW {run} RESUMED"))?;
    }
    if let Some(hooks) = hooks {
        hooks.stop()?;
    }
    if let Ok(mut t) = transcript.lock() {
        t.close_workflow_turn(u32::try_from(runs.len()).unwrap_or(u32::MAX), true)?;
    }
    for (run, task) in runs.iter().zip(&tasks) {
        // Only the agents without a result in the journal run: the first one's is kept.
        agent_finishes(&layout, &session, run, "beta")?;
        agent_finishes(&layout, &session, run, "gamma")?;
        let snapshot = layout.snapshot(run);
        if let Some(parent) = snapshot.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(
            &snapshot,
            json!({"runId": run, "taskId": task, "timestamp": rfc3339(now_ms()),
                "result": {"alpha": "alpha done", "beta": "beta done", "gamma": "gamma done"}})
            .to_string(),
        )?;
        let body = format!(
            "<status>completed</status>\n<summary>Dynamic workflow \"A wave of agents\" \
             completed</summary>\n<result>{{\"alpha\":\"alpha done\",\"beta\":\"beta done\",\
             \"gamma\":\"gamma done\"}}</result>"
        );
        let wake = format!("<task-notification>\n<task-id>{task}</task-id>\n{body}");
        // The wake-up is a prompt of Claude Code's own: its hook, its turn, its Stop.
        let _ = hooks.and_then(|h| h.prompt(&wake));
        if let Ok(mut t) = transcript.lock() {
            t.workflow_notified(task, &body, false)?;
            t.answer(&wake, "The workflow is done.")?;
        }
        say(&format!("WORKFLOW {run} COMPLETED"))?;
        if let Some(hooks) = hooks {
            hooks.stop()?;
        }
        if let Ok(mut t) = transcript.lock() {
            t.close_workflow_turn(0, true)?;
        }
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_ids_are_read_from_a_prompt() {
        assert_eq!(
            runs_named("resume (runs wf_fake0001-a1b and wf_2), not wf_fake0001-a1b again."),
            ["wf_fake0001-a1b", "wf_2"]
        );
        assert!(runs_named("nothing to resume").is_empty());
    }

    /// A launch whose second agent is refused, and its resume: what is on disk at each point.
    #[test]
    fn a_paused_run_is_unfinished_until_it_is_resumed() -> io::Result<()> {
        let dir = std::env::temp_dir().join(format!("fake-agent-wf-{}", now_ms()));
        let transcript = dir.join("s1.jsonl");
        let layout = Layout::of(&transcript);
        let run = "wf_fake0001-000";
        std::fs::create_dir_all(layout.run(run))?;
        std::fs::write(layout.run(run).join(TASK_FILE), "w1")?;
        agent_finishes(&layout, "s1", run, "alpha")?;
        agent_is_refused(&layout, "s1", run, "beta")?;
        assert_eq!(layout.unfinished(), [(run.to_owned(), "w1".to_owned())]);
        let refused = std::fs::read_dir(layout.run(run))?
            .flatten()
            .map(|entry| std::fs::read_to_string(entry.path()).unwrap_or_default())
            .filter(|text| text.contains("\"usage_limit_reached\""))
            .count();
        assert_eq!(refused, 1, "in the refused agent's file only");
        std::fs::create_dir_all(layout.snapshot(run).parent().unwrap_or(&dir))?;
        std::fs::write(layout.snapshot(run), "{}")?;
        assert!(layout.unfinished().is_empty());
        std::fs::remove_dir_all(&dir)
    }
}
