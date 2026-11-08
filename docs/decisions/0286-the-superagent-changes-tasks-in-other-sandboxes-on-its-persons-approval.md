# 0286. The Superagent changes tasks in other sandboxes on its person's approval

## Status

Accepted. Specified in [SPEC-442](../../.ai/specs/SPEC-442-a-session-can-manage-the-tasks-it-asks-f.md);
task #424. Extends [0227](0227-the-superagent-starts-tasks-in-other-sandboxes-only-on-its-persons-approval.md),
whose "cross-sandbox messaging, stopping and reading" alternative it builds.

## Context

ADR-0227 let a Superagent start a task in another of its person's sandboxes, on the person's
click. Nothing could follow it up: on 2026-11-07 a Superagent created a task in another sandbox
prepared, the person wanted it queued, and the only way was to delete it and ask for a new one,
which the rule against recreating tasks forbids and which risks its attachments.

## Decision

- **Four Superagent-only tools**: `sandbox_task_get` (read), `sandbox_task_update` (start a task
  that has not started now, re-chain it, prepare or schedule it, as `task_update`),
  `sandbox_task_message` (as `task_message`, labelled as the person's) and `sandbox_task_stop` (as
  `task_stop`). Each names `sandbox` and the task by `task_id` or `number`.
- **Every change is a request of ADR-0227**: the same waiting set, bound (3 per person and source
  sandbox), 4-minute wait, card above the Superagent and browser-only answer. A request carries
  its action (`create`, `update`, `message`, `stop`) and, but for a create, the task it names; the
  card says what is asked and its button names the action. On approval the same-sandbox tool
  runs in the target for the person with exactly the arguments shown, so its checks (write
  access, the task's state, personal-secret tasks) hold at that moment.
- **Reads are not approved**: `sandbox_task_get` is pre-allowed, as `sandboxes_list` is.
- **Never delete, archive or restore across sandboxes**: no tool and no request action exists for
  them; the person does that on the target's Tasks page.
- **Audited in both sandboxes**: `superagent.cross_task` on the target (`from_sandbox`) and the
  source (`to_sandbox`), with the action, the task's number and the outcome, never the text.
- **No standing approval**: approving a create grants nothing later; each change is one click.
- **Guidance**: tasks in another sandbox are created queued unless the person asks for prepared;
  a request that should change is followed up with these tools, never deleted and recreated.

## Alternatives

- **An optional `sandbox` argument on `task_get`, `task_update`, `task_message` and
  `task_stop`.** Fewer names, but one tool would answer at once or wait minutes for a click
  depending on an argument, `task_get` could no longer be pre-allowed as a whole, and an agent
  reading the descriptions could not tell which calls ask the person. Rejected.
- **Let the session change, without asking again, tasks it created when the person allowed that
  on the create's card.** Saves a click on the follow-up, but needs a stored grant (which task,
  which session, until when) and lets text the Superagent reads later act unseen in another
  repository. The follow-up is rare and the card is one click. Rejected for now.
- **Approving reads too.** Every look at a task would put a card in front of the person, who would
  learn to click through them, weakening the cards that change something. Rejected.

## Consequences

- "Prepared but should be queued" is one `sandbox_task_update` and one click; nothing is deleted.
- A Superagent can steer a task of another repository only through cards its person reads; an
  injected instruction still only puts a card in front of them.
- The audit of each sandbox names cross-sandbox changes made to it or from it.
- Running Superagents get the tools after the update by restarting once idle (the launch digest
  covers tool descriptors and instructions).
