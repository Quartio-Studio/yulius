//! Split proposals by one headless Claude Code run (SPEC-002 ADR-16): `claude -p` with a
//! connected account's OAuth token, the small fast model, no tools, no MCP servers, no slash
//! commands, no saved session, JSON output checked against a schema. The request goes on stdin
//! (never argv), the run gets an empty private config directory and a minimal environment, and
//! it is bounded in time and output; on timeout the process is killed and reaped.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use serde_json::Value;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use yulius_app::model::Secret;
use yulius_app::ports::intake::{ProposerFailure, SplitProposer};
use yulius_domain::intake::{MAX_PROPOSED_TASKS, ProposedTask};

/// The model alias Claude Code resolves to its current small fast model.
pub const SPLIT_MODEL: &str = "haiku";
/// Longest run.
pub const SPLIT_TIMEOUT: Duration = Duration::from_secs(60);
/// Largest stdout read.
pub const MAX_OUTPUT_BYTES: usize = 256 * 1024;
/// How long a killed run may take to be reaped.
const REAP_WAIT: Duration = Duration::from_secs(5);

/// What Claude is asked to do (the request follows on stdin).
pub const SYSTEM_PROMPT: &str = "You split a software change request into coding tasks for \
    separate agents. Consistency beats granularity: if the request is one coherent thread, \
    return exactly one task. Otherwise return the SMALLEST sensible number of tasks: normally \
    2, never more than 4. Group items by area into coherent workstreams that could be built \
    and reviewed independently (for example user-interface work and backend work); never make \
    one task per item or bullet, and keep items of the same area or thread together. Each \
    task has a title of the form \"Area: what\" (for example \"UX: layout, terminal and messages \
    fixes\"), at most 80 characters; a prompt that copies its items of the request word for \
    word (keep their exact text and any shared context from the request); and \
    after_previous = true only when it cannot start before the previous task is finished. \
    Every item of the request must appear in exactly one task. Answer with the JSON object \
    only.";

/// The output schema (`--json-schema`).
pub const SCHEMA: &str = r#"{"type":"object","properties":{"tasks":{"type":"array","minItems":1,"maxItems":4,"items":{"type":"object","properties":{"title":{"type":"string"},"prompt":{"type":"string"},"after_previous":{"type":"boolean"}},"required":["title","prompt","after_previous"],"additionalProperties":false}}},"required":["tasks"],"additionalProperties":false}"#;

/// The proposer.
#[derive(Debug, Clone)]
pub struct ClaudeSplitProposer {
    program: PathBuf,
    runtime_dir: PathBuf,
    timeout: Duration,
}

static RUNS: AtomicU64 = AtomicU64::new(0);

impl ClaudeSplitProposer {
    /// Runs `program` (`claude`, resolved through `PATH`) in private directories under
    /// `runtime_dir`.
    #[must_use]
    pub fn new(program: impl Into<PathBuf>, runtime_dir: impl Into<PathBuf>) -> Self {
        Self {
            program: program.into(),
            runtime_dir: runtime_dir.into(),
            timeout: SPLIT_TIMEOUT,
        }
    }

    /// The same proposer with another time bound (tests).
    #[must_use]
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// The arguments after the program (no request text, no secret).
    #[must_use]
    pub fn args() -> Vec<String> {
        [
            "-p",
            "--model",
            SPLIT_MODEL,
            "--output-format",
            "json",
            "--json-schema",
            SCHEMA,
            "--tools",
            "",
            "--strict-mcp-config",
            "--disable-slash-commands",
            "--no-session-persistence",
            "--system-prompt",
            SYSTEM_PROMPT,
        ]
        .iter()
        .map(|s| (*s).to_owned())
        .collect()
    }

    fn run_dir(&self) -> Result<PathBuf, ProposerFailure> {
        use std::os::unix::fs::DirBuilderExt;
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        let dir = self.runtime_dir.join(format!(
            "split-{}-{nanos}-{}",
            std::process::id(),
            RUNS.fetch_add(1, Ordering::SeqCst)
        ));
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(dir.join(".claude"))
            .map_err(|_| ProposerFailure("cannot prepare the proposal directory".into()))?;
        Ok(dir)
    }

