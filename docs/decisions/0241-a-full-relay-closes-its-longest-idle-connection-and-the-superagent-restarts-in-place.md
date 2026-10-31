# 0241. A full relay closes its longest-idle connection, and the Superagent restarts in place

## Status

Accepted. Specified in
[SPEC-355](../../.ai/specs/SPEC-355-a-superagent-whose-sandbox-lost-yulius-r.md); task #337,
issues #974 and #970. Builds on
[ADR-0240](0240-a-run-s-strays-are-reaped-after-a-bound-and-a-sandbox-gets-half-the-proxy.md)
(the proxy share and the strays) and
[ADR-0239](0239-the-superagent-gets-a-fingerprint-file-as-a-task-run-does.md) (the Superagent's
fingerprint file).

## Context

On 2026-10-30 a Superagent's hooks and MCP were refused with `ECONNRESET at
http://127.0.0.1:8081`, and a Yulius restart did not bring it back. A sandbox reaches Yulius on one
port through two relays: the holder's forward inside the sandbox and the server's `out.sock`, 256
connections each. At its limit a relay closed every new connection unread, so its client got a
reset. A server restart does not restart the holder, and the holder keeps a connection whose
server end closed until its client closes too. A leftover process that holds a socket never
does, so the forward stayed full after the restart. A deterministic test showed the agent's call
reset before the restart and after it.

ADR-0240's per-sandbox proxy share does not protect the forward: the proxy counts tunnels at the
server, which a restart forgets, and a sandbox's processes may hold the forward with other
connections. Claude Code also keeps an HTTP MCP server disconnected once it failed, so a
Superagent stays cut off from its Yulius tools even after the relay frees up. Until now the only
way out was to close the pane.

## Decision

- **A full relay gives way.** At its limit a relay toward Yulius (the holder's forward, the
  server's `out.sock`) admits a new connection by closing its longest-idle one, if that one
  carried no byte for at least 5 minutes (`IDLE_FLOOR`). The egress proxy does the same at a sandbox's
  share, with that sandbox's own connections, and at its own limit, with any. A relay or proxy
  whose connections were all active within the floor refuses as before. Each connection's
  activity is updated by its copy loop, and the choice is a deterministic domain table
  (`yulius_domain::connections`). Giving way is logged at most once a minute.
- **The floor keeps live calls.** The longest call to Yulius that carries no byte while it waits
  is the Superagent's `sandbox_task_create`: 240 s for its person and 50 s of grace (SPEC-335,
  below Node's 300 s for an answer's headers). `yulius wait` holds at most 55 s, and the MCP route
  answers on each request (`GET /mcp` is 405). A call in progress is therefore idle for less than
  the floor. A test ties the floor to that call. A plain HTTP request through the proxy is held in
  flight until its answer's head arrives, because its bytes pass through no tracked stream.
- **Yulius tests each Superagent's link.** Every minute, from inside the sandbox through the
  holder's forward, `GET /internal/healthz` must be answered. Two failures in a row count as a
  lost link, because one test can fail while a restarted server is still adopting its shells. The holder's `in.sock` now also
  connects to its own forwarded ports for this. `in.sock` is reachable only by the server. A holder
  started by an older Yulius refuses the port, which counts as unknown, not lost.
- **The panel says so, and the Superagent restarts in place.** While the link is lost, or once it
  came back after a loss, the Superagent's panel says so and offers Restart Superagent. The
  restart ends the session and starts it again, resuming its newest conversation, as a refresh
  does (SPEC-186), but at once because a person asked for it.
- **An idle Superagent's strays are reaped** like an idle task's (ADR-0240), probed every minute
  while its conversation is idle and checked again right before the reap. A terminal of the Shell
  panel is a person's own and is not probed.

## Alternatives

- **Reserve connections for the agent (a separate port or Unix socket for Yulius's own
  clients).** Rejected: the relay sees only bytes, and every process in the sandbox inherits the
  agent's environment (`YULIUS_URL`, `HTTPS_PROXY`). A reserved port would need the wrapper to hide
  it from commands, and it would change every run's environment and the holder's forwards. ADR-0240
  rejected that for the same reason. Eviction needs no identity: a leftover's held connection is
  idle, while the agent's calls are short or active.
- **Evict only connections whose server end closed (half-closed).** Rejected as the only rule: it
  covers the restart, but not leftovers that hold live idle tunnels before it.
- **Larger limits.** Rejected: a leak grows past any limit (ADR-0240).
- **Restart holders on a server restart.** Rejected: that ends every run in the sandbox.
- **Detect the lost link from the Superagent's transcript (hook errors).** Rejected: since
  ADR-0239 its Bash runs from the fingerprint file while Yulius cannot be reached, and MCP failures
  are not in the transcript. The test through the forward is the path its calls take.

## Consequences

- A leftover process's connection idle for 5 minutes closes when the relay or the proxy needs the
  room. Its client sees a reset and reconnects, which HTTP clients do.
- An agent's own call idle past the floor (a tool that thinks for minutes) can give way only when
  no connection has been idle longer.
- A holder runs its relay from its own start. One started before this change keeps the old relay,
  and no link test, until the sandbox's holder starts again. The operations guide says how.
- A restart ends a task request the Superagent was waiting on (SPEC-335): the person asks again.
