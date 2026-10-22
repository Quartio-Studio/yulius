//! Stepping a Claude Code transcript back over a usage limit (SPEC-034 LIM-001).
//!
//! When the account's usage limit rejects a request, Claude Code 2.1.292 appends an error record
//! (`isApiErrorMessage`, `apiError: "usage_limit_reached"`) and `system` notices ("Usage limit
//! reached · continuing automatically at 7:40pm", "Goal paused · usage limit reached", the turn's
//! `turn_duration`). Every message that arrives before the reset gets its own error. The step back
//! removes those records from the failed tail (everything after the last successful assistant
//! record) and keeps what was lost: the unanswered prompts, task notifications and tool results.
//! A resume with Claude Code's interrupted-turn flag then redoes exactly that work, as if the limit
//! had never happened.
//!
//! Records that pointed at a removed record (`parentUuid`, `logicalParentUuid`) are relinked to its
//! nearest kept ancestor, because Claude Code rebuilds the conversation by walking that chain from
//! the newest record. Every other line, unparsable ones included, is kept byte for byte.

use std::collections::HashMap;
use std::io;
use std::path::Path;

use serde_json::{Map, Value};

use crate::claude::observe_line;
use crate::json::{bool_of, parse_object, str_of};
use crate::launch::PlannedFile;
use crate::observe::Observed;

/// The record fields that link a record to an earlier one.
const LINKS: [&str; 2] = ["parentUuid", "logicalParentUuid"];

/// One transcript line and what the step back needs to know about it.
struct Line<'a> {
    raw: &'a str,
    record: Option<Map<String, Value>>,
}

impl Line<'_> {
    fn kind(&self) -> Option<&str> {
        self.record.as_ref().and_then(|r| str_of(r, "type"))
    }

    fn sidechain(&self) -> bool {
        self.record
            .as_ref()
            .is_some_and(|r| bool_of(r, "isSidechain"))
    }

    /// An assistant record the API answered (not an error Claude Code wrote locally).
    fn answered(&self) -> bool {
        self.kind() == Some("assistant")
            && !self.sidechain()
            && self.record.as_ref().is_some_and(|r| {
                !bool_of(r, "isApiErrorMessage") && r.get("error").is_none_or(Value::is_null)
            })
    }

    /// The error record of a usage limit, recognised like the transcript observer does.
    fn limit(&self) -> bool {
        self.kind() == Some("assistant")
            && !self.sidechain()
            && !self.answered()
            && observe_line(self.raw)
                .iter()
                .any(|o| matches!(o, Observed::LimitReached { .. }))
    }

    fn uuid(&self) -> Option<&str> {
        self.record.as_ref().and_then(|r| str_of(r, "uuid"))
    }
}

