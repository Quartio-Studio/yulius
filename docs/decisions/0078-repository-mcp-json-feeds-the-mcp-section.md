# 0078. A repository's .mcp.json feeds the sandbox's MCP servers; the Superagent rejects the file's servers

## Status

Accepted. Specified in
[SPEC-095](../../.ai/specs/SPEC-095-while-onboarding-other-project-i-got-som.md). Amends SPEC-037
MGT-004 (the Superagent gets one setting) and SPEC-002 §7.2 (how MCP values reach a run).

## Context

A project onboarded with a `.mcp.json` (supabase, apify, pipedream-gmail) opened the Superagent on
Claude Code's "3 new MCP servers found in this project" dialog. The Superagent runs `claude` in
the sandbox repository without `--strict-mcp-config`, so that the person's own servers keep
working (MGT-004). Claude Code therefore reads the repository's `.mcp.json` and asks about every
server not yet approved or rejected. Task runs are strict, so they never ask, and they never get
those servers either: Yulius did not read the file, and its MCP section did not list them. The
person asked for no interaction and for the servers to be connected through Yulius's MCP section.

Measured on Claude Code 2.1.292: the answers are settings. `--settings
'{"disabledMcpjsonServers":[…]}'` naming every server starts without the dialog and writes
nothing to the repository; `{"enableAllProjectMcpServers": false}` still asks.

The import also exposed a clash. A stdio server's value travelled in the run's environment under
the server's own variable name, such as `APIFY_TOKEN`. An agent secret of that name already puts
its placeholder there, and the session host refuses the duplicate, so every run in the sandbox
failed to spawn. The import makes this the normal case, because `${APIFY_TOKEN}` maps to the agent
secret `APIFY_TOKEN`.

## Decision

- Before every task run and every Superagent start, Yulius reads the sandbox repository's
  `.mcp.json` (at most 256 KiB and 50 servers). Each stdio or http server whose name the sandbox
  and the instance lack becomes an enabled sandbox server, audited `mcp.imported` by `yulius`.
  Whole `${VAR}`, `${VAR:-default}` and `Bearer ${VAR}` values become `${secret:VAR}` references
  to the agent secrets. A server already configured is never touched, so turning a server off in
  Yulius keeps it off. Nothing here stops a run: errors are logged and skipped.
- The Superagent starts with `--settings` whose only key is `disabledMcpjsonServers`, naming every
  server in the file, the skipped ones too. Yulius is the one place those servers are configured.
- A stdio server's values travel in `YULIUS_MCP_<SERVER>_<VAR>` variables, as an HTTP bearer
  already did in `YULIUS_MCP_<SERVER>_TOKEN`. The run's `mcp.json` maps each one back to the
  server's own variable name. Names that still meet get a numbered suffix.

## Alternatives

- **Approve the file's servers in the Superagent** (`enabledMcpjsonServers`): there would be no
  dialog, but the Superagent would run the file's servers outside Yulius, without the vault's
  secrets, and a server turned off in Yulius would still run there.
- **`--strict-mcp-config` for the Superagent**: no dialog, but the person's own MCP servers would
  stop working, which MGT-004 kept on purpose.
- **Import once, at onboarding**: projects already onboarded, and servers added to the file later,
  would never arrive, and the dialog would come back for each new server. The cost of importing at
  every start: a server deleted in Yulius comes back while the file still lists it. Turning it off
  is the way to keep it out, and that is recorded.
- **Write the answers into the repository's `.claude/settings.local.json`** (where the dialog
  stores them): this edits the person's checkout, and it is shared by every user of the sandbox.
- **Drop the agent secret's placeholder when an MCP variable has its name**: the agent's own
  environment would then hold the real value instead of the placeholder that only the egress proxy
  replaces.

## Consequences

- A project's `.mcp.json` is enough to give every task its servers. The person adds the agent
  secrets the file names (for example `APIFY_TOKEN`) in Secrets. Until then, a server runs without
  that value and a warning is logged.
- The Superagent still gets only the `yulius` server from Yulius. Giving it the sandbox's servers
  means passing their secrets to a session without the redaction hook, which is a follow-up
  (#395).
- Values that name a variable inside other text (in arguments, a URL, or `prefix-${VAR}`) are
  imported as written. Claude Code expands them from the run's environment, where Yulius sets
  nothing for them.
- Run environments change names: a stdio server's value is no longer in the agent's environment
  under the server's variable name. That name now holds the agent secret's placeholder, if there is
  one.
