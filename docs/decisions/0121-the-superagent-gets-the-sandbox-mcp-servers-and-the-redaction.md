# 0121. The Superagent gets the sandbox's MCP servers and the task runs' redaction

## Status

Accepted. Specified in [SPEC-147](../../.ai/specs/SPEC-147-fix-395-the-superagent-gets-the-sandbox.md);
task #141, issue #395. Amends SPEC-037 MGT-004 and SPEC-021 CKP-002
([ADR-0029](0029-cockpit-manages-through-mcp.md)): the Superagent's MCP addition is no longer the
Yulius server alone, and it gets one hook, the redaction hook.
Superseded in part by [0239](0239-the-superagent-gets-a-fingerprint-file-as-a-task-run-does.md):
the Superagent now has a fingerprint file, so its commands run redacted while Yulius restarts
(the first consequence below no longer holds). A loss of access still stops them: a refusal is
not read as a restart.

## Context

Since SPEC-095 a repository's `.mcp.json` servers join the sandbox's MCP servers and reach every
task run, and the Superagent (the Cockpit) rejects the file's servers instead of asking. The
Superagent itself got only the `yulius` server, so a person asking it a quick question could not
use the project's servers (supabase, apify, …).

Those servers carry secrets: a stdio server's environment and an HTTP server's bearer, either a
value typed into the server's form or `${secret:NAME}`, the value of an agent secret. A task run
holds the values in its environment and keeps agent secrets out of what it reads by redaction
inside the session (SPEC-002 ADR-20): every Bash command, hook and stdio MCP server runs through
`yulius --scrub-exec`, and the `PostToolUse` hook replaces a tool result holding a value. The
Superagent had no hooks at all (CKP-002). Agent secret values are otherwise shown to nobody: a
person who manages one reveals it only after a two-factor unlock.

## Decision

- **The same servers as a task run.** The Superagent's inline `--mcp-config` lists `yulius` and
  every server `App::mcp_attachments` gives a task run of its sandbox, in the task run's shape: the
  values only as `${YULIUS_MCP_<SERVER>_<VAR>}` references, the values in its secret environment
  (never argv, a file or the shell record). The config stays non-strict: the person's own servers
  keep working.
- **The same redaction as a task run.** The Superagent's `--settings` holds the `PostToolUse`
  redaction hook (matcher `*`), and its environment `CLAUDE_CODE_SHELL_PREFIX=<yulius> --scrub-exec`.
  It is the only hook: CKP-002 excluded the task lifecycle (briefing, `Stop`, status line, the
  `pgrep` guard), which the Superagent still does not get. Its screen is redacted on the server
  as a task's terminal is (VLT-007): what passes the in-session layers (an MCP tool error, which
  `PostToolUse` cannot rewrite, a result the hook could not read, what Claude Code shows of the
  raw result) reaches the person's screen as `[secret:NAME]`.
- **Fingerprints with the workspace token.** The wrapper and the hook load the fingerprints with
  `YULIUS_TASK_TOKEN`, which in a person's environment is their workspace token (`ws_…`, already
  there for the Git credential helper). The fingerprint and report endpoints accept it while its
  person is enabled and may work in its sandbox, by an access decision within the staleness bound
  (30 minutes), so that every command and tool result is answered without waiting on GitHub;
  redactions are audited with the person's login, once per person, sandbox, secret and channel
  while Yulius runs.
  The Superagent has no fingerprint file: while Yulius is unreachable its commands do not run and
  tool results are withheld (fail closed), as a task run's would without its file.
- **Literal MCP values are not redacted**, in the Superagent as in task runs: they are not agent
  secrets, and the Superagent runs for a person with write access, who may configure the
  sandbox's servers (and so set those values) anyway. An instance server's literal value is
  readable to them, as it already is through any task run they start. A value meant to be hidden
  from people is an agent secret referenced as `${secret:NAME}`.

- **Servers imported from `.mcp.json` keep their placeholders.** The import stores a file's
  `${VAR}` as the agent secret's placeholder (`yulius-secret-VAR`), which only the egress proxy
  replaces, on the secret's bound hosts. The Superagent is not behind the proxy: such a server
  starts in it with the placeholder and its calls are refused until the person's session is put
  behind the proxy, a decision of its own (all of the person's traffic through Yulius, and agent
  secrets usable by placeholder from their session). A server whose value is `${secret:NAME}`,
  set on the MCP page, works in the Superagent today.

## Alternatives

- **No secret-bearing servers in the Superagent** (only servers without values). Most of the
  project's servers need a key, so the Superagent would still not use them. Rejected.
- **A Yulius-side relay** that runs the servers and serves their tools through the `yulius` MCP
  server, so values never reach the session. The servers would no longer be the person's native
  Claude Code servers (tools renamed, resources and prompts lost), and every server would need a
  process host in Yulius. Far larger for the same guarantee the task runs live with. Rejected.
- **Let the Superagent see the values**, as a person's own terminal could. Agent secrets are shown
  to nobody without a two-factor unlock; a person with write access would read every one referenced
  by a server just by asking. Rejected.
- **A new Superagent token for the scrub endpoints.** The person's process already holds the
  workspace token, which is checked against their current access at every call; a second token
  would add a variable and a lookup and protect nothing more.
- **A fingerprint file in the person's home** for restarts. The home is the person's to write, the
  file would go stale, and a restart is short. Not now.

## Consequences

- The Superagent can use the project's MCP servers. Their agent-secret values never reach the
  model or the screen through Bash output or a tool result, as in a task run; like a task run's,
  the redaction is exact-value and in-session (a command that transforms a value beyond the known
  encodings is not caught), and it adds a hook process per tool call.
- While Yulius is down or restarting, the Superagent's Bash commands do not run and its tool
  results are withheld. Its Bash commands run 10 nice levels below it, as an agent's do (the
  wrapper's `nice`).
- A loss of write access stops the Superagent's commands within the staleness bound, or at once
  when another check (its own Yulius MCP calls, an open terminal) asked GitHub meanwhile.
- A process with the person's workspace token can also report redactions, which writes at most
  one audit entry per person, sandbox, secret and channel while Yulius runs.
- Any process with a person's workspace token (their terminals too) can read the fingerprints of
  their sandbox's agent secrets: salted hashes a task agent started by the same person gets anyway.
- The Superagent's conversation is kept in the person's own home (`~/.claude/projects`), which
  their plain terminal reads without redaction. What passes the in-session layers (an MCP tool
  error, a raw result Claude Code keeps) can be written there in clear. That only adds an
  accidental way to see a value: a person bent on it already could transform one past the
  exact-value redaction (`printenv … | rev`), as an agent could.
- A server imported from `.mcp.json` is named both in `disabledMcpjsonServers` and, by the same
  name, in `--mcp-config`. Measured on Claude Code 2.1.292 (SPEC-147): the inline server starts
  and the rejected project server does not.
- Literal MCP values remain readable in the session (`echo $YULIUS_MCP_…`), as in task runs; a
  follow-up could redact them in both.
