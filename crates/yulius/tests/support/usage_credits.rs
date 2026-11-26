//! Exhausted models, a person's context and a session's transcript lines.

use crate::support::TestResult;
use crate::support::eventually;
use crate::support::live::pool::HOUR;
use serde_json::{Value, json};
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::time::Duration;
use yulius_app::auth::{AuthContext, rfc3339};
use yulius_app::context::App;
use yulius_domain::ids::{AccountId, UserId};
use yulius_domain::time::Timestamp;

/// Opus 5.5 is used up on `accounts` too, for six more days: a task of a Fable model has no
/// model to fall back to there (SPEC-482), so what SPEC-464 says about a model no subscription
/// can run still holds: the task waits for a reset, or a person is asked. A provider's reading
/// keeps the window (its reset is ahead); storing an account's usage whole replaces it.
pub(crate) async fn opus_out(app: &App, accounts: &[&str]) -> TestResult {
    for account in accounts {
        let id = AccountId::parse(account)?;
        let record = app.deps.store.account(&id).await?.ok_or("account")?;
        let mut usage: Value = serde_json::from_str(record.usage_json.as_deref().unwrap_or("{}"))?;
        usage["weekly_opus"] = json!({"used_percent": 100.0, "window_minutes": 10_080,
            "resets_at": app.now().plus_millis(6 * 24 * HOUR).0});
        app.deps
            .store
            .set_account_state(
                &id,
                record.status,
                Some(&usage.to_string()),
                None,
                app.now(),
            )
            .await?;
    }
    Ok(())
}

/// A person's context for `user`.
pub(crate) async fn person(
    app: &App,
    user: &UserId,
) -> Result<AuthContext, Box<dyn std::error::Error>> {
    Ok(AuthContext {
        user: app.deps.store.user(user).await?.ok_or("user")?,
        csrf_token: None,
        session_hash: None,
        token: None,
    })
}

/// Wait for the fake agent's complete transcript record for its first answer.
pub(crate) async fn first_turn_transcript(
    data: &Path,
) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let mut transcript = None;
    eventually("the first turn", Duration::from_secs(30), || {
        transcript = session_transcript(data, "msg_fake_1");
        let written = transcript.is_some();
        async move { written }
    })
    .await?;
    Ok(transcript.ok_or("transcript")?)
}

/// A default task switched to Fable meets its credit limit, with Opus also full here.
pub(crate) async fn append_switched_fable_limit(app: &App, data: &Path) -> TestResult {
    let transcript = first_turn_transcript(data).await?;
    opus_out(app, &["a-first"]).await?;
    append_lines(
        &transcript,
        "switched",
        &[
            Line::Prompt,
            Line::RequestOfFable,
            Line::CreditsRefusal,
            Line::TurnDuration,
        ],
    )
}

/// The transcript of the one session under `data`, once a record with `marker` is in it whole.
pub(crate) fn session_transcript(data: &Path, marker: &str) -> Option<PathBuf> {
    let mut dirs = vec![data.to_path_buf()];
    while let Some(dir) = dirs.pop() {
        for entry in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
            let path = entry.path();
            if entry.file_type().is_ok_and(|t| t.is_dir()) {
                dirs.push(path);
            } else if path.extension().is_some_and(|e| e == "jsonl")
                && path.components().any(|c| c.as_os_str() == "projects")
                && std::fs::read_to_string(&path)
                    .is_ok_and(|text| text.contains(marker) && text.ends_with('\n'))
            {
                return Some(path);
            }
        }
    }
    None
}

/// A record of a session's transcript as Claude Code writes it, for a test to append.
#[derive(Debug, Clone, Copy)]
pub(crate) enum Line {
    /// A prompt someone typed.
    Prompt,
    /// The person interrupted the running turn.
    Interrupt,
    /// The refusal of a model for its usage credits, as Claude Code 2.1.311 writes it (task
    /// #438's record of 2026-11-11): no window, no reset.
    CreditsRefusal,
    /// The subscription's own 5-hour limit, in a record that names no reset.
    SessionLimit,
    /// The refusal of a lapsed subscription, as Claude Code 2.1.311 writes it.
    AccessLost,
    /// The notice of a goal Claude Code paused at a refusal.
    GoalPause,
    /// The record closing a turn.
    TurnDuration,
    /// A line of the model's own work (a tool call).
    Work,
    /// The record Claude Code 2.1.325 writes before a request, here one that asks for Fable 5.2:
    /// the session's person switched the model inside it (`/model`).
    RequestOfFable,
    /// The refusal at the weekly Opus cap (`seven_day_opus`), which names its family.
    OpusCapRefusal,
}

