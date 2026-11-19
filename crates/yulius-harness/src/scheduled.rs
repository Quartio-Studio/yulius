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
    run_to_submit: std::ops::RangeInclusive<Timestamp>,
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
        run_to_submit,
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
    run_to_submit: std::ops::RangeInclusive<Timestamp>,
    clipped: bool,
) -> SubmitOrigin {
    // The file can contain enqueue, dequeue, THEN fire. Their provider timestamps give the
    // causal order. A fire can display a loop name in place of the expanded prompt. The enqueue carries
    // the complete text. Pair the fire with its next enqueue, never a later same-text entry.
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
        let Some(at) = r
            .timestamp
            .and_then(parse_rfc3339)
            .filter(|at| run_to_submit.contains(at))
        else {
            continue;
        };
        let event = match (r.kind, r.subtype, r.operation) {
            ("system", Some("scheduled_task_fire"), _) => r.uuid.map(Event::Fire),
            ("queue-operation", _, Some("enqueue")) => {
                let text = r.content.as_ref().and_then(serde_json::Value::as_str);
                Some(Event::Enqueue(text.map(str::to_owned)))
            }
            // The model or the handed-on prompt has reached the file. A later, independently
            // typed submit is no longer at this hand-over, even when the earlier enqueue was clipped.
            ("assistant", _, _) | ("user", _, _)
                if r.kind == "assistant" || r.prompt_source.is_some() =>
            {
                Some(Event::Turn {
                    fire: r.scheduled_fire_id,
                    source: r.prompt_source,
                })
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
    let mut ambiguous = 0usize;
    let is_notice = |text: &Option<String>| {
        text.as_deref()
            .is_some_and(|text| text.trim_start().starts_with("<task-notification>"))
    };
    for (_, event) in events {
        match event {
            Event::Fire(uuid) => fires.push_back(uuid),
            Event::Enqueue(text) => {
                waiting.push_back((text, fires.pop_front()));
            }
            Event::Dequeue => {
                // A dequeue has no text. If Claude might have skipped a stranded notice,
                // keep both entries until its user record identifies which actually ran.
                let mut plain = waiting
                    .iter()
                    .filter(|(text, _)| !is_notice(text))
                    .peekable();
                if waiting.front().is_some_and(|(text, _)| is_notice(text))
                    && plain.peek().is_some()
                {
                    ambiguous = ambiguous.saturating_add(1);
                    if handed_on == SubmitOrigin::Unmarked
                        && plain.take(ambiguous).any(|(text, fire)| {
                            text.as_deref() == Some(prompt)
                                && fire.map_or(clipped, |uuid| !finished.contains(uuid))
                        })
                    {
                        handed_on = SubmitOrigin::UnidentifiedHandOver;
                    }
                    continue;
                }
                match waiting.pop_front() {
                    Some((Some(text), Some(fire)))
                        if text == prompt && !finished.contains(fire) =>
                    {
                        handed_on = SubmitOrigin::Scheduled;
                    }
                    Some((Some(text), None))
                        if clipped && text == prompt && handed_on == SubmitOrigin::Unmarked =>
                    {
                        handed_on = SubmitOrigin::UnidentifiedHandOver;
                    }
                    None if clipped && handed_on == SubmitOrigin::Unmarked => {
                        handed_on = SubmitOrigin::UnidentifiedHandOver;
                    }
                    _ => {}
                }
            }
            Event::Turn { fire, source } => {
                if ambiguous > 0 {
                    let entry =
                        waiting
                            .iter()
                            .position(|(text, queued_fire)| match (fire, source) {
                                (Some(id), _) => *queued_fire == Some(id),
                                (None, Some("system")) => is_notice(text),
                                (None, Some("queued")) => !is_notice(text) && queued_fire.is_none(),
                                _ => false,
                            });
                    if let Some(i) = entry {
                        waiting.remove(i);
                        ambiguous -= 1;
                    }
                }
                handed_on = SubmitOrigin::Unmarked;
            }
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
    Fire(&'a str),
    Enqueue(Option<String>),
    Turn {
        fire: Option<&'a str>,
        source: Option<&'a str>,
    },
    Dequeue,
    Removal(Option<String>),
}

impl Event<'_> {
    fn order(&self) -> u8 {
        match self {
            Self::Fire(..) => 0,
            Self::Enqueue(_) => 1,
            Self::Dequeue | Self::Removal(_) => 2,
            Self::Turn { .. } => 3,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn origin_of(records: &[serde_json::Value], id: &str, prompt: &str, at: i64) -> SubmitOrigin {
        let lines = records.iter().map(ToString::to_string).collect::<Vec<_>>();
        origin_in(
            lines.iter().map(String::as_bytes),
            "s",
            id,
            prompt,
            Timestamp(0)..=Timestamp(at),
            false,
        )
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
        assert_eq!(origin_of(&records, "p", &text, 4), SubmitOrigin::Unmarked);
        records.push(record("queue-operation", 5, json!({"operation":"dequeue"})));
        assert_eq!(
            origin_of(&records, "p", &text, 4),
            SubmitOrigin::Unmarked,
            "the hand-over followed this hook"
        );
        assert_eq!(origin_of(&records, "p", &text, 5), SubmitOrigin::Scheduled);
        assert_eq!(
            origin_of(&records, "p", "ordinary", 5),
            SubmitOrigin::Unmarked
        );
        records.push(record(
            "user",
            6,
            json!({"isMeta":true,"promptSource":"system","promptId":"p","scheduledFireId":"fire"}),
        ));
        assert_eq!(
            origin_of(&records, "p", &text, 7),
            SubmitOrigin::Scheduled,
            "replayed scheduled hook"
        );
        assert_eq!(
            origin_of(&records, "another", &text, 7),
            SubmitOrigin::Unmarked,
            "retired fire"
        );
    }

    #[test]
    fn the_fire_display_can_differ_from_the_scheduled_prompt() {
        use serde_json::json;
        for (display, text) in [
            ("/loop (loop.md)", "# /loop tick: check build"),
            ("👩💻", "👩‍💻"),
        ] {
            let records = [
                record(
                    "system",
                    1,
                    json!({"subtype":"scheduled_task_fire","uuid":"fire","prompt":display}),
                ),
                record(
                    "queue-operation",
                    2,
                    json!({"operation":"enqueue","content":text}),
                ),
                record("queue-operation", 3, json!({"operation":"dequeue"})),
            ];
            assert_eq!(
                origin_of(&records, "p", text, 4),
                SubmitOrigin::Scheduled,
                "{display}"
            );
        }
    }

    #[test]
    fn a_resumed_process_starts_with_an_empty_prompt_queue() {
        use serde_json::json;
        let records = [
            record(
                "queue-operation",
                1,
                json!({"operation":"enqueue","content":"old process"}),
            ),
            record(
                "system",
                11,
                json!({"subtype":"scheduled_task_fire","uuid":"fire","prompt":"check"}),
            ),
            record(
                "queue-operation",
                12,
                json!({"operation":"enqueue","content":"check"}),
            ),
            record("queue-operation", 13, json!({"operation":"dequeue"})),
        ];
        let lines = records.iter().map(ToString::to_string).collect::<Vec<_>>();
        // The current attempt started at 10.
        assert_eq!(
            origin_in(
                lines.iter().map(String::as_bytes),
                "s",
                "p",
                "check",
                Timestamp(10)..=Timestamp(14),
                false
            ),
            SubmitOrigin::Scheduled
        );
    }

    #[test]
    fn a_visible_matching_enqueue_can_have_its_fire_outside_the_tail() {
        use serde_json::json;
        let records = [
            record(
                "queue-operation",
                2,
                json!({"operation":"enqueue","content":"check"}),
            ),
            record("queue-operation", 3, json!({"operation":"dequeue"})),
        ];
        let lines = records.iter().map(ToString::to_string).collect::<Vec<_>>();
        assert_eq!(
            origin_in(
                lines.iter().map(String::as_bytes),
                "s",
                "p",
                "check",
                Timestamp(0)..=Timestamp(4),
                true
            ),
            SubmitOrigin::UnidentifiedHandOver
        );
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
        assert_eq!(
            origin_of(&records, "typed", "check", 5),
            SubmitOrigin::Unmarked
        );
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
        assert_eq!(
            origin_of(&records, "p", "check", 13),
            SubmitOrigin::UnidentifiedHandOver
        );
        assert_eq!(
            origin_of(&records, "p", "other", 13),
            SubmitOrigin::Unmarked,
            "a different typed prompt still has its ordinary effects"
        );
    }

    #[test]
    fn an_identified_queued_prompt_leaves_the_next_queue_entry_in_place() {
        use serde_json::json;
        let mut records = vec![
            record(
                "queue-operation",
                1,
                json!({"operation":"enqueue","content":"first"}),
            ),
            record(
                "queue-operation",
                2,
                json!({"operation":"enqueue","content":"second"}),
            ),
            record(
                "system",
                3,
                json!({"subtype":"scheduled_task_fire","uuid":"fire"}),
            ),
            record(
                "queue-operation",
                4,
                json!({"operation":"enqueue","content":"check"}),
            ),
            record("queue-operation", 5, json!({"operation":"dequeue"})),
            record(
                "user",
                6,
                json!({"promptSource":"queued","message":{"role":"user","content":"first"}}),
            ),
            record("queue-operation", 7, json!({"operation":"dequeue"})),
        ];
        assert_eq!(origin_of(&records, "p", "check", 7), SubmitOrigin::Unmarked);
        records.push(record("queue-operation", 8, json!({"operation":"dequeue"})));
        assert_eq!(
            origin_of(&records, "p", "check", 8),
            SubmitOrigin::Scheduled
        );
    }

    #[test]
    fn a_delivered_notification_leaves_the_schedule_for_its_own_hand_over() {
        use serde_json::json;
        let notice = "<task-notification>done</task-notification>";
        let records = [
            record(
                "queue-operation",
                0,
                json!({"operation":"enqueue","content":notice}),
            ),
            record(
                "system",
                1,
                json!({"subtype":"scheduled_task_fire","uuid":"fire"}),
            ),
            record(
                "queue-operation",
                2,
                json!({"operation":"enqueue","content":"check"}),
            ),
            record("queue-operation", 3, json!({"operation":"dequeue"})),
            record(
                "user",
                4,
                json!({"promptSource":"system","message":{"role":"user","content":notice}}),
            ),
            record("queue-operation", 5, json!({"operation":"dequeue"})),
        ];
        assert_eq!(
            origin_of(&records, "p", "check", 6),
            SubmitOrigin::Scheduled
        );
    }

    #[test]
    fn successive_ticks_behind_a_stranded_notice_keep_personal_state() {
        use serde_json::json;
        let mut records = vec![record(
            "queue-operation",
            0,
            json!({"operation":"enqueue","content":"<task-notification>old</task-notification>"}),
        )];
        for (at, fire) in [(10, "first"), (20, "second")] {
            records.extend([
                record(
                    "system",
                    at,
                    json!({"subtype":"scheduled_task_fire","uuid":fire}),
                ),
                record(
                    "queue-operation",
                    at + 1,
                    json!({"operation":"enqueue","content":"check"}),
                ),
                record("queue-operation", at + 2, json!({"operation":"dequeue"})),
            ]);
            let lines = records.iter().map(ToString::to_string).collect::<Vec<_>>();
            assert_ne!(
                origin_in(
                    lines.iter().map(String::as_bytes),
                    "s",
                    fire,
                    "check",
                    Timestamp(0)..=Timestamp(at + 3),
                    false
                ),
                SubmitOrigin::Unmarked,
                "{fire} tick"
            );
            records.push(record("user", at+4, json!({"promptId":fire,"scheduledFireId":fire,"promptSource":"system","isMeta":true})));
        }
        let lines = records.iter().map(ToString::to_string).collect::<Vec<_>>();
        assert_eq!(
            origin_in(
                lines.iter().map(String::as_bytes),
                "s",
                "typed",
                "check",
                Timestamp(0)..=Timestamp(30),
                false
            ),
            SubmitOrigin::Unmarked
        );
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
            submit_origin(
                file.try_clone()?,
                "s",
                "p",
                "check",
                Timestamp(0)..=Timestamp(5)
            )?,
            SubmitOrigin::UnidentifiedHandOver
        );
        write(
            &mut file,
            "user",
            6,
            json!({"promptId":"p","isMeta":true,"promptSource":"system","scheduledFireId":"fire"}),
        )?;
        assert_eq!(
            submit_origin(file, "s", "other", "check", Timestamp(0)..=Timestamp(7))?,
            SubmitOrigin::Unmarked
        );
        Ok(())
    }
}
