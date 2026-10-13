# 0051. The Cockpit moves past a usage limit like a task, and is called the Superagent

## Status

Accepted. Specified in [SPEC-074](../../.ai/specs/SPEC-074-i-just-hit-usage-limit-in-the-agent-it-s.md)
(task #69). Extends [ADR-0020](0020-cockpit-as-a-plain-shell-session.md) and
[ADR-0027](0027-step-back-past-a-usage-limit.md); renames the UI name of
[ADR-0036](0036-agent-in-the-right-panel.md).

## Context

The Cockpit (the Agent panel) took a pool subscription once, when it started, and nothing watched
it afterwards. A Cockpit lives as long as someone views it, so it could stay for days on a
subscription that tasks filled and then left. A person hit "You've hit your weekly limit" in it
while ten tasks kept working on the same pool. Tasks never stay stuck like that: their transcript
observer reports the limit, it is recorded on the subscription and the session resumes elsewhere,
stepped back over the limit (SPEC-034). The person also asked for the panel to be called
"Superagent", because every task runs "an agent" too.

## Decision

A Cockpit remembers its subscription in its shell record and watches its newest conversation while
it runs. It reads the conversation from the start and acts only on a limit recorded after the
Cockpit was created (the limit observation carries its record's time). It records the limit on the
subscription and, under the Cockpit's start lock, starts it again on a subscription with capacity.
The new process resumes the conversation stepped back over the limit with Claude Code's
interrupted-turn flag. The old one ends with the exit status `switched`, and the web panel follows
it to the new one. Without capacity, the Cockpit stays and its viewers get a notice. The web UI
calls the panel **Superagent**. The API, the shell kind and the records keep `cockpit`.

## Alternatives

- **Switch ahead of the limit at the rotation threshold**: restarts a session the person may be
  typing in, for a limit that may never come; one failed request is cheaper, and the step back
  removes it.
- **Swap the token in the running process**: Claude Code reads `CLAUDE_CODE_OAUTH_TOKEN` once, at
  start.
- **Read the terminal for the limit text**: its wording and wrapping change with the CLI and the
  width; the transcript record is structured and already parsed for tasks.
- **Tail only new lines**: a fresh conversation's first prompt can hit the limit before the watch
  first sees the file; reading from the start with a time filter catches it without replaying old
  limits.

## Consequences

A limit costs the person one reconnect of the panel, not a wait for the reset. The rotation
decides where the Cockpit goes, so it skips a subscription whose Opus cap is spent (SPEC-023). A
Cockpit adopted after a restart is watched too, and a limit recorded while the server was down
counts. The watch reads the newest conversation every second, and its first read of a
conversation costs its whole length once.
