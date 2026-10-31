---
yulius-task: 1f698f47-3522-4ef5-9129-1ea074abe1fc
status: in-progress
---

# SPEC-355: A Superagent whose sandbox lost Yulius recovers without the person closing it

Status: in progress
Date: 2026-10-31
Owner: Yulius task #337
Scope authorization: task #337, a follow-up to #335 (PR #971, SPEC-353), #334 (PR #967,
SPEC-352) and #333. Production host, quartio-v2 sandbox `7c46cdac-…`, 2026-10-30/31.

## Goal

Leftover processes that use up a sandbox's connections to Yulius no longer cut off its
Superagent, before or after a Yulius restart. When the Superagent's link to Yulius is unusable
anyway, its panel says so and restarts it in place, resuming its conversation. An idle
Superagent's leftover processes are reaped like an idle task's.

## Context

### What happened

- 21:37 CET: every Bash call and hook of the Superagent `claude --resume cec9f680-…` was refused
  ("the agent-secret fingerprints of this run cannot be loaded"). SPEC-353 traced it to the
  sandbox's one relayed port to Yulius (8081: internal API, MCP and egress proxy) running out of
  connections, held by leftover processes.
- 21:40: `systemctl restart yulius`. Task runs reattached, the Superagent never recovered: at
  00:34 its hooks were still refused, and `/mcp` → reconnect failed with `ECONNRESET at
  http://127.0.0.1:8081`. Only closing the pane got out, losing the conversation's place and a
  pending task request (SPEC-335 XSR-008).

### Root cause (verified, not assumed)

- The path of a call from inside a sandbox: a process connects to `127.0.0.1:8081` in the
  sandbox; the holder's forward relay (`relay::serve_forward`, inside the sandbox, a process of
  its own) takes one of its 256 places and connects to the server's `out.sock`
  (`relay::serve_outbound`, in the server, 256 per sandbox), which connects to the internal
  listener.
- The forward refuses past its limit by closing the accepted connection unread: the client has
  sent its request, so the kernel answers with a reset. That is the `ECONNRESET` the person saw.
- A server restart does not restart the holder (`Manager::adopt` only starts a new `out.sock`
  server beside the running holder). The holder copies each connection with
  `copy_bidirectional`: when the server's end closes (the old process exits), it shuts down the
  client's write side and keeps waiting for the client to close too. A leftover process that keeps
  its socket open and never writes holds its place for good. So the forward stays at its limit
  after the restart, and only the Superagent's processes, which use it all the time, see it.
- Reproduced deterministically: `relay::tests::the_agent_gets_through_a_forward_full_of_leftovers_before_and_after_a_restart`
  fills a forward of 4 places with 4 idle leftover connections through a real `serve_outbound`
  running in its own runtime (dropped as the process exits), then starts a new one. Before the
  fix the agent's `POST /internal/scrub/fingerprints` gets `ConnectionReset` before the restart
  and after it.
- SPEC-353's per-sandbox proxy share (128 of 256) does not protect the forward: it counts tunnels
  at the server, which a restart forgets, while the holder still counts the dead ones; and a
  sandbox's processes may hold the forward with connections that are no tunnels.
- Claude Code does not reconnect an HTTP MCP server that failed (seen again in task #42's
  transcript: "the Yulius MCP server, which is still disconnected (ECONNRESET)"). Once its link
  failed, a Superagent keeps its Yulius tools disconnected until it starts again, even after the
  relay frees up. SPEC-352 already gives it a fingerprint file, so its Bash runs while Yulius
  cannot be reached; that file does not help MCP.
- Strays: SPEC-353 LAP-004 reaps only an idle task's strays (#970). The Superagent's sandbox
  session lives for days, and nothing probes it.

## Requirements

- **SLR-001** At its limit, a relay toward Yulius (the holder's forward, the server's `out.sock`)
  admits a new connection by closing the connection that has been idle longest (no byte either
  way), if it has been idle for at least `IDLE_FLOOR` (60 s), instead of refusing the new one.
  Only when every connection was active within the floor is the new one refused (and warned
  about, as SPEC-353 LAP-002). An eviction is logged at most once a minute per relay.
- **SLR-002** The egress proxy does the same at a sandbox's share (128): it closes that sandbox's
  longest-idle tunnel or intercepted connection, idle for at least the floor; and at its own
  limit (256), the longest-idle one of any sandbox. Otherwise it refuses as before (`503`).
- **SLR-003** The floor keeps a live call: Yulius's longest hold (`/internal/agent/wait`, at most
  55 s) is below it, and the Yulius MCP route answers each call on its request (no idle stream:
  `GET /mcp` is 405).
