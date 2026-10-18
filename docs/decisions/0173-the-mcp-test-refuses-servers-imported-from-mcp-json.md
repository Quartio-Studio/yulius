# 0173. The MCP Test refuses servers imported from .mcp.json until a person changes what they run

## Status

Accepted. Specified in
[SPEC-218](../../.ai/specs/SPEC-218-fix-654-mcp-stdio-test-must-not-run-agen.md) (#654). Amends
[0078](0078-repository-mcp-json-feeds-the-mcp-section.md) (imported servers are marked).

## Context

Since 0078, Yulius imports the servers of the sandbox repository's `.mcp.json` as enabled sandbox
servers before each run. Agents write that file. The MCP Test of a stdio server spawns its command
on the Yulius host, outside any containment, as the service user; on a contained instance only an
instance administrator may press it, because that gate assumed a person chose the command. An
imported server looked exactly like a person's, so an administrator opening
`…/mcp/<id>?test=1` (the edit page tests on load) ran an agent's command on the host: the instance
key, the database, every sandbox (#654, found by the Vidoc audit of SPEC-217). The Test of an
imported HTTP server likewise POSTed to a loopback or private URL an agent chose.

The stored environment of an imported stdio server also comes from the file, and the edit page
keeps stored values it never shows unless the person replaces them. A person who changed only the
command would still run it with an agent's `LD_PRELOAD` or `NODE_OPTIONS`.

## Decision

- Every MCP server has an `origin`: `person` or `mcp_json` (migration 0037; servers named by an
  `mcp.imported` audit entry are marked `mcp_json`, also if a person edited them since). The import
  stores `mcp_json`, the API's create `person`; `McpServerView` returns it.
- The Test of a stdio server from `.mcp.json` is refused without starting anything, for everyone
  and whether or not the instance is contained; the result says why and what lifts it. The Test of
  such an HTTP server is refused when its URL resolves to a private or loopback address; public
  addresses are tested as before.
- An update makes the server `person` only when it changes what the server runs: a stdio server's
  command or arguments **together with** a replaced environment (or none stored), or an HTTP
  server's URL (or a new transport). Re-sending the stored fields, as the enable toggle does,
  changes nothing; the edit page re-sends the arguments exactly, empty ones included. A take-over
  that does not replace the environment (one through another transport) drops the stored one, so
  the file's values never reach a later local command. A `person` server never becomes `mcp_json`
  again.
- The MCP list badges such a server "From .mcp.json" and disables its stdio Test; the edit page
  explains it and does not run the Test on load.

## Alternatives

- **Run the stdio probe inside the sandbox's containment**, as a terminal runs. It would keep the
  Test for imported servers, but needs the bwrap launch, the sandbox's egress proxy and its user
  mapping for a one-shot JSON-RPC exchange: a larger change with its own failure modes. It remains
  the way to test such servers later; the refusal does not prevent it.
- **Look up the audit log at Test time.** The log is history, not state: a person's change could
  not lift the refusal without another table, and a lost entry would silently allow the Test.
- **Refuse only on contained instances.** Uncontained, agents already run on the host, so the
  refusal costs nothing there, and one rule is simpler to explain and to test.
- **Take a server over on any save by a person.** The enable toggle re-sends the command, and a
  save of an unrelated field shows nothing about the hidden environment; requiring a change of what
  runs keeps the person's attention on it, as the issue asks.

## Consequences

- A command an agent wrote is never run on the host by the Test. Imported servers still reach agent
  runs as before (inside the run's containment when contained).
- To test an imported stdio server, a person checks its whole command line, changes it and
  re-enters its environment (or deletes it and adds it themselves). Changing one argument is
  enough for the rule, so the page asks the person to check the whole line; the rule keeps the
  person's attention on what runs, it does not prove they read it. Servers imported before this change, including ones a
  person already edited, need that once.
