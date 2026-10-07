//! Provider-neutral transcript IR used for cross-provider translation (SPEC-002 §7.3, ADR-06).
//!
//! Adapters read a native Claude Code or Codex transcript into a [`Transcript`], the domain
//! bounds and normalizes it with [`plan_translation`], and adapters write the target CLI's
//! native format from the result.

use crate::harness::Harness;
use crate::time::Timestamp;

/// Metadata of the source session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TranscriptMeta {
    /// Harness that produced the transcript.
    pub source: Harness,
    /// Provider session id of the source.
    pub session_id: String,
    /// Working directory recorded by the source, if any.
    pub cwd: Option<String>,
    /// Model recorded by the source, if any.
    pub model: Option<String>,
}

/// Speaker of a turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Role {
    /// The person (or Yulius on their behalf).
    User,
    /// The coding agent.
    Assistant,
}

/// One piece of turn content.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Block {
    /// Plain text.
    Text(String),
    /// A tool invocation by the agent.
    ToolCall {
        /// Tool name (for example `Bash` or `shell`).
        name: String,
        /// Short human-readable rendering of the arguments.
        input_summary: String,
    },
    /// A tool's result.
    ToolResult {
        /// Result text (possibly already shortened by the reader).
        summary: String,
        /// The tool reported an error.
        is_error: bool,
    },
    /// Content that cannot be carried over (reasoning, images), kept as a marker.
    Omitted {
        /// What was dropped (for example `thinking` or `image`).
        what: String,
    },
}

impl Block {
    /// Plain-text rendering used when a target cannot represent the block natively.
    #[must_use]
    pub fn render(&self) -> String {
        match self {
            Self::Text(text) => text.clone(),
            Self::ToolCall {
                name,
                input_summary,
            } => format!("[tool {name}] {input_summary}"),
            Self::ToolResult { summary, is_error } => {
                let label = if *is_error {
                    "tool error"
                } else {
                    "tool result"
                };
                format!("[{label}] {summary}")
            }
            Self::Omitted { what } => format!("[{what} omitted]"),
        }
    }
}

/// One message of the conversation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Turn {
    /// Speaker.
    pub role: Role,
    /// Content in order.
    pub blocks: Vec<Block>,
    /// When the turn started, if known.
    pub at: Option<Timestamp>,
}

impl Turn {
    /// All blocks rendered as text and joined by blank lines.
    #[must_use]
    pub fn rendered_text(&self) -> String {
        self.blocks
            .iter()
            .map(Block::render)
            .filter(|text| !text.is_empty())
            .collect::<Vec<_>>()
            .join("\n\n")
    }
}

/// A whole conversation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Transcript {
    /// Source metadata.
    pub meta: TranscriptMeta,
    /// Turns in conversation order.
    pub turns: Vec<Turn>,
}

/// Size bounds applied by [`plan_translation`] (SPEC-002 §7.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TranslationLimits {
    /// Maximum bytes kept of each tool result.
    pub tool_output_bytes: usize,
    /// Maximum number of turns kept, including the omission note.
    pub max_messages: usize,
    /// Maximum total rendered bytes kept, including the omission note.
    pub max_bytes: usize,
}

impl Default for TranslationLimits {
    fn default() -> Self {
        Self {
            tool_output_bytes: 2048,
            max_messages: 300,
            max_bytes: 400 * 1024,
        }
    }
}

/// Upper bound of the bytes [`truncate_with_marker`] appends.
const MARKER_RESERVE: usize = 48;