/// The transcript `text` stepped back over the usage limit that ended it; `None` when its tail
/// holds no usage-limit record (nothing to change), or when a line it cannot read names a record it
/// would remove (relinking it is impossible, and a dangling link would cut the history).
#[must_use]
pub fn step_back(text: &str) -> Option<String> {
    let raws: Vec<&str> = text.split_inclusive('\n').collect();
    // Only the tail after the last answered assistant record can change: parse just that.
    let mut tail: Vec<Line<'_>> = Vec::new();
    for raw in raws.iter().rev() {
        let line = Line {
            raw,
            record: parse_object(raw),
        };
        if line.answered() {
            break;
        }
        tail.push(line);
    }
    tail.reverse();
    let first = tail.iter().position(Line::limit)?;
    // Removed record → its own parent (`None` for the root).
    let mut removed: HashMap<String, Option<String>> = HashMap::new();
    let mut drop = vec![false; tail.len()];
    for (i, line) in tail.iter().enumerate().skip(first) {
        // A compaction boundary of any kind is history, not a notice about the limit.
        let notice = line.kind() == Some("system")
            && !line.sidechain()
            && !line
                .record
                .as_ref()
                .and_then(|r| str_of(r, "subtype"))
                .is_some_and(|subtype| subtype.ends_with("_boundary"));
        if notice || line.limit() {
            drop[i] = true;
            if let (Some(uuid), Some(record)) = (line.uuid(), &line.record) {
                let parent = str_of(record, "parentUuid").map(str::to_owned);
                removed.insert(uuid.to_owned(), parent);
            }
        }
    }
    if tail
        .iter()
        .any(|l| l.record.is_none() && removed.keys().any(|id| l.raw.contains(id.as_str())))
    {
        return None;
    }
    let resolve = |mut id: String| -> Option<String> {
        // A chain is at most as long as the transcript; a cycle ends the walk.
        for _ in 0..=removed.len() {
            match removed.get(&id) {
                Some(Some(parent)) => id = parent.clone(),
                Some(None) => return None,
                None => return Some(id),
            }
        }
        None
    };
    let mut out = raws[..raws.len() - tail.len()].concat();
    for (i, line) in tail.iter().enumerate() {
        if drop[i] {
            continue;
        }
        let Some(record) = &line.record else {
            out.push_str(line.raw);
            continue;
        };
        let stale: Vec<&str> = LINKS
            .into_iter()
            .filter(|key| str_of(record, key).is_some_and(|id| removed.contains_key(id)))
            .collect();
        if stale.is_empty() {
            out.push_str(line.raw);
            continue;
        }
        let mut record = record.clone();
        for key in stale {
            let target = str_of(&record, key).map(str::to_owned).and_then(resolve);
            record.insert(key.to_owned(), target.map_or(Value::Null, Value::String));
        }
        out.push_str(&Value::Object(record).to_string());
        if line.raw.ends_with('\n') {
            out.push('\n');
        }
    }
    Some(out)
}

