//! Claude Code's scheduled hand-over at UserPromptSubmit (SPEC-541).

use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};

use serde::Deserialize;
use yulius_domain::harness::SubmitOrigin;
use yulius_domain::time::{Timestamp, parse_rfc3339};

/// The submit hook is on the agent's critical path. Read only complete lines in the last
/// 4 MiB, the watcher's single-record budget. The caller has opened the task's native file.
///
/// # Errors
/// Returns the error from inspecting or reading the opened transcript file.
pub fn submit_origin(
    mut file: File,
    session: &str,
    prompt_id: &str,
    prompt: &str,
    sent: Timestamp,
) -> io::Result<SubmitOrigin> {
    let len = file.metadata()?.len();
    let start = len.saturating_sub(crate::tail::DEFAULT_MAX_LINE_BYTES as u64);
    file.seek(SeekFrom::Start(start))?;
    let mut bytes = Vec::new();
    file.take(len - start).read_to_end(&mut bytes)?;
    let mut lines = bytes.split(|&byte| byte == b'\n');
    // The first line of a clipped tail and the last line of an unfinished write are partial.
    if start != 0 {
        lines.next();
    }
    lines.next_back();
    Ok(origin_in(
        lines,
        session,
        prompt_id,
        prompt,
        sent,
        start != 0,
    ))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Record<'a> {
    #[serde(rename = "type")]
    kind: &'a str,
    session_id: Option<&'a str>,
    timestamp: Option<&'a str>,
    subtype: Option<&'a str>,
    operation: Option<&'a str>,
    // Enqueue content may contain JSON escapes, or (on another record) be an object.
    content: Option<serde_json::Value>,
    prompt: Option<String>,
    uuid: Option<&'a str>,
    is_sidechain: Option<bool>,
    scheduled_fire_id: Option<&'a str>,
    prompt_id: Option<&'a str>,
    is_meta: Option<bool>,
    prompt_source: Option<&'a str>,
}

fn origin_in<'a>(
    lines: impl Iterator<Item = &'a [u8]>,
    session: &str,
    id: &str,
    prompt: &str,
    sent: Timestamp,
    clipped: bool,
) -> SubmitOrigin {
    // The file can contain enqueue, dequeue, THEN fire. Their provider timestamps give the
    // causal order. A fire's display prompt can be folded or shortened. The enqueue carries
    // the complete text. Pair the fire with its next enqueue, never a later same-text entry.
    let preview = prompt.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut events = Vec::new();
    let mut finished = std::collections::HashSet::new();
    for line in lines {
        let Ok(r) = serde_json::from_slice::<Record<'_>>(line) else {
            continue;
        };
        if r.is_sidechain == Some(true) || r.session_id != Some(session) {
            continue;
        }
        if r.kind == "user" {
            if r.prompt_id == Some(id) && r.prompt_source.is_some() {
                return if r.is_meta == Some(true) && r.prompt_source == Some("system") {
                    SubmitOrigin::Scheduled
                } else {
                    SubmitOrigin::Unmarked
                };
            }
            if let Some(fire) = r.scheduled_fire_id {
                finished.insert(fire);
            }
        }
        let Some(at) = r.timestamp.and_then(parse_rfc3339).filter(|at| *at <= sent) else {
            continue;
        };
        let event = match (r.kind, r.subtype, r.operation) {
            ("system", Some("scheduled_task_fire"), _) => r.uuid.map(|uuid| {
                // The fire holds a display preview (at most 200 UTF-16 units); the enqueue
                // is the exact comparison. Keep unrelated fires in order too.
                Event::Fire(
                    uuid,
                    r.prompt.as_deref().is_some_and(|p| preview.starts_with(p)),
                )
            }),
            ("queue-operation", _, Some("enqueue")) => {
                let text = r.content.as_ref().and_then(serde_json::Value::as_str);
                Some(Event::Enqueue(text.map(str::to_owned)))
            }
            // The model or the handed-on prompt has reached the file. A later, independently
            // typed submit is no longer at this hand-over, even when the earlier enqueue was clipped.
            ("assistant", _, _) | ("user", _, _)
                if r.kind == "assistant" || r.prompt_source.is_some() =>
            {
                Some(Event::Turn)
            }
            ("queue-operation", _, Some("dequeue")) => Some(Event::Dequeue),
            ("queue-operation", _, Some("remove" | "popOne" | "popAll")) => Some(Event::Removal(
                r.content
                    .as_ref()
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_owned),
            )),
            _ => None,
        };
        if let Some(event) = event {
            events.push((at, event));
        }
    }
    hand_over(events, &finished, prompt, clipped)
}