/// Longest prefix of `text` that is at most `max_bytes` long and ends on a character boundary.
#[must_use]
pub fn truncate_utf8(text: &str, max_bytes: usize) -> &str {
    if text.len() <= max_bytes {
        return text;
    }
    let mut end = max_bytes;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

/// `text` shortened to at most `max_bytes` (on a character boundary) plus a marker stating how
/// many bytes were cut.
#[must_use]
pub fn truncate_with_marker(text: &str, max_bytes: usize) -> String {
    let kept = truncate_utf8(text, max_bytes);
    if kept.len() == text.len() {
        return text.to_owned();
    }
    format!("{kept}… [truncated {} bytes]", text.len() - kept.len())
}

/// Text of the note that replaces omitted older turns.
#[must_use]
pub fn omission_note(omitted_turns: usize) -> String {
    format!(
        "[Yulius] {omitted_turns} earlier message(s) of this conversation were omitted to fit \
         the translation size limit. The task specification, its plan and the Yulius \
         project_context and task_get tools hold the full context of this task."
    )
}

/// Produces the bounded, text-only transcript a target writer consumes.
///
/// Tool calls, tool results and omitted content become text blocks (`[tool Bash] …`,
/// `[tool result] …`, `[thinking omitted]`); tool results are cut to
/// `limits.tool_output_bytes`. Empty turns are dropped and consecutive turns of the same role
/// are merged so the result alternates roles. When the conversation exceeds
/// `limits.max_messages` turns or `limits.max_bytes` bytes, the oldest turns are dropped and a
/// user note pointing to the task specification is prefixed. A newest turn that alone exceeds
/// the byte budget is cut. Text is never split inside a UTF-8 character.
#[must_use]
pub fn plan_translation(transcript: &Transcript, limits: TranslationLimits) -> Transcript {
    let rendered = render_and_merge(&transcript.turns, limits.tool_output_bytes);
    let separator = 2;
    let turn_bytes = |turn: &Turn| -> usize {
        turn.blocks
            .iter()
            .map(|b| match b {
                Block::Text(t) => t.len() + separator,
                _ => 0,
            })
            .sum()
    };
    let note_reserve = omission_note(rendered.len()).len() + separator;

    // Walk from the newest turn backwards while both budgets hold. The note's budget is
    // reserved only when something is actually omitted.
    let fits = |count: usize, bytes: usize, omitted: bool| {
        let (count_cap, byte_cap) = if omitted {
            (
                limits.max_messages.saturating_sub(1),
                limits.max_bytes.saturating_sub(note_reserve),
            )
        } else {
            (limits.max_messages, limits.max_bytes)
        };
        count <= count_cap && bytes <= byte_cap
    };
    let total_bytes: usize = rendered.iter().map(turn_bytes).sum();
    let keep_all = fits(rendered.len(), total_bytes, false);
    let mut kept_from = 0;
    if !keep_all {
        let mut bytes = 0;
        kept_from = rendered.len();
        for (index, turn) in rendered.iter().enumerate().rev() {
            let next_bytes = bytes + turn_bytes(turn);
            if !fits(rendered.len() - index, next_bytes, true) {
                break;
            }
            bytes = next_bytes;
            kept_from = index;
        }
    }
    let mut kept: Vec<Turn> = rendered[kept_from..].to_vec();
    let mut omitted = kept_from;

    // A single newest turn larger than the whole budget is cut rather than dropped entirely.
    if kept.is_empty()
        && limits.max_messages >= 2
        && let Some(last) = rendered.last()
    {
        let budget = limits
            .max_bytes
            .saturating_sub(note_reserve + separator + MARKER_RESERVE);
        let text = truncate_with_marker(&last.rendered_text(), budget);
        kept.push(Turn {
            role: last.role,
            blocks: vec![Block::Text(text)],
            at: last.at,
        });
        omitted = rendered.len() - 1;
    }

    if omitted > 0 {
        let note = Block::Text(omission_note(omitted));
        match kept.first_mut() {
            Some(first) if first.role == Role::User => first.blocks.insert(0, note),
            _ => kept.insert(
                0,
                Turn {
                    role: Role::User,
                    blocks: vec![note],
                    at: None,
                },
            ),
        }
    }
    Transcript {
        meta: transcript.meta.clone(),
        turns: kept,
    }
}

fn render_and_merge(turns: &[Turn], tool_output_bytes: usize) -> Vec<Turn> {
    let mut out: Vec<Turn> = Vec::with_capacity(turns.len());
    for turn in turns {
        let blocks: Vec<Block> = turn
            .blocks
            .iter()
            .filter_map(|block| {
                let text = match block {
                    Block::ToolResult { summary, is_error } => Block::ToolResult {
                        summary: truncate_with_marker(summary, tool_output_bytes),
                        is_error: *is_error,
                    }
                    .render(),
                    other => other.render(),
                };
                (!text.trim().is_empty()).then_some(Block::Text(text))
            })
            .collect();
        if blocks.is_empty() {
            continue;
        }
        match out.last_mut() {
            Some(previous) if previous.role == turn.role => previous.blocks.extend(blocks),
            _ => out.push(Turn {
                role: turn.role,
                blocks,
                at: turn.at,
            }),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn meta() -> TranscriptMeta {
        TranscriptMeta {
            source: Harness::ClaudeCode,
            session_id: "s1".into(),
            cwd: Some("/w".into()),
            model: None,
        }
    }

    fn turn(role: Role, blocks: Vec<Block>) -> Turn {
        Turn {
            role,
            blocks,
            at: None,
        }
    }

    fn text(role: Role, body: &str) -> Turn {
        turn(role, vec![Block::Text(body.into())])
    }

    fn texts(t: &Transcript) -> Vec<String> {
        t.turns.iter().map(Turn::rendered_text).collect()
    }

    #[test]
    fn renders_tool_blocks_and_truncates_tool_output() {
        let long_output = "é".repeat(2000); // 4000 bytes, two bytes per char
        let transcript = Transcript {
            meta: meta(),
            turns: vec![
                text(Role::User, "run tests"),
                turn(
                    Role::Assistant,
                    vec![
                        Block::Text("Running.".into()),
                        Block::ToolCall {
                            name: "Bash".into(),
                            input_summary: "cargo test".into(),
                        },
                        Block::ToolResult {
                            summary: long_output,
                            is_error: true,
                        },
                        Block::Omitted {
                            what: "thinking".into(),
                        },
                    ],
                ),
            ],
        };
        let planned = plan_translation(
            &transcript,
            TranslationLimits {
                tool_output_bytes: 101,
                ..TranslationLimits::default()
            },
        );
        let assistant = &planned.turns[1];
        assert!(assistant.blocks.iter().all(|b| matches!(b, Block::Text(_))));
        let rendered = assistant.rendered_text();
        assert!(rendered.contains("[tool Bash] cargo test"));
        assert!(rendered.contains("[tool error] "));
        assert!(rendered.contains("[truncated 3900 bytes]"));
        assert!(rendered.contains("[thinking omitted]"));
        // 101 is inside a two-byte character: the cut lands on the boundary below it.
        assert!(rendered.contains(&format!("{}… [truncated", "é".repeat(50))));
    }

    #[test]
    fn merges_same_role_turns_and_drops_empty_ones() {
        let transcript = Transcript {
            meta: meta(),
            turns: vec![
                text(Role::User, "a"),
                turn(Role::Assistant, vec![]),
                text(Role::User, "b"),
                text(Role::Assistant, "c"),
                text(Role::Assistant, "d"),
            ],
        };
        let planned = plan_translation(&transcript, TranslationLimits::default());
        assert_eq!(texts(&planned), vec!["a\n\nb", "c\n\nd"]);
    }

    #[test]
    fn keeps_everything_within_limits_without_note() {
        let transcript = Transcript {
            meta: meta(),
            turns: vec![text(Role::User, "hi"), text(Role::Assistant, "hello")],
        };
        let planned = plan_translation(&transcript, TranslationLimits::default());
        assert_eq!(planned.turns.len(), 2);
        assert!(!texts(&planned)[0].contains("[Yulius]"));
        assert_eq!(planned.meta, transcript.meta);
    }

    #[test]
    fn drops_oldest_turns_beyond_message_limit_with_note() {
        let turns = (0..10)
            .map(|i| {
                let role = if i % 2 == 0 {
                    Role::User
                } else {
                    Role::Assistant
                };
                text(role, &format!("m{i}"))
            })
            .collect();
        let transcript = Transcript {
            meta: meta(),
            turns,
        };
        let planned = plan_translation(
            &transcript,
            TranslationLimits {
                max_messages: 4,
                ..TranslationLimits::default()
            },
        );
        let rendered = texts(&planned);
        // 3 newest kept + note; newest kept starts with an assistant turn, so the note is its
        // own user turn.
        assert_eq!(rendered.len(), 4);
        assert!(rendered[0].starts_with("[Yulius] 7 earlier message(s)"));
        assert_eq!(&rendered[1..], &["m7", "m8", "m9"]);
        assert_eq!(planned.turns[0].role, Role::User);
    }

    #[test]
    fn note_merges_into_a_leading_user_turn() {
        let turns = (0..6)
            .map(|i| {
                let role = if i % 2 == 0 {
                    Role::User
                } else {
                    Role::Assistant
                };
                text(role, &format!("m{i}"))
            })
            .collect();
        let transcript = Transcript {
            meta: meta(),
            turns,
        };
        let planned = plan_translation(
            &transcript,
            TranslationLimits {
                max_messages: 3,
                ..TranslationLimits::default()
            },
        );
        let rendered = texts(&planned);
        assert_eq!(rendered.len(), 2);
        assert!(rendered[0].starts_with("[Yulius] 4 earlier"));
        assert!(rendered[0].ends_with("m4"));
        assert_eq!(rendered[1], "m5");
    }

    #[test]
    fn byte_budget_drops_oldest_and_never_exceeds_limit() {
        let big = "x".repeat(1000);
        let turns = (0..20)
            .map(|i| {
                let role = if i % 2 == 0 {
                    Role::User
                } else {
                    Role::Assistant
                };
                text(role, &big)
            })
            .collect();
        let transcript = Transcript {
            meta: meta(),
            turns,
        };
        let limits = TranslationLimits {
            max_bytes: 5000,
            ..TranslationLimits::default()
        };
        let planned = plan_translation(&transcript, limits);
        let total: usize = planned
            .turns
            .iter()
            .flat_map(|t| &t.blocks)
            .map(|b| match b {
                Block::Text(t) => t.len() + 2,
                _ => 0,
            })
            .sum();
        assert!(total <= limits.max_bytes, "{total}");
        assert!(texts(&planned)[0].starts_with("[Yulius]"));
        assert!(planned.turns.len() < 20);
    }

    #[test]
    fn oversized_single_turn_is_cut_on_a_char_boundary() {
        let transcript = Transcript {
            meta: meta(),
            turns: vec![
                text(Role::User, "first"),
                text(Role::Assistant, &"ż".repeat(5000)),
            ],
        };
        let limits = TranslationLimits {
            max_bytes: 1000,
            ..TranslationLimits::default()
        };
        let planned = plan_translation(&transcript, limits);
        assert_eq!(planned.turns.len(), 2);
        assert!(texts(&planned)[0].starts_with("[Yulius] 1 earlier"));
        let cut = texts(&planned)[1].clone();
        assert!(cut.contains("[truncated"));
        assert!(cut.len() < 1000);
    }

    #[test]
    fn truncate_utf8_respects_boundaries() {
        assert_eq!(truncate_utf8("abc", 10), "abc");
        assert_eq!(truncate_utf8("aéb", 2), "a");
        assert_eq!(truncate_utf8("aéb", 3), "aé");
        assert_eq!(truncate_utf8("😀", 3), "");
        assert_eq!(
            truncate_with_marker("abcdef", 4),
            "abcd… [truncated 2 bytes]"
        );
    }
}
