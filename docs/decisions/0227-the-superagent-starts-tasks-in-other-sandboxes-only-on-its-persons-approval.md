# 0227. The Superagent starts tasks in other sandboxes only on its person's approval

## Status

Accepted. Specified in [SPEC-335](../../.ai/specs/SPEC-335-superagents-should-be-able-to-talk-to-ot.md);
task #319. Extends [0029](0029-cockpit-manages-through-mcp.md) (the Superagent's tools act in its
own sandbox) with one tool that reaches the person's other sandboxes. Leaves
[0165](0165-the-superagent-bypasses-approvals-on-contained-servers.md) (the approval bypass)
unchanged.

## Context

People asked for their Superagent to "talk to other sandboxes": in one sandbox, start a task in
another (a client repository after an API change, a website after a product release). Every
Superagent tool so far is pinned to its own sandbox by its `ck_` token. Reaching further is a
larger risk than any of them: the Superagent reads repository files, transcripts and MCP output,
and on a contained server it runs without Claude Code's approval prompts (ADR-0165). Text it reads
could then start work in every repository its person can write. The person asked that every such
action require their explicit acceptance.

## Decision

- **Two Superagent-only tools.** `sandboxes_list` names the other live sandboxes the person can
  work in (write access); it only reads and is pre-allowed. `sandbox_task_create` takes
  `task_create`'s arguments plus `sandbox` (slug, owner/name or id). Task agents are served
  neither. The Superagent's own sandbox, a sandbox the person cannot work in (as not found),
  unknown arguments and an empty prompt are refused before anyone is asked.
- **The server asks the person, every time.** A valid call records a request in memory and waits
  for the answer, at most 4 minutes (`CROSS_REQUEST_WAIT`), at most 3 waiting per person and
  source sandbox. The person's Superagent panel lists it above the session with the target, the
  title, the whole prompt and the start options set, and **Create task** / **Decline**. A
  request whose texts hold characters a person cannot see (Unicode tag characters, which carry
  hidden ASCII, bidirectional and zero-width marks, blank fillers, variation selectors, private
  use) is refused before anyone is asked, so emoji with a variation selector are refused too. The
  card shows any such character by its code and counts long runs of empty lines; Create
  task takes a click only once the prompt's end was on screen (an intersection observer, so
  neither the prompt box, the scrolling list nor the window hides the rest), and both buttons only 600 ms after
  the card appeared or moved up because another went (no double-click approves the next one)
  (`GET`/`POST /api/sandboxes/{sid}/superagent/requests[/{id}]`, announced to that person only
  by `superagent.requests`).
- **Only a person's click answers.** The answer needs a browser session (a cookie with CSRF,
  `AuthContext::is_browser`); every API token, an operate token included, gets 403, so an agent
  that holds one, the Superagent itself included, cannot approve. Only the request's person sees
  or answers it; another person gets not found.
- **Approval runs `task_create` in the target**, as the person, with exactly the arguments shown:
  write access, the onboarding gate, workflows, chains and the hourly bound are checked there at
  that moment, and a refusal is the answer of both the click and the waiting call. The prompt is
  not rewritten.
- **A request lives as long as its call.** Answered, unanswered (the bound), dropped (the client
  went away), cancelled (`notifications/cancelled` naming the call's JSON-RPC id, which Claude
  Code sends on Esc while it may keep the HTTP request open) or lost with a server restart, it is
  gone and cannot be approved later. An approval, once taken, runs detached from the person's
  HTTP request, so a page that goes away mid-way still lets the call learn the outcome. A guard removes it whichever way the call ends; a timeout that races
  an approval being created waits for that outcome (up to a minute) instead of saying
  "unanswered".
- **Audited** `superagent.cross_task` on the target with `approved`, `failed`, `declined`,
  `cancelled` or `expired`, the source sandbox and the caller, never the prompt.

## Alternatives

- **Claude Code's permission prompt** (leave `sandbox_task_create` out of `--allowedTools`). On a
  contained server the bypass skips it, and elsewhere "always allow" ends it after one answer.
  The acceptance would not hold for every action. Rejected.
- **Stored requests, answered at any time, with a status tool.** The person could approve
  hours later, but the Superagent would learn of it only by polling, and a stored request needs a
  migration, an expiry sweep and a way to tell a Superagent that started again. The person who
  asks is at the panel. Rejected for now; a later need can store them.
- **A longer wait.** MCP clients commonly run on Node's HTTP client, which gives up when the
  answer's headers take more than 300 s. A wait above that would turn an approval into an error
  in the Superagent while the task is created. 4 minutes stays below it.
- **Approval through a notification or the Tasks page of the target.** The person talks to the
  Superagent in its panel; a card elsewhere would be missed while the call waits.
- **Cross-sandbox messaging, stopping and reading.** Not asked for; each would need the same
  approval and is left to a later task.

## Consequences

- "Make a task in the website repo for that" works from any sandbox's Superagent, and nothing
  reaches the other repository without the person reading and approving that exact request.
- The Superagent's turn waits while the person decides. Interrupting it (Esc) or not answering
  in 4 minutes ends the request; the Superagent says nothing was created and asks again only when
  told.
- An injected instruction can at most put a card in front of the person. It cannot create the
  task, hide words of it below the fold or behind invisible characters, or catch a stray click,
  and it cannot fill the panel: at most 3 requests wait at once.
- Requests are in memory: a server restart (an update) ends the waiting calls with an error and
  their cards with them.
- Running Superagents get the tools after the update by restarting once idle (the launch digest
  covers tool descriptors).