/// Appends `lines` to `transcript`, written now, to the millisecond as Claude Code writes them.
/// `tag` tells one call's records from another's.
pub(crate) fn append_lines(transcript: &Path, tag: &str, lines: &[Line]) -> TestResult {
    let now = i64::try_from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_millis(),
    )?;
    let at = format!(
        "{}.{:03}Z",
        rfc3339(Timestamp(now)).trim_end_matches('Z'),
        now.rem_euclid(1000)
    );
    let text = std::fs::read_to_string(transcript)?;
    let last: Value = serde_json::from_str(text.lines().last().ok_or("an empty transcript")?)?;
    let session = last["sessionId"].as_str().ok_or("a session id")?.to_owned();
    let mut parent = last["uuid"].as_str().ok_or("a uuid")?.to_owned();
    let refusal = |id: &str, text: &str| {
        json!({"id": id, "type": "message", "role": "assistant", "model": "<synthetic>",
            "stop_reason": "stop_sequence", "content": [{"type": "text", "text": text}]})
    };
    let mut out = String::new();
    for (n, line) in lines.iter().enumerate() {
        let uuid = format!("{tag}-{n}");
        let (kind, extra) = match line {
            Line::Prompt => (
                "user",
                json!({"message": {"role": "user", "content": "go on"}}),
            ),
            Line::Interrupt => (
                "user",
                json!({"message": {"role": "user", "content": "[Request interrupted by user]"}}),
            ),
            Line::CreditsRefusal => (
                "assistant",
                json!({"isApiErrorMessage": true, "error": "rate_limit",
                    "apiError": "model_requires_usage_credits", "apiErrorStatus": 429,
                    "message": refusal(&uuid,
                        "You're out of usage credits. /model to switch models.")}),
            ),
            Line::SessionLimit => (
                "assistant",
                json!({"isApiErrorMessage": true, "error": "rate_limit",
                    "apiError": "usage_limit_reached", "apiErrorStatus": 429,
                    "apiErrorParams": {"rate_limit_info":
                        {"status": "rejected", "rateLimitType": "five_hour"}},
                    "message": refusal(&uuid, "You've hit your session limit")}),
            ),
            Line::AccessLost => (
                "assistant",
                json!({"isApiErrorMessage": true, "error": "oauth_org_not_allowed",
                    "apiErrorStatus": 403, "apiErrorCode": "oauth_not_allowed_for_organization",
                    "message": refusal(&uuid, "Your organization has disabled Claude \
                        subscription access for Claude Code")}),
            ),
            Line::GoalPause => (
                "system",
                json!({"subtype": "informational", "isMeta": false, "level": "notice", "content":
                    "Goal paused \u{b7} the request was rate limited \u{b7} send a message to retry"}),
            ),
            Line::TurnDuration => (
                "system",
                json!({"subtype": "turn_duration", "durationMs": 250, "isMeta": false}),
            ),
            Line::OpusCapRefusal => (
                "assistant",
                json!({"isApiErrorMessage": true, "error": "rate_limit",
                    "apiError": "usage_limit_reached", "apiErrorStatus": 429,
                    "apiErrorParams": {"rate_limit_info":
                        {"status": "rejected", "rateLimitType": "seven_day_opus"}},
                    "message": refusal(&uuid, "You've hit your Opus limit")}),
            ),
            Line::RequestOfFable => (
                "api-request",
                json!({"model": "claude-fable-5-2", "max_tokens": 64_000}),
            ),
            Line::Work => (
                "assistant",
                json!({"requestId": format!("req-{uuid}"), "message": {"id": format!("msg-{uuid}"),
                    "type": "message", "role": "assistant", "model": "fake-model",
                    "content": [{"type": "tool_use", "id": format!("toolu-{uuid}"),
                        "name": "Bash", "input": {"command": "true"}}],
                    "stop_reason": "tool_use",
                    "usage": {"input_tokens": 8, "cache_creation_input_tokens": 0,
                        "cache_read_input_tokens": 0, "output_tokens": 6}}}),
            ),
        };
        if matches!(line, Line::RequestOfFable) {
            // Not a message of the conversation: it has no place in the chain of records.
            let record = json!({"type": kind, "sessionId": session, "id": uuid,
                "timestamp": at, "version": "2.1.325", "querySource": "repl_main_thread",
                "params": extra});
            out.push_str(&format!("{record}\n"));
            continue;
        }
        let mut record = json!({"type": kind, "uuid": uuid, "parentUuid": parent,
            "isSidechain": false, "sessionId": session, "timestamp": at,
            "userType": "external", "version": "2.1.311"});
        if let (Some(map), Some(more)) = (record.as_object_mut(), extra.as_object()) {
            map.extend(more.clone());
        }
        out.push_str(&format!("{record}\n"));
        parent = uuid;
    }
    std::fs::OpenOptions::new()
        .append(true)
        .open(transcript)?
        .write_all(out.as_bytes())?;
    Ok(())
}