    async fn run(
        &self,
        dir: &Path,
        credential: &Secret,
        request: &str,
    ) -> Result<Vec<u8>, ProposerFailure> {
        let mut command = tokio::process::Command::new(&self.program);
        command
            .args(Self::args())
            .current_dir(dir)
            .env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .env("HOME", dir)
            .env("CLAUDE_CONFIG_DIR", dir.join(".claude"))
            .env("CLAUDE_CODE_OAUTH_TOKEN", credential.expose().trim())
            .env("DISABLE_AUTOUPDATER", "1")
            .env("CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC", "1")
            .env("TERM", "dumb")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        let mut child = command
            .spawn()
            .map_err(|_| ProposerFailure("Claude Code could not be started".into()))?;
        let mut stdin = child
            .stdin
            .take()
            .ok_or_else(|| ProposerFailure("no stdin".into()))?;
        let mut stdout = child
            .stdout
            .take()
            .ok_or_else(|| ProposerFailure("no stdout".into()))?;
        let request = request.as_bytes().to_vec();
        let work = async move {
            stdin
                .write_all(&request)
                .await
                .map_err(|_| ProposerFailure("cannot send the request".into()))?;
            drop(stdin);
            let mut out = Vec::new();
            (&mut stdout)
                .take(MAX_OUTPUT_BYTES as u64 + 1)
                .read_to_end(&mut out)
                .await
                .map_err(|_| ProposerFailure("cannot read the answer".into()))?;
            if out.len() > MAX_OUTPUT_BYTES {
                return Err(ProposerFailure("the answer is too large".into()));
            }
            Ok(out)
        };
        let outcome = tokio::time::timeout(self.timeout, async {
            let out = work.await?;
            let status = child
                .wait()
                .await
                .map_err(|_| ProposerFailure("cannot wait for Claude Code".into()))?;
            Ok::<_, ProposerFailure>((out, status))
        })
        .await;
        match outcome {
            Ok(Ok((out, status))) if status.success() => Ok(out),
            Ok(Ok((_, status))) => {
                Err(ProposerFailure(format!("Claude Code exited with {status}")))
            }
            Ok(Err(err)) => {
                reap(&mut child).await;
                Err(err)
            }
            Err(_) => {
                reap(&mut child).await;
                Err(ProposerFailure(format!(
                    "no proposal within {} s",
                    self.timeout.as_secs()
                )))
            }
        }
    }
}

/// Kills the run and waits (bounded) until it is reaped.
async fn reap(child: &mut tokio::process::Child) {
    let _ = child.start_kill();
    if tokio::time::timeout(REAP_WAIT, child.wait()).await.is_err() {
        tracing::warn!("a timed-out split proposal run was not reaped in time");
    }
}

/// Reads Claude Code's `--output-format json` result: `structured_output` when present, else
/// the `result` text as JSON (code fences removed). At most a few more tasks than shown are
/// read; the domain checks the rest.
///
/// # Errors
/// [`ProposerFailure`] for errors and unusable output.
pub fn parse_output(stdout: &[u8]) -> Result<Vec<ProposedTask>, ProposerFailure> {
    let bad = || ProposerFailure("Claude's answer is not a usable proposal".into());
    let result: Value = serde_json::from_slice(stdout).map_err(|_| bad())?;
    if result.get("is_error").and_then(Value::as_bool) == Some(true) {
        return Err(ProposerFailure("Claude reported an error".into()));
    }
    let proposal = match result.get("structured_output") {
        Some(value) if value.is_object() => value.clone(),
        _ => {
            let text = result
                .get("result")
                .and_then(Value::as_str)
                .ok_or_else(bad)?;
            let text = text.trim();
            let text = text
                .strip_prefix("```json")
                .or_else(|| text.strip_prefix("```"))
                .map_or(text, |rest| rest.trim_end().trim_end_matches("```"));
            serde_json::from_str(text.trim()).map_err(|_| bad())?
        }
    };
    let tasks = proposal
        .get("tasks")
        .and_then(Value::as_array)
        .ok_or_else(bad)?;
    tasks
        .iter()
        .take(MAX_PROPOSED_TASKS * 4)
        .map(|task| {
            Ok(ProposedTask {
                title: task
                    .get("title")
                    .and_then(Value::as_str)
                    .ok_or_else(bad)?
                    .to_owned(),
                prompt: task
                    .get("prompt")
                    .and_then(Value::as_str)
                    .ok_or_else(bad)?
                    .to_owned(),
                after_previous: task
                    .get("after_previous")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
            })
        })
        .collect()
}

