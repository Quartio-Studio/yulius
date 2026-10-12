# 0019. No messages until their delivery is reliable

## Status

Accepted. Specified in
[SPEC-028](../../.ai/specs/SPEC-028-delete-message-mechanism-for-now-until-w.md); narrows
[0006](0006-yulius-mcp-and-hooks.md) (the MCP server and hooks no longer carry messages).

## Context

After task-to-task messages went (SPEC-012), messages were Yulius's notices to an agent and an
agent's messages to people. A notice was stored, injected by the `UserPromptSubmit` hook, made the
turn-end gate block while urgent ones were unread, and was typed into an idle session as text plus
Enter. Typing into Claude Code's prompt box needed two fixes in one day (Enter read as a pasted
newline; wake-ups piling up in the input box), and the Messages panel mostly showed Yulius talking
to agents. The user asked to "delete message mechanism for now until we make the infra very good"
(task #26).

## Decision

Remove the mechanism end to end, not just its panel: no `message_send` or `messages_inbox` tools,
no stored notices, no hook digest, no "unread messages" stop reason, no idle wake-up, no Messages
panel, unread counts, notification or `GET /api/tasks/{tid}/messages`. Agents pull what they need
from the tools they already use: `yulius wait` (a background command Claude Code wakes them from)
ends when the checks at their head end, their delivery moves or an armed merge is decided, and it
now says how the armed merge ended (merged, refused with the reason, or dropped because the branch
moved or the checks failed). The outcome lives in memory next to the armed merge. The `messages`
tables, the store API and the `wake_enabled` column stay, so a return needs no migration.

## Alternatives

- Hide the panel and keep the pipeline: keeps the unreliable part (typing into sessions, the
  digest, the stop gate) running where nobody sees it.
- Keep notices but deliver them only through hooks: still a second channel next to `yulius wait`,
  and hooks fire only while the agent is already working.
- Drop the tables too: saves nothing now and makes a better mechanism later need a migration.

## Consequences

An agent that ends its turn without `yulius wait` running is not woken by Yulius; the task shows
"Your turn" and a person decides. People see what Yulius did in the task's terminal notices, its
steps and the Git view, not in a message list. A step the user rules out reaches the agent as a
skipped step with the user's note. An errored CI run is still queued again once by itself.
Rebuilding messages later means a delivery channel with an explicit, tested contract first.