/// Steps the transcript at `path` back over a usage limit ([`step_back`]), replacing it
/// atomically with the same permissions. Returns whether it changed; a missing file is unchanged.
///
/// # Errors
/// I/O failures, or a transcript that is not UTF-8.
pub fn step_back_file(path: &Path) -> io::Result<bool> {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(err) => return Err(err),
    };
    let text =
        String::from_utf8(bytes).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    let Some(stepped) = step_back(&text) else {
        return Ok(false);
    };
    #[cfg(unix)]
    let mode = {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::metadata(path)?.permissions().mode() & 0o7777
    };
    #[cfg(not(unix))]
    let mode = 0o600;
    PlannedFile {
        path: path.to_path_buf(),
        contents: stepped.into_bytes(),
        mode,
    }
    .write()?;
    // The new file was created under the umask: give it exactly the old permissions.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))?;
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use serde_json::{Value, json};

    use super::{step_back, step_back_file};

    /// A record of the synthetic transcript: `uuid` follows `parent`.
    fn record(kind: &str, uuid: &str, parent: Option<&str>, extra: &Value) -> String {
        let mut record = json!({"type": kind, "uuid": uuid, "parentUuid": parent,
            "isSidechain": false, "sessionId": "s1", "timestamp": "2026-10-12T15:39:29.249Z"});
        if let (Some(map), Some(more)) = (record.as_object_mut(), extra.as_object()) {
            map.extend(more.clone());
        }
        format!("{record}\n")
    }

    fn prompt(uuid: &str, parent: &str, text: &str) -> String {
        record(
            "user",
            uuid,
            Some(parent),
            &json!({"message": {"role": "user", "content": text},
            "origin": {"kind": "human"}}),
        )
    }

    fn answer(uuid: &str, parent: &str) -> String {
        record(
            "assistant",
            uuid,
            Some(parent),
            &json!({"message": {"id": format!("msg_{uuid}"),
            "role": "assistant", "model": "claude-opus-5-5", "stop_reason": "end_turn",
            "content": [{"type": "text", "text": "done"}]}}),
        )
    }

    fn tool_use(uuid: &str, parent: &str) -> String {
        record(
            "assistant",
            uuid,
            Some(parent),
            &json!({"message": {"id": format!("msg_{uuid}"),
            "role": "assistant", "model": "claude-opus-5-5", "stop_reason": "tool_use",
            "content": [{"type": "tool_use", "id": "toolu_1", "name": "Bash", "input": {}}]}}),
        )
    }

    fn tool_result(uuid: &str, parent: &str) -> String {
        record(
            "user",
            uuid,
            Some(parent),
            &json!({"message": {"role": "user",
            "content": [{"type": "tool_result", "tool_use_id": "toolu_1", "content": "ok"}]}}),
        )
    }

    /// Claude Code 2.1.292's usage-limit error record.
    fn limit(uuid: &str, parent: &str) -> String {
        record(
            "assistant",
            uuid,
            Some(parent),
            &json!({"isApiErrorMessage": true,
            "error": "rate_limit", "apiError": "usage_limit_reached", "apiErrorStatus": 429,
            "apiErrorParams": {"rate_limit_info": {"status": "rejected", "resetsAt": 1_760_300_000,
                "rateLimitType": "five_hour"}},
            "message": {"id": format!("msg_{uuid}"), "role": "assistant", "model": "<synthetic>",
                "content": [{"type": "text",
                    "text": "You've hit your session limit · resets 7:40pm (Europe/Berlin)"}]}}),
        )
    }

    fn system(uuid: &str, parent: &str, subtype: &str, content: &str) -> String {
        record(
            "system",
            uuid,
            Some(parent),
            &json!({"subtype": subtype, "content": content}),
        )
    }

    fn attachment(uuid: &str, parent: &str) -> String {
        record(
            "attachment",
            uuid,
            Some(parent),
            &json!({"attachment": {"type": "hook_additional_context", "content": ["ctx"]}}),
        )
    }

    /// The records and links of a transcript: `(uuid, parentUuid)` per parsed line.
    fn chain(text: &str) -> Vec<(String, Option<String>)> {
        text.lines()
            .filter_map(|l| serde_json::from_str::<Value>(l).ok())
            .map(|v| {
                (
                    v["uuid"].as_str().unwrap_or("-").to_owned(),
                    v["parentUuid"].as_str().map(str::to_owned),
                )
            })
            .collect()
    }

    fn uuids(text: &str) -> Vec<String> {
        chain(text).into_iter().map(|(u, _)| u).collect()
    }

    /// Task #25 (2026-10-12): a prompt, then a task notification, each answered by the limit;
    /// the step back keeps both messages, drops the errors and Claude Code's notices, and the
    /// chain from the newest record reaches the last answered turn.
    #[test]
    fn step_back_keeps_the_unanswered_messages_of_task_25() -> Result<(), Box<dyn std::error::Error>>
    {
        let text = [
            prompt("u1", "root", "first"),
            answer("a1", "u1"),
            system("s1", "a1", "turn_duration", ""),
            "{\"type\":\"file-history-snapshot\",\"messageId\":\"m\"}\n".to_owned(),
            prompt("u2", "s1", "Yulius: you have new messages."),
            attachment("t2", "u2"),
            limit("e2", "t2"),
            system(
                "n1",
                "e2",
                "informational",
                "Usage limit reached · continuing automatically at 7:40pm",
            ),
            system(
                "n2",
                "n1",
                "informational",
                "Goal paused · usage limit reached",
            ),
            system("d2", "n2", "turn_duration", ""),
            "{\"type\":\"queue-operation\",\"operation\":\"enqueue\"}\n".to_owned(),
            prompt("u3", "d2", "<task-notification>done</task-notification>"),
            limit("e3", "u3"),
            system("d3", "e3", "turn_duration", ""),
        ]
        .concat();
        let stepped = step_back(&text).ok_or("a limit tail")?;
        assert_eq!(
            uuids(&stepped),
            ["u1", "a1", "s1", "-", "u2", "t2", "-", "u3"],
            "{stepped}"
        );
        let links: Vec<_> = chain(&stepped);
        assert_eq!(
            links[7].1.as_deref(),
            Some("t2"),
            "u3 relinked past the notices"
        );
        assert!(!stepped.contains("usage_limit_reached") && !stepped.contains("Usage limit"));
        // Kept lines are byte for byte the same.
        assert!(stepped.contains(&prompt("u2", "s1", "Yulius: you have new messages.")));
        assert!(stepped.contains(&attachment("t2", "u2")));
        assert_eq!(
            step_back(&stepped),
            None,
            "stepping back twice changes nothing"
        );
        Ok(())
    }

    /// Most limits in real sessions hit mid-turn, after a tool result: the tool work stays, so
    /// Claude Code continues the turn instead of redoing it.
    #[test]
    fn step_back_mid_turn_keeps_the_tool_work() -> Result<(), Box<dyn std::error::Error>> {
        let text = [
            prompt("u1", "root", "work"),
            tool_use("a1", "u1"),
            tool_result("r1", "a1"),
            attachment("t1", "r1"),
            limit("e1", "t1"),
            system("n1", "e1", "informational", "Usage limit reached"),
            system("d1", "n1", "turn_duration", ""),
            attachment("h1", "d1"),
        ]
        .concat();
        let stepped = step_back(&text).ok_or("a limit tail")?;
        let links = chain(&stepped);
        assert_eq!(uuids(&stepped), ["u1", "a1", "r1", "t1", "h1"]);
        assert_eq!(links[4].1.as_deref(), Some("t1"));
        assert!(stepped.ends_with('\n'));
        Ok(())
    }

    /// The fake agent's shape: the limit after a completed turn. Only the error goes.
    #[test]
    fn step_back_after_a_completed_turn_removes_only_the_limit()
    -> Result<(), Box<dyn std::error::Error>> {
        let text = [
            prompt("u1", "root", "hello"),
            answer("a1", "u1"),
            limit("e1", "a1"),
        ]
        .concat();
        let stepped = step_back(&text).ok_or("a limit tail")?;
        assert_eq!(
            stepped,
            [prompt("u1", "root", "hello"), answer("a1", "u1")].concat()
        );
        Ok(())
    }

    /// No limit, a limit that was already answered after it, or only another API error: nothing
    /// to step back. Notices before the failed tail and compaction boundaries stay.
    #[test]
    fn step_back_leaves_other_transcripts_alone() -> Result<(), Box<dyn std::error::Error>> {
        let plain = [prompt("u1", "root", "hello"), answer("a1", "u1")].concat();
        assert_eq!(step_back(&plain), None);
        let answered_later = [
            prompt("u1", "root", "hello"),
            limit("e1", "u1"),
            prompt("u2", "e1", "Continue."),
            answer("a2", "u2"),
        ]
        .concat();
        assert_eq!(step_back(&answered_later), None, "the limit is history now");
        let overloaded = [
            prompt("u1", "root", "hello"),
            record(
                "assistant",
                "e1",
                Some("u1"),
                &json!({"isApiErrorMessage": true,
                "error": "overloaded", "message": {"role": "assistant", "model": "<synthetic>",
                "content": [{"type": "text", "text": "API Error: 529 overloaded"}]}}),
            ),
        ]
        .concat();
        assert_eq!(step_back(&overloaded), None, "not a usage limit");
        let compacted = [
            prompt("u1", "root", "hello"),
            limit("e1", "u1"),
            system("c1", "e1", "compact_boundary", "Conversation compacted"),
        ]
        .concat();
        let stepped = step_back(&compacted).ok_or("a limit tail")?;
        assert_eq!(uuids(&stepped), ["u1", "c1"]);
        assert_eq!(chain(&stepped)[1].1.as_deref(), Some("u1"));
        Ok(())
    }

    /// An older CLI that wrote only the limit text, a root that is removed, unparsable and blank
    /// lines, and `logicalParentUuid`.
    #[test]
    fn step_back_handles_odd_lines_and_links() -> Result<(), Box<dyn std::error::Error>> {
        let text_only = record(
            "assistant",
            "e0",
            None,
            &json!({"isApiErrorMessage": true,
            "message": {"role": "assistant", "model": "<synthetic>",
                "content": [{"type": "text", "text": "Claude AI usage limit reached|1760000000"}]}}),
        );
        let logical = record(
            "user",
            "u1",
            Some("e0"),
            &json!({"logicalParentUuid": "e0",
            "message": {"role": "user", "content": "again"}}),
        );
        let text = [text_only, "not json\n".to_owned(), "\n".to_owned(), logical].concat();
        let stepped = step_back(&text).ok_or("a limit tail")?;
        let mut lines = stepped.lines();
        assert_eq!(lines.next(), Some("not json"));
        assert_eq!(lines.next(), Some(""));
        let relinked: Value = serde_json::from_str(lines.next().ok_or("the prompt")?)?;
        assert_eq!(relinked["parentUuid"], Value::Null);
        assert_eq!(relinked["logicalParentUuid"], Value::Null);
        assert_eq!(relinked["message"]["content"], "again");
        Ok(())
    }

    /// Review of SPEC-034: a line serde cannot read that names a removed record cannot be
    /// relinked, so the transcript is left as it is rather than cutting the resumed history.
    #[test]
    fn step_back_never_strands_an_unreadable_line() {
        let text = [
            prompt("u1", "root", "hello"),
            limit("e1", "u1"),
            system("n1", "e1", "informational", "Usage limit reached"),
            // A lone surrogate escape (a split emoji from Node's JSON.stringify) after n1.
            "{\"type\":\"user\",\"uuid\":\"u2\",\"parentUuid\":\"n1\",\"message\":{\"role\":\"user\",\"content\":\"\\ud83d\"}}\n".to_owned(),
        ]
        .concat();
        assert_eq!(step_back(&text), None);
    }

    /// Every compaction kind of boundary is history (`compact_boundary`,
    /// `microcompact_boundary`, …), not a notice about the limit.
    #[test]
    fn step_back_keeps_every_boundary() -> Result<(), Box<dyn std::error::Error>> {
        let text = [
            prompt("u1", "root", "hello"),
            limit("e1", "u1"),
            system("m1", "e1", "microcompact_boundary", ""),
        ]
        .concat();
        let stepped = step_back(&text).ok_or("a limit tail")?;
        assert_eq!(uuids(&stepped), ["u1", "m1"]);
        Ok(())
    }

    /// `<sandbox>/home/.claude/projects/<slug>/s1.jsonl`, the layout `step_back_file` expects.
    fn transcript_in(sandbox: &Path) -> PathBuf {
        sandbox.join("home/.claude/projects/-wt/s1.jsonl")
    }

    fn limited() -> String {
        [
            prompt("u1", "root", "hello"),
            answer("a1", "u1"),
            limit("e1", "a1"),
        ]
        .concat()
    }

    fn mode(path: &Path) -> std::io::Result<u32> {
        use std::os::unix::fs::PermissionsExt as _;
        Ok(std::fs::metadata(path)?.permissions().mode() & 0o7777)
    }

    #[test]
    fn step_back_file_rewrites_only_when_needed() -> Result<(), Box<dyn std::error::Error>> {
        use std::os::unix::fs::PermissionsExt as _;
        let sandbox = tempfile::tempdir()?;
        let path = transcript_in(sandbox.path());
        assert!(
            !step_back_file(&path)?,
            "a missing transcript (and folder) is left alone"
        );
        let folder = path.parent().ok_or("a folder")?;
        std::fs::create_dir_all(folder)?;
        assert!(!step_back_file(&path)?, "a missing transcript is left alone");
        let plain = [prompt("u1", "root", "hello"), answer("a1", "u1")].concat();
        std::fs::write(&path, &plain)?;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
        assert!(!step_back_file(&path)?);
        std::fs::write(&path, limited())?;
        assert!(step_back_file(&path)?);
        assert_eq!(std::fs::read_to_string(&path)?, plain);
        assert_eq!(mode(&path)?, 0o600);
        // A mode other than the temporary file's own is carried over too (SPEC-266 STB-005).
        std::fs::write(&path, limited())?;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o640))?;
        assert!(step_back_file(&path)?);
        assert_eq!(std::fs::read_to_string(&path)?, plain);
        assert_eq!(mode(&path)?, 0o640);
        assert_eq!(
            std::fs::read_dir(folder)?.count(),
            1,
            "no temporary file left"
        );
        Ok(())
    }

    /// #661: a link the agent put on the way to the transcript (its project folder, its home)
    /// fails the step back, naming it, and the link's target is neither read nor replaced.
    #[test]
    fn step_back_file_follows_no_linked_folder() -> Result<(), Box<dyn std::error::Error>> {
        use std::os::unix::fs::symlink;
        for linked in ["home/.claude/projects/-wt", "home"] {
            let sandbox = tempfile::tempdir()?;
            let outside = tempfile::tempdir()?;
            let path = transcript_in(sandbox.path());
            // The host folder holds what the link would make the step back see.
            let rest = path.strip_prefix(sandbox.path().join(linked))?;
            let host_file = outside.path().join(rest);
            std::fs::create_dir_all(host_file.parent().ok_or("a folder")?)?;
            std::fs::write(&host_file, limited())?;
            let link = sandbox.path().join(linked);
            std::fs::create_dir_all(link.parent().ok_or("a folder")?)?;
            symlink(outside.path(), &link)?;
            let err = step_back_file(&path).err().ok_or(format!("{linked}: refused"))?;
            assert!(err.to_string().contains("link"), "{linked}: {err}");
            assert_eq!(std::fs::read_to_string(&host_file)?, limited(), "{linked}");
            assert_eq!(
                std::fs::read_dir(host_file.parent().ok_or("a folder")?)?.count(),
                1,
                "{linked}: nothing written next to the host file"
            );
        }
        Ok(())
    }

    /// #661: a link at the transcript is never read nor renamed over; its target stays as it was.
    #[test]
    fn step_back_file_follows_no_linked_transcript() -> Result<(), Box<dyn std::error::Error>> {
        use std::os::unix::fs::symlink;
        let sandbox = tempfile::tempdir()?;
        let outside = tempfile::tempdir()?;
        let host_file = outside.path().join("host.jsonl");
        std::fs::write(&host_file, limited())?;
        let path = transcript_in(sandbox.path());
        std::fs::create_dir_all(path.parent().ok_or("a folder")?)?;
        symlink(&host_file, &path)?;
        let err = step_back_file(&path).err().ok_or("refused")?;
        assert!(err.to_string().contains("link"), "{err}");
        assert!(path.symlink_metadata()?.file_type().is_symlink(), "the link stays");
        assert_eq!(std::fs::read_to_string(&host_file)?, limited());
        Ok(())
    }

    /// #661: a FIFO at the transcript fails at once instead of holding the blocking task (and the
    /// run start awaiting it) forever.
    #[test]
    fn step_back_file_returns_at_once_on_a_fifo() -> Result<(), Box<dyn std::error::Error>> {
        let sandbox = tempfile::tempdir()?;
        let path = transcript_in(sandbox.path());
        std::fs::create_dir_all(path.parent().ok_or("a folder")?)?;
        nix::unistd::mkfifo(&path, nix::sys::stat::Mode::S_IRUSR | nix::sys::stat::Mode::S_IWUSR)?;
        let (done, finished) = std::sync::mpsc::channel();
        let fifo = path.clone();
        // A bound on a blocked open, not a wait for work: the call returns in microseconds.
        std::thread::spawn(move || done.send(step_back_file(&fifo).map_err(|e| e.to_string())));
        let result = finished.recv_timeout(std::time::Duration::from_secs(30))?;
        let err = result.err().ok_or("refused")?;
        assert!(err.contains("not a regular file"), "{err}");
        Ok(())
    }
}