- **SLR-004** An idle Superagent's strays (SPEC-353 LAP-003) are logged when first seen and reaped
  once seen for `stray_reap_after_ms`, signalled only while their start times match. Its
  conversation counts as idle when no turn is open and no background work runs, as its refresh
  reads it (SPEC-186 SAV-006), and it is checked again right before the reap: a prompt written
  since keeps them. Processes under the agent are never strays. A terminal of the Shell panel is
  a person's own and is not reaped.
- **SLR-005** Yulius tests each live Superagent's link to Yulius every `LINK_EVERY` (60 s): from
  inside its sandbox, through the holder's forward, a request to the internal listener must be
  answered. A holder that cannot be asked (one started by an older Yulius) gives no answer, which
  is not a failure.
- **SLR-006** When the link test failed since the Superagent started, its panel says so above its
  session: while it fails, that the Superagent cannot reach Yulius and its Yulius tools fail; once
  it works again, that it lost Yulius at that time and its tools may stay disconnected. Both offer
  **Restart Superagent**, which ends the session and starts it again in place, resuming the same
  conversation (`POST /api/sandboxes/{sid}/cockpit/restart`). The panel follows it as it follows a
  refresh. A request it had pending ends with it (said in the notice's help).
- **SLR-007** `docs/operations.md` says how an operator sees a saturated sandbox relay (holder
  log, the relay's connections in the sandbox's network namespace) and frees it without
  restarting Yulius: end the leftover processes, or restart the sandbox's holder when its relay
  predates this change.

## Plan

- [x] Reproduce the defect and find its root cause
- [ ] Relays toward Yulius give way to a new connection at their limit
- [ ] The egress proxy gives way at a sandbox's share and at its limit
- [ ] An idle Superagent's strays are reported and reaped
- [ ] Yulius tests each Superagent's link to Yulius
- [ ] The panel says when the link is lost and restarts the Superagent in place
- [ ] Operations guide, ADR, architecture and changes
- [ ] Independent review in a fresh session, findings fixed test-first
- [ ] Gates green, pull request merged into dev on green CI

## Acceptance criteria

- A forward whose places are all held by idle leftover connections takes the agent's call, before
  and after the server process restarts (red before the fix: `ConnectionReset` both times).
- A forward whose places were all active within the floor still refuses, and the longest-idle
  connection is the one closed.
- A sandbox holding its whole proxy share with idle tunnels gets a new tunnel, and the oldest idle
  one closes; with fresh tunnels it is still refused `503`.
- An idle Superagent's strays are reaped after the bound; a Superagent in a turn keeps them.
- A Superagent whose link test fails has its panel say so with Restart Superagent; the restart
  resumes the same conversation in a new process.

## Test plan

- Unit (`yulius-domain::connections`): the admission table: free place, eviction of the
  longest-idle past the floor, refusal within the floor, per-key and total limits.
- Unit (`yulius-sandbox::relay`): the restart reproduction above; a forward of active
  connections still refuses.
- Integration (`yulius-egress/tests/proxy.rs`): idle tunnels at the share give way; fresh ones
  are refused as before.
- Integration (`yulius/tests/cockpit.rs`): an idle Superagent's strays reaped after the bound and
  kept during a turn (a session host that reports strays); a failed link test is reported and the
  restart resumes the conversation.
- Unit (`yulius-sandbox::relay`): the link test through a forward answers, and fails against a
  full forward of fresh connections.
- e2e (`e2e/tests/agent-panel.e2e.ts`): the lost-link notice and its restart.

## Decisions

- ADR-0241: relays toward Yulius and the egress proxy give way to a new connection by closing the
  longest-idle one past a floor, rather than reserving places for the agent. The relay sees only
  bytes and cannot tell the agent's connections from a leftover's (they share its environment and
  `HTTPS_PROXY`); a reserved port would change every run's environment (rejected in ADR-0240 for
  the same reason). Eviction needs no identity: the agent's calls are short or active, a leftover's
  held connection is idle.
- The restart in place reuses the refresh path of SPEC-186 (end, start again resuming the newest
  conversation); it is a person's explicit action, so it does not wait for the turn to end.

## Review

(to be filled)

## Evidence

- Red before the fix: `cargo test -p yulius-sandbox --lib the_agent_gets_through` →
  `the agent's call before the restart: Err("ConnectionReset: Connection reset by peer (os error
  104)"), after it: Err("ConnectionReset: …")`.