#[async_trait]
impl SplitProposer for ClaudeSplitProposer {
    async fn propose(
        &self,
        credential: &Secret,
        request: &str,
    ) -> Result<Vec<ProposedTask>, ProposerFailure> {
        let dir = self.run_dir()?;
        let result = self.run(&dir, credential, request).await;
        if let Err(err) = std::fs::remove_dir_all(&dir) {
            tracing::debug!(error = %err, "cannot remove a split proposal directory");
        }
        parse_output(&result?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    type TestResult = Result<(), Box<dyn std::error::Error>>;
    const TOKEN: &str = "sk-ant-oat01-synthetic";

    /// A fake `claude`: checks that the request came on stdin and not argv, that the token and
    /// the private config directory are set and no other credential leaks in, then prints
    /// `answer`.
    fn fake_claude(dir: &Path, answer: &str, sleep: u32) -> Result<PathBuf, std::io::Error> {
        use std::os::unix::fs::PermissionsExt;
        let script = dir.join("claude");
        std::fs::write(dir.join("answer.json"), answer)?;
        std::fs::write(
            &script,
            format!(
                "#!/bin/sh\nset -e\nrequest=$(cat)\ncase \"$*\" in *\"$request\"*) echo argv-leak >&2; exit 9;; esac\n\
                 [ \"$CLAUDE_CODE_OAUTH_TOKEN\" = \"{TOKEN}\" ] || exit 7\n\
                 [ -d \"$CLAUDE_CONFIG_DIR\" ] || exit 6\n[ -z \"$ANTHROPIC_API_KEY\" ] || exit 5\n\
                 printf '%s' \"$request\" > \"{}/request.txt\"\nsleep {sleep}\ncat \"{}/answer.json\"\n",
                dir.display(),
                dir.display()
            ),
        )?;
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700))?;
        Ok(script)
    }

    fn secret() -> Secret {
        Secret(TOKEN.into())
    }

    #[tokio::test]
    async fn structured_output_becomes_tasks_and_the_run_directory_is_removed() -> TestResult {
        let dir = tempfile::tempdir()?;
        let answer = serde_json::json!({"type": "result", "subtype": "success", "is_error": false,
            "result": "", "structured_output": {"tasks": [
                {"title": "UX: fixes", "prompt": "1. a", "after_previous": false},
                {"title": "Backend: pool", "prompt": "2. b", "after_previous": true}]}});
        let program = fake_claude(dir.path(), &answer.to_string(), 0)?;
        let runtime = dir.path().join("runtime");
        let proposer = ClaudeSplitProposer::new(&program, &runtime);
        let tasks = proposer
            .propose(&secret(), "1. a\n2. b and more words")
            .await
            .map_err(|e| e.to_string())?;
        assert_eq!(tasks.len(), 2);
        assert!(tasks[1].after_previous);
        assert_eq!(
            std::fs::read_to_string(dir.path().join("request.txt"))?,
            "1. a\n2. b and more words"
        );
        assert_eq!(
            std::fs::read_dir(&runtime)?.count(),
            0,
            "run directory removed"
        );
        Ok(())
    }

    #[test]
    fn result_text_errors_and_bad_shapes() -> Result<(), String> {
        let fenced = serde_json::json!({"is_error": false, "result":
            "```json\n{\"tasks\":[{\"title\":\"One\",\"prompt\":\"all\",\"after_previous\":false}]}\n```"});
        let tasks = parse_output(fenced.to_string().as_bytes()).map_err(|e| e.0)?;
        assert_eq!(tasks.len(), 1);
        assert!(parse_output(br#"{"is_error": true, "result": "rate limited"}"#).is_err());
        assert!(parse_output(b"not json").is_err());
        assert!(parse_output(br#"{"result": "no json here"}"#).is_err());
        assert!(parse_output(br#"{"structured_output": {"tasks": [{"title": 1}]}}"#).is_err());
        Ok(())
    }

    #[tokio::test]
    async fn a_hanging_run_is_killed_within_the_bound_and_failures_are_typed() -> TestResult {
        let dir = tempfile::tempdir()?;
        let program = fake_claude(dir.path(), "{}", 30)?;
        let proposer = ClaudeSplitProposer::new(&program, dir.path().join("runtime"))
            .with_timeout(Duration::from_millis(300));
        let started = std::time::Instant::now();
        let failure = proposer.propose(&secret(), "request").await;
        assert!(failure.is_err());
        assert!(started.elapsed() < Duration::from_secs(5));
        let missing = ClaudeSplitProposer::new(dir.path().join("nope"), dir.path().join("rt"));
        assert!(missing.propose(&secret(), "request").await.is_err());
        // A wrong token makes the fake exit non-zero: a typed failure, nothing leaked.
        let program = fake_claude(dir.path(), "{}", 0)?;
        let wrong = ClaudeSplitProposer::new(&program, dir.path().join("rt2"));
        let err = wrong
            .propose(&Secret("other".into()), "request")
            .await
            .err()
            .map(|e| e.0)
            .unwrap_or_default();
        assert!(err.contains("exited"), "{err}");
        assert!(!err.contains("other"));
        Ok(())
    }

    #[test]
    fn arguments_carry_no_request_and_no_secret() {
        let args = ClaudeSplitProposer::args();
        assert!(args.contains(&"-p".to_owned()));
        assert!(args.contains(&SPLIT_MODEL.to_owned()));
        assert!(args.iter().all(|a| !a.contains("sk-ant")));
        let schema: Value = serde_json::from_str(SCHEMA).unwrap_or(Value::Null);
        assert_eq!(schema["properties"]["tasks"]["maxItems"], 4);
    }
}
