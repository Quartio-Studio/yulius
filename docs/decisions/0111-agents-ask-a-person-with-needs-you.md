# 0111. Agents ask a person to act with one tool, `needs_you`

## Status

Accepted. Specified in [SPEC-134](../../.ai/specs/SPEC-134-implement-cd-it-should-write-cd-bash-scr.md);
task #113. Extends [0068](0068-needs-you-means-a-person-has-to-act.md).

## Context

"To make something be in 'needs you' the agent should explicitly call a tool and prompt the user to
do something, for example to put the credential in secrets." Setting up CD needs people: a machine's
credentials go into the vault, a secret is allowed outside the proxy, production is deployed. The
existing requests are each bound to one surface (`browser_request_user_action` to a browser tab,
`secret_request_hosts` and `secret_copy_to_env` to a secret). An agent that needed anything else
stopped and waited, and its question was only in the transcript.

## Decision

- MCP `needs_you {action, why?, link?}` records one open request per task: what the person should
  do, why, and where. The task is in the sidebar's Needs you group, reads "Needs you: <action>", and
  people are notified, while the agent goes on with other work.
- The request ends when a person writes to the task (a prompt they typed into its terminal, or
  continuing a stopped task; prompts Yulius types do not count) or when the agent calls
  `needs_you {resolved: true}` (it saw the result, for example the secret is there). An ended task
  shows none.
- The request is stored with the task, so a restart keeps it, and audited (`task.needs_you`).

## Alternatives

- **Steps marked blocked.** A plan state the Git page shows, not a request people are notified of.
- **Issues.** They do not reach Needs you and are meant for follow-up work.
- **One tool per kind of request.** Every new need would wait for a new tool.

## Consequences

- An agent states exactly what a person must do; nothing else puts a working task in Needs you.
- A request no person answers keeps the task in Needs you, which is what it means.
