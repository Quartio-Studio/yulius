# 0243. The busiest run gives way at a full proxy share

## Status

Accepted. Specified in
[SPEC-357](../../.ai/specs/SPEC-357-urgent-the-egress-proxy-fails-closed-503.md); task #342.
Refines [ADR-0240](0240-a-run-s-strays-are-reaped-after-a-bound-and-a-sandbox-gets-half-the-proxy.md)
(each sandbox gets half of the egress proxy) and
[ADR-0241](0241-a-full-relay-closes-its-longest-idle-connection-and-the-superagent-restarts-in-place.md)
(a connection idle for 5 minutes gives way).

## Context

On 2026-10-31, from about 10:50 UTC, the egress proxy answered every `CONNECT` from the quartio-v2
sandbox with `503` for more than 90 minutes. The sandbox's processes held all of its 128
connections. Every task of that sandbox lost git, gh and its package registries. Claude itself kept
working, because `anthropic.com` is in every run's `NO_PROXY`. The running release had ADR-0240's
share but not ADR-0241's give-way, so nothing freed a place until the server restarted.

ADR-0241 frees a place only from a connection idle for 5 minutes. A connection that stays active
(a page's live stream or WebSocket from a leaked browser, polling, a long download) never gives
way. The proxy also counted only the sandbox, so one run whose processes keep their connections
active still locks every other run of the sandbox out, and the refusal does not say whose they
are.

## Decision

- **The proxy counts each connection's run.** A connection belongs to its sandbox and to its run:
  an agent run (attempt), a CI job or a Superagent's shell (`EgressClient::use_key`).
- **At a full sandbox share,** the sandbox's longest-idle connection past the floor gives way
  first, as in ADR-0241. If none was idle that long, the busiest other run of the sandbox gives
  up its oldest connection, when it holds at least 2 more than the asking run. A plain request
  waiting for its answer is held in flight and is never taken.
- **At the proxy's full total,** the same rule applies one level up. The longest-idle connection of
  any sandbox goes first. Otherwise the busiest sandbox's busiest run gives up its oldest
  connection, when that sandbox holds at least 2 more than the asking sandbox.
- **A refusal means no other run had room to give.** A run (or sandbox) is refused only when no
  other one has 2 more connections to give up (requests in flight are never taken), so a share
  used evenly is refused as before. The `503` says how many of the sandbox's connections the asking
  run holds, and whether that is the most. A request refused anyway (bad target, local
  destination) takes no place first. A connection taken from a busier run or sandbox is logged
  once a minute per sandbox and kind, with the run and its count.
- The limits stay: 256 in all and 128 per sandbox. The sandbox's relay toward Yulius (256) also
  carries its MCP, hook and wait calls, and ADR-0240 keeps half of it for them.

## Alternatives

- **A fixed cap per run (say 64).** Rejected: it halves a lone run's ceiling, and many runs below
  their caps still fill the share and lock the next one out.
- **A share that grows with the sandbox's live sessions.** Rejected: the bound that matters is the
  sandbox's relay toward Yulius, which does not grow. A larger share would let tunnels crowd out
  the sandbox's own calls to Yulius.
- **Closing the oldest active connection of the sandbox, whoever holds it.** Rejected: it takes
  from a run that holds one connection as readily as from the one that holds a hundred.
- **Telling leftovers from the agent's traffic.** Rejected: the proxy sees one credential per run,
  shared by every process the run starts (ADR-0241).

## Consequences

- One run's processes can fill an otherwise idle sandbox share, but they lose their oldest
  connections as soon as another run needs a place. The busy run's long-lived connections
  (usually leftovers' streams) close first, and their clients see the tunnel end.
- A run that legitimately needs many simultaneous connections can lose its oldest one while
  another run of the sandbox works. HTTP clients reconnect, and the loss is logged.
- A refusal at a share now means that no other run of the sandbox has 2 more connections to
  give up: the asking run is the busiest, or the runs hold about as many each. The text says
  which, and the run that reads it in its own commands' output is the one whose processes to
  end. The operations guide lists a sandbox's processes and their connections.