fn hand_over(
    mut events: Vec<(Timestamp, Event<'_>)>,
    finished: &std::collections::HashSet<&str>,
    prompt: &str,
    clipped: bool,
) -> SubmitOrigin {
    events.sort_by_key(|(at, event)| (*at, event.order()));
    let mut fires = std::collections::VecDeque::new();
    let mut waiting = std::collections::VecDeque::new();
    let mut handed_on = SubmitOrigin::Unmarked;
    for (_, event) in events {
        match event {
            Event::Fire(uuid, matches) => fires.push_back((uuid, matches)),
            Event::Enqueue(text) => {
                let scheduled = fires
                    .pop_front()
                    .is_some_and(|(uuid, matches)| matches && !finished.contains(uuid));
                waiting.push_back((text, scheduled));
            }
            Event::Dequeue => {
                match waiting.pop_front() {
                    Some((Some(text), true)) if text == prompt => {
                        handed_on = SubmitOrigin::Scheduled
                    }
                    // Claude can leave a background notice counted forever (SPEC-493).
                    Some((Some(text), _))
                        if text.trim_start().starts_with("<task-notification>")
                            && waiting.iter().any(|(text, scheduled)| {
                                text.as_deref() == Some(prompt) && *scheduled
                            })
                            && handed_on == SubmitOrigin::Unmarked =>
                    {
                        handed_on = SubmitOrigin::UnidentifiedHandOver;
                    }
                    None if clipped && handed_on == SubmitOrigin::Unmarked => {
                        handed_on = SubmitOrigin::UnidentifiedHandOver;
                    }
                    _ => {}
                }
            }
            Event::Turn => handed_on = SubmitOrigin::Unmarked,
            Event::Removal(removed) => {
                if let Some(i) = waiting
                    .iter()
                    .position(|(text, _)| removed.is_none() || text == &removed)
                {
                    waiting.remove(i);
                }
            }
        }
    }
    handed_on
}

enum Event<'a> {
    Fire(&'a str, bool),
    Enqueue(Option<String>),
    Turn,
    Dequeue,
    Removal(Option<String>),
}

impl Event<'_> {
    fn order(&self) -> u8 {
        match self {
            Self::Fire(..) => 0,
            Self::Enqueue(_) => 1,
            Self::Dequeue | Self::Removal(_) => 2,
            Self::Turn => 3,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn classified(records: &[serde_json::Value], id: &str, prompt: &str, at: i64) -> bool {
        let lines = records.iter().map(ToString::to_string).collect::<Vec<_>>();
        origin_in(
            lines.iter().map(String::as_bytes),
            "s",
            id,
            prompt,
            Timestamp(at),
            false,
        ) == SubmitOrigin::Scheduled
    }

    fn record(kind: &str, millis: i64, fields: serde_json::Value) -> serde_json::Value {
        let mut record = fields;
        record["type"] = kind.into();
        record["sessionId"] = "s".into();
        record["timestamp"] = yulius_domain::time::format_rfc3339(Timestamp(millis)).into();
        record
    }

    #[test]
    fn a_scheduled_entry_waiting_behind_another_prompt_has_no_submit_yet() {
        use serde_json::json;
        let text = "one\ntwo".repeat(120);
        let mut records = vec![
            record(
                "queue-operation",
                1,
                json!({"operation":"enqueue","content":"ordinary"}),
            ),
            record(
                "queue-operation",
                3,
                json!({"operation":"enqueue","content":text}),
            ),
            record(
                "system",
                2,
                json!({"subtype":"scheduled_task_fire","uuid":"fire","prompt":"one twoone two"}),
            ),
            record("queue-operation", 4, json!({"operation":"dequeue"})),
        ];
        assert!(!classified(&records, "p", &text, 4));
        records.push(record("queue-operation", 5, json!({"operation":"dequeue"})));
        assert!(
            !classified(&records, "p", &text, 4),
            "the hand-over followed this hook"
        );
        assert!(classified(&records, "p", &text, 5));
        assert!(!classified(&records, "p", "ordinary", 5));
        records.push(record(
            "user",
            6,
            json!({"isMeta":true,"promptSource":"system","promptId":"p","scheduledFireId":"fire"}),
        ));
        assert!(
            classified(&records, "p", &text, 7),
            "replayed scheduled hook"
        );
        assert!(!classified(&records, "another", &text, 7), "retired fire");
    }

    #[test]
    fn a_withdrawn_schedule_cannot_claim_a_later_same_text_submit() {
        use serde_json::json;
        let records = [
            record(
                "system",
                0,
                json!({"subtype":"scheduled_task_fire","uuid":"fire","prompt":"check"}),
            ),
            record(
                "queue-operation",
                1,
                json!({"operation":"enqueue","content":"check"}),
            ),
            record(
                "queue-operation",
                2,
                json!({"operation":"popAll","content":"check"}),
            ),
            record(
                "queue-operation",
                3,
                json!({"operation":"enqueue","content":"check"}),
            ),
            record("queue-operation", 4, json!({"operation":"dequeue"})),
        ];
        assert!(!classified(&records, "typed", "check", 5));
    }
    #[test]
    fn a_notice_claude_never_takes_out_does_not_hide_a_scheduled_hand_over() {
        use serde_json::json;
        let records = [
            record(
                "queue-operation",
                0,
                json!({"operation":"enqueue","content":"<task-notification>old</task-notification>"}),
            ),
            record(
                "system",
                10,
                json!({"subtype":"scheduled_task_fire","uuid":"fire","prompt":"check"}),
            ),
            record(
                "queue-operation",
                11,
                json!({"operation":"enqueue","content":"check"}),
            ),
            record("queue-operation", 12, json!({"operation":"dequeue"})),
        ];
        let lines = records.iter().map(ToString::to_string).collect::<Vec<_>>();
        assert_eq!(
            origin_in(
                lines.iter().map(String::as_bytes),
                "s",
                "p",
                "check",
                Timestamp(13),
                false
            ),
            SubmitOrigin::UnidentifiedHandOver
        );
        assert!(!classified(&records, "p", "other", 13));
    }

    #[test]
    fn a_hand_over_with_its_fire_outside_the_tail_preserves_personal_state() -> io::Result<()> {
        use serde_json::json;
        use std::io::Write as _;
        let mut file = tempfile::tempfile()?;
        let write =
            |file: &mut File, kind, at, fields| writeln!(file, "{}", record(kind, at, fields));
        write(
            &mut file,
            "system",
            1,
            json!({"subtype":"scheduled_task_fire","uuid":"fire","prompt":"check"}),
        )?;
        write(
            &mut file,
            "queue-operation",
            2,
            json!({"operation":"enqueue","content":"check"}),
        )?;
        write(
            &mut file,
            "api-request-blob",
            3,
            json!({"data":"x".repeat(crate::tail::DEFAULT_MAX_LINE_BYTES)}),
        )?;
        write(
            &mut file,
            "queue-operation",
            4,
            json!({"operation":"dequeue"}),
        )?;
        assert_eq!(
            submit_origin(file.try_clone()?, "s", "p", "check", Timestamp(5))?,
            SubmitOrigin::UnidentifiedHandOver
        );
        write(
            &mut file,
            "user",
            6,
            json!({"promptId":"p","isMeta":true,"promptSource":"system","scheduledFireId":"fire"}),
        )?;
        assert_eq!(
            submit_origin(file, "s", "other", "check", Timestamp(7))?,
            SubmitOrigin::Unmarked
        );
        Ok(())
    }
}
