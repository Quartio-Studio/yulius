---
yulius-task: 1462ec26-59b9-4f46-b3cf-1311cb2c4da3
status: in-progress
---

# SPEC-485: a Yulius session runs no plugin language server and is not asked to install one

Status: in-progress
Date: 2026-11-13
Owner: Yulius task #467
Scope authorization: the request of task #467 (2026-11-13): "fix such errors like the yellow
one", about the notice `Language server "plugin:rust-analyzer-lsp:rust-analyzer" couldn't start.
The Errors tab in /plugin may have the reason.` in a task's terminal. Decision in
[ADR-0327](../../docs/decisions/0327-a-run-decides-its-plugin-language-servers-and-they-are-off.md).

## Goal

A person reading a task (or a Superagent) sees the agent's work, not Claude Code's notices or
questions about a language server: no session started by Yulius starts a plugin's language
server, reports that one could not start, or is asked whether to install one. What a session
runs is decided by Yulius for each run, not by a file every session and every dialog can change.

## Context

Everything below was measured with the installed Claude Code 2.1.323 run in a terminal against a
local stand-in for the model API (no account, no provider request), in this sandbox, with the
sandbox's own plugin folder copied; scripts and logs are in the Evidence section.

- **The notice and its cause.** Claude Code starts a plugin's language server the first time a
  session writes or edits a file of the server's language (a read does not start it). The
  official plugin `rust-analyzer-lsp` runs the program `rust-analyzer`, here rustup's proxy
  `~/.cargo/bin/rust-analyzer`, which picks a toolchain from where it is started. With the
  plugin enabled and the active toolchain lacking the `rust-analyzer` component, the server
  exits at once: `error: Unknown binary 'rust-analyzer' in official toolchain
  'stable-x86_64-unknown-linux-gnu'.`, exit code 1 (Claude Code's log: `LSP server
  plugin:rust-analyzer-lsp:rust-analyzer crashed with exit code 1`), and the terminal shows the
  notice word for word. With the program missing from `PATH` the notice has another text ("its
  program "rust-analyzer" wasn't found"), so the session the person saw had the proxy on its
  `PATH`, as task sessions have.
- **How the plugin got there.** When a session writes a file whose language server's program is
  on `PATH` and the plugin is not installed, Claude Code opens a dialog in the terminal, "LSP
  plugin recommendation … Would you like to install this LSP plugin?". Its default answer is
  yes: one Enter installs the plugin for the user (`plugins/installed_plugins.json`, scope
  `user`) and writes `"enabledPlugins": {"<plugin>": true}` into the home's `settings.json`. A
  sandbox's sessions share one home, so from then on every session of the sandbox starts that
  server. This sandbox's plugin was installed that way on 2026-10-12; it holds for every
  language whose server is on `PATH` (pyright, gopls, typescript-language-server, clangd…).
- **What it cost when it worked.** On 2026-10-13 a person's session found "a new rust-analyzer
  instance per worktree for each agent session", 20 GB in all with OOM kills in the sandbox
  (SPEC-082), and set the plugin to `false` in the home's `settings.json`. Measured again here
  on this repository (4 minutes, one server): 2.6 GiB for the server alone, 6.2 GiB with its
  children, 109 threads, and a `cargo check` of the whole workspace in the worktree's own
  `target/` after a saved file (80 s), which holds the build directory's lock while the agent's
  own cargo waits. Concurrent cargo runs break the gate's doctest step in this repository.
- **Which setting wins.** Only `true` starts the server; a missing entry does not. The entry is
  read from every settings layer, and the run's `--settings` (which Yulius writes for every
  session) outrank the home's file both ways: with `true` in the home and `false` in
  `--settings` no server is loaded; with `false` in the home and `true` in `--settings` it is
  loaded and the notice appears. A repository's `.claude/settings.json` and a worktree's
  `.claude/settings.local.json` outrank the home's file the same way. So `false` in the home's
  file is no decision: any higher layer, a `/plugin` in a terminal or the next dialog changes
  it for every session.
- **Today's state of this sandbox.** The plugin is installed and `false` in the home's file,
  which also keeps the dialog away (an installed plugin is not offered). A new task session
  here starts no language server (seen in this task's own session: a `.rs` file written, no
  server process). Which session showed the person the notice on 2026-11-13 cannot be told
  from inside the sandbox: Claude Code does not write the notice to a transcript, terminals are
  kept on the host, and no settings file in the sandbox enables the plugin now.
- **What exists to build on.** `ensure_claude_config` (yulius-harness) merges Yulius's keys into
  a config dir's `.claude.json` before every launch (onboarding, trust, the diff panel) through
  handles that follow no link (ADR-0174). `plan_launch` writes each task run's
  `claude-settings.json` and `plan_cockpit` a Superagent's `--settings`; both already carry
  settings that must outrank the home's file (`autoCompactEnabled`). MCP servers are already
  only the ones the run names (`--strict-mcp-config`).

## Requirements

- LSP-001 (no offer). `ensure_claude_config` sets `"lspRecommendationDisabled": true` in the
  config dir's `.claude.json` at every launch it prepares (task runs, Superagents). Claude Code
  then logs "Recommendations are disabled" and opens no dialog.
- LSP-002 (the plugins that bring a language server).
  `yulius_harness::claude_plugins::language_server_plugins(config_dir)` returns the ids
  (`name@marketplace`) of the plugins installed in a config dir that bring a language server,
  sorted. A plugin brings one when any of these names `lspServers` with a value other than
  `null`, or is the file itself: its entry in
  `plugins/marketplaces/<marketplace>/.claude-plugin/marketplace.json`; `.claude-plugin/plugin.json`
  in one of its folders `plugins/cache/<marketplace>/<name>/<version>/`; a file `.lsp.json` in
  such a folder. The installed plugins are the keys of `plugins/installed_plugins.json`.
  Everything is read through handles opened one name at a time without following a link, from
  the folder above the home (as ADR-0174 does); a file is read up to 8 MiB, at most 256 plugins
  and 16 version folders each are looked at; an id whose name or marketplace is not a plain file
  name is skipped. Nothing is created or written. What cannot be read (missing, a link, not
  JSON) brings nothing: the function never fails a launch.
- LSP-003 (off in every run). `LaunchRequest` and `CockpitRequest` carry `disabled_plugins`; the
  task run's settings file and the Superagent's `--settings` then hold
  `"enabledPlugins": {"<id>": false}` for each, and no such key when the list is empty. The
  wiring fills the list from LSP-002 for the session's config dir before each launch. The
  run's settings outrank the home's, a repository's and a worktree's settings, so the server
  is neither loaded nor started nor reported, whatever those say.
- LSP-004 (the check with the real tool). An opt-in live test (`YULIUS_LIVE_CLAUDE=1`, as the
  other checks of the real CLI) builds a config dir in which a language-server plugin is
  installed and `true` in the home's `settings.json`, asks the installed `claude` for its
  plugins (`claude plugin list --json`) and expects the plugin enabled; then asks again with
  the `--settings` Yulius plans for a run of that config dir and expects it disabled. It needs
  no account and makes no provider request.
- LSP-005 (documents). ADR-0327 records the cause and the decision; the architecture document's
  section on the Claude Code config and launch says what a run switches off; the operations
  guide says what an existing instance needs (the update, nothing in a sandbox's home) and how
  to see a session's plugins; the changes log has an entry.

Out of scope, with the reason: turning a language server on for a project with a budget and a
build directory of its own (an alternative ADR-0327 weighs and leaves to its own task); other
plugins a person installs in a sandbox's home (skills, commands: no notice of this kind was
found for them); Claude Code's own notices about limits and goals (listed under Decisions).

## Plan

- [x] The cause reproduced with the real CLI, related work read, the spec written and validated
- [x] Claude Code no longer offers to install a language server in a Yulius session (LSP-001)
- [ ] Yulius finds the installed plugins that bring a language server (LSP-002)
- [ ] Every task run and Superagent switches those plugins off in its own settings (LSP-003)
- [ ] The live check with the real CLI, and real sessions shown without the notice (LSP-004)
- [ ] ADR-0327, the architecture document, the operations guide and the changes log (LSP-005)
- [ ] Every gate green and the independent review in a fresh session answered

## Acceptance criteria

- With a language-server plugin installed and `true` in the home's `settings.json`, the run
  settings Yulius plans make the real `claude` report it disabled, and a real session that
  writes a file of its language shows no "couldn't start" notice and starts no server; without
  those settings the same session shows the notice (the control).
- With no such plugin installed and the server's program on `PATH`, a real session whose config
  Yulius prepared shows no "LSP plugin recommendation" dialog when it writes such a file;
  without the preparation it does (the control).
- A task run's settings file and a Superagent's `--settings` hold `enabledPlugins` with `false`
  for exactly the installed plugins that bring a language server, by each of the three ways a
  plugin can bring one, and nothing for other plugins.
- A link, a FIFO or an oversized file anywhere in the plugin folder never blocks or fails a
  launch and is never followed; nothing in the config dir is created or changed by the lookup.
- A new task session of this project runs without the notice (shown from a real session).

## Test plan

- Unit tests in `claude_plugins.rs`: each of the three sources; `null` and a missing key; another
  plugin without a server; several versions; a plugin folder, a marketplace file and the plugins
  folder that are links (not followed, a file outside stays unread); a FIFO where a file is
  expected (no block); an oversized file; ids with `/` or `..`; a missing config dir (nothing
  created); the bounds.
- Unit test in `claude_config.rs`: `lspRecommendationDisabled` is set, kept on a second run, and
  other keys stay.
- `crates/yulius-harness/tests/launch.rs`: the task run's settings and the Superagent's
  `--settings` with and without `disabled_plugins` (also the Global Superagent's).
- `crates/yulius/src/wiring/harness.rs` tests: a launch planned for a home whose plugin folder
  holds a language-server plugin carries it switched off; the Superagent's the same.
- Live (`YULIUS_LIVE_CLAUDE=1`): LSP-004.
- Real sessions (recorded in Evidence): the probe runs of the real CLI in a terminal with and
  without Yulius's settings and config (notice, dialog), and this task's own session.
- Mutation checks: the `enabledPlugins` insertion removed, the `lspRecommendationDisabled`
  insertion removed, a no-follow flag dropped: each fails a test.

## Decisions

- **Off, for every session Yulius starts, decided per run** (ADR-0327). The reasons: a server per
  session (2.6 GiB at rest here, 6.2 GiB with its checks; 20 GB and OOM kills in October with
  parallel tasks); its `cargo check` in the worktree's `target/` at every saved file competes
  with the agent's own cargo for the build lock and breaks the gate's doctest step; whether it
  can start depends on a program and a toolchain Yulius does not provision; and the agent gets
  the same diagnostics from the checks it runs anyway. The plugin was used while it ran (411
  requests in a day and a half), so navigation has value; turning it on per project, with a
  budget and a build directory of its own, is left to a task that can pay for that.
- **In the run's settings, not in the home's file.** The home's `settings.json` is the lowest
  layer and shared: a repository's settings, a `/plugin` in any terminal or a dialog's Enter
  change it for all. The run's `--settings` outrank those and are written by Yulius alone.
- **Only plugins that bring a language server.** Other plugins in a sandbox's home (skills,
  commands) stay as the person set them: no notice of this kind was found for them, and
  switching all plugins off would be a product decision nobody asked for.
- **"Brings a language server" is read from three places, by the key's presence.** Claude Code
  takes `lspServers` from a marketplace entry, a plugin's manifest and `.lsp.json`. Yulius does
  not mirror their grammar: a key that is there switches the plugin off, valid or not (an
  invalid one would only add an error to the same tab).
- **The lookup never fails a launch and fails open.** It is tidiness, not a boundary: a language
  server runs inside the session's containment like any program the agent starts. An agent
  that hides a plugin behind a link gets a server only for its own sandbox.
- **The dialog is switched off by Claude Code's own key**, in `.claude.json` where it keeps it,
  merged like the other keys of `ensure_claude_config`.
- **The installed plugin stays in this sandbox's home.** Removing it would bring the dialog back
  for sessions the current server starts until the update is deployed; switched off per run it
  is inert.
- **Other notices of the class, looked for in the last three days' transcripts of this sandbox
  (175 files) and in Claude Code's start-up log here.** Found, none of them tooling that Yulius
  set up and that does not work: "Goal paused …" and "Usage limit reached …" (62 notices: limits,
  handled by the limit handling and task #468); "A hook blocked the turn from ending N
  consecutive times" (28: Claude Code's cap on its own `/goal` check, by design); "Hook evaluator
  API error: You've hit your … limit" (13: the `/goal` check itself refused by the usage limit;
  the cause is the limit); "Compacted while idle" (12: information). Yulius's own Stop-hook
  answers (7) are its messages to the agent. The start-up log of a session with this sandbox's
  config has no warning or error about hooks, the status line, MCP servers or plugins. Not
  looked at, because out of reach from inside this sandbox: other tasks' terminals (notices are
  not written to transcripts) and other projects' sandboxes; the language-neutral rule covers
  their language servers.
