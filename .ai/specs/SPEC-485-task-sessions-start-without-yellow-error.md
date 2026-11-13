---
yulius-task: 1462ec26-59b9-4f46-b3cf-1311cb2c4da3
status: done
---

# SPEC-485: a Yulius session runs no plugin language server and is not asked to install one

Status: done
Date: 2026-11-13
Owner: Yulius task #467
Scope authorization: the request of task #467 (2026-11-13): "fix such errors like the yellow
one", about the notice `Language server "plugin:rust-analyzer-lsp:rust-analyzer" couldn't start.
The Errors tab in /plugin may have the reason.` in a task's terminal. Decision in
[ADR-0327](../../docs/decisions/0327-a-run-decides-its-plugin-language-servers-and-they-are-off.md).

## Goal

A person reading a task (or a Superagent) sees the agent's work, not Claude Code's notices or
questions about a language server: a session started by Yulius starts no language server of a
plugin Yulius can find, so reports none that could not start, and is not asked whether to
install one. What a session runs is decided by Yulius for each run, not by a file every session
and every dialog can change.

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
  `yulius_harness::claude_plugins::language_server_plugins(config_dir, sandbox_dir, worktree)`
  returns the ids (`name@marketplace`) of the plugins a session with that config dir, started in
  that worktree, would load a language server from, sorted, and what it left unread.
  - Candidates: the keys of `plugins/installed_plugins.json`, and the plugins that the home's
    `settings.json`, the worktree's `.claude/settings.json` or its `.claude/settings.local.json`
    switch on (`enabledPlugins`, any value but `false`): Claude Code loads such a plugin in place
    from a marketplace that is a folder, with no install record.
  - A marketplace lies where `plugins/known_marketplaces.json` says (`installLocation`); else
    where one of those three settings files names it as a folder (`extraKnownMarketplaces`,
    source `directory`, a relative path being the worktree's), which Claude Code registers
    during a session and, in a terminal, loads from in that same session; else at
    `plugins/marketplaces/<marketplace>`.
  - A plugin brings a language server when its entry in the marketplace's
    `.claude-plugin/marketplace.json` names `lspServers` with a value other than `null`, or one
    of its folders holds a file `.lsp.json` or a `.claude-plugin/plugin.json` that names
    `lspServers` so. Its folders: `plugins/cache/<marketplace>/<name>/<version>`, each install
    record's `installPath`, and the entry's relative `source` in the marketplace (`./path`, or a
    bare name below `metadata.pluginRoot`).
  - Reading: through handles opened one name at a time without following a link, from the
    folder above the home (as ADR-0174 does). A path the agent wrote (`installLocation`,
    `installPath`, the worktree) is opened only below the sandbox directory or the folder above
    the home; `..` in a `source` is not walked; an id is taken only when its name and its
    marketplace are names as Claude Code takes them (letters, digits, `.`, `_`, `-`), each one
    plain file name. Nothing is created or written.
  - Bounds, for the whole lookup: 16 MiB read (whatever a file holds), 2 MiB a file, 1024
    plugin folders, 256 plugins of at most 200 bytes each, 16 version folders a plugin;
    `.lsp.json` is not read. The ids' bound and characters keep what a run's settings grow by
    (about 54 kB) below what one argument of a command line may have.
  - What cannot be read (a link, a special file, not JSON, past a bound, a marketplace whose
    folder cannot be opened, an id that is not two such names) brings nothing and is named in
    the result's `unread` (at most 8 notes); a missing file is no note. The function never fails a
    launch.
- LSP-003 (off in every run). `LaunchRequest` and `CockpitRequest` carry `disabled_plugins`; the
  task run's settings file and the Superagent's `--settings` then hold
  `"enabledPlugins": {"<id>": false}` for each, and no such key when the list is empty. The
  wiring fills the list from LSP-002 for the session's config dir and worktree before each
  launch, and logs what it switches off (info) and what the lookup left unread (warn). The
  run's settings outrank the home's, a repository's and a worktree's settings, so for the
  plugins found the server is neither loaded nor started nor reported, whatever those say. A
  plugin that brings a language server and other things (skills, commands) is switched off
  whole: Claude Code has no switch for a plugin's language server alone.
- LSP-004 (the check with the real tool). An opt-in live test (`YULIUS_LIVE_CLAUDE=1`, as the
  other checks of the real CLI) runs the installed `claude` in eight layouts: a cloned
  marketplace with installed plugins; a marketplace that is a folder (as `claude plugin
  marketplace add <folder>` leaves it); plugins switched on with no install record; switched on
  by the repository's settings and by the worktree's local settings; installed copies outside
  the cache that alone say two plugins bring a server; a marketplace the repository brings
  (`extraKnownMarketplaces`) by an absolute and by a relative path, the run's settings planned
  before it is registered. In each, three
  plugins bring a server by the three ways and a fourth brings none. The control shows that the
  CLI loads the three servers (its debug log); with the `--settings` Yulius plans for a run of
  that home and worktree it loads none, and `claude plugin list --json` shows the fourth still
  on. It needs no account and makes no provider request.
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
- [x] Yulius finds the installed plugins that bring a language server (LSP-002)
- [x] Every task run and Superagent switches those plugins off in its own settings (LSP-003)
- [x] The live check with the real CLI, and real sessions shown without the notice (LSP-004)
- [x] ADR-0327, the architecture document, the operations guide and the changes log (LSP-005)
- [x] Every gate green and the independent review in a fresh session answered

## Acceptance criteria

- With a language-server plugin installed and `true` in the home's `settings.json`, the run
  settings Yulius plans make the real `claude` report it disabled, and a real session that
  writes a file of its language shows no "couldn't start" notice and starts no server; without
  those settings the same session shows the notice (the control).
- With no such plugin installed and the server's program on `PATH`, a real session whose config
  Yulius prepared shows no "LSP plugin recommendation" dialog when it writes such a file;
  without the preparation it does (the control).
- A task run's settings file and a Superagent's `--settings` hold `enabledPlugins` with `false`
  for exactly the session's plugins that bring a language server, by each of the three ways a
  plugin can bring one and in each layout of LSP-004, and nothing for other plugins.
- A link, a FIFO or an oversized file anywhere in the plugin folder never blocks or fails a
  launch and is never followed; nothing in the config dir is created or changed by the lookup;
  the whole lookup reads at most 16 MiB whatever the agent laid out.
- A new task session of this project runs without the notice (shown from a real session).

## Test plan

- Unit tests in `claude_plugins.rs`: each of the three sources; `null` and a missing key; another
  plugin without a server; several versions; a marketplace kept in a folder of its own (below
  the sandbox directory: read; outside it, behind a link or through `..`: not opened); plugins
  switched on by each of the three settings files with no install record, found by the entry,
  by the `source` folder and below `metadata.pluginRoot`, a `source` with `..` not walked, a
  plugin set to `false` no candidate; an `installPath` inside and outside the sandbox
  directory; a plugin folder, a marketplace file, the plugins folder, the config dir, the home
  and a worktree's `.claude` that are links (not followed, a file outside stays unread); a FIFO
  where a file is expected (no block); an oversized file; ids with `/` or `..`; a missing
  config dir (nothing created); each bound and the whole lookup's budget (256 marketplaces of
  2 MiB catalogs and 4096 manifests of 2 MiB: at most 16 MiB read, 1024 folders looked into),
  with the notes about what was left unread.
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
  switching all plugins off would be a product decision nobody asked for. A plugin that brings
  a server and other things is switched off whole; Claude Code offers nothing finer.
- **"Brings a language server" is read from three places, by the key's presence.** Claude Code
  takes `lspServers` from a marketplace entry, a plugin's manifest and `.lsp.json`. Yulius does
  not mirror their grammar: a key that is there switches the plugin off, valid or not (an
  invalid one would only add an error to the same tab).
- **Which plugins and where their marketplace lies is read as the real tool does it** (review
  round 1). The first lookup read install records and cloned marketplaces only; the reviewer's
  probes of the real CLI showed servers still loaded from a marketplace that is a folder and
  from plugins switched on with no install record. The lookup now follows
  `known_marketplaces.json` and the settings files, and the live test compares it with the real
  CLI in each layout, control first.
- **A path the agent wrote is opened only inside the sandbox's folders.** `installLocation` and
  `installPath` are text in files the agent writes; the host follows them below the sandbox
  directory and the folder above the home, one name at a time without following a link, and
  nowhere else. A session's own folders all lie there.
- **One budget for the whole lookup** (review round 1). Bounds per file and per plugin
  multiplied: 256 catalogs kept parsed and 4096 manifests of 8 MiB took 370 s and 32 GiB in
  the server. 16 MiB read and 1024 folders in all keep it to a fraction of a second, on the
  launch's own thread as the config preparation is.
- **The lookup never fails a launch and fails open, and says so.** It is tidiness, not a
  boundary: a language server runs inside the session's containment like any program the agent
  starts. An agent that hides a plugin behind a link gets a server only for its own sandbox.
  What was left unread is logged with the launch, so a notice that still appears can be traced.
- **A repository's own marketplace is read from the settings that bring it** (review round 2).
  The first rework waited for Claude Code to register it, on a headless measurement ("the
  first session loads nothing"). In a terminal, which is what Yulius runs, the registering
  session loads its language servers too (`Total LSP servers loaded: 0`, then `3`), and so
  does the first session of each new worktree.
- **Not covered, by decision.** A marketplace or an installed copy outside the sandbox's folders
  (the host does not open a path the agent names there); for a contained Superagent that
  includes a folder marketplace or a copy in the person's own home, which its Claude Code
  names by the path it sees there (`/home/<login>/…`), not by the host's; a plugin installed
  or switched on while a session runs (off from the next launch); plugins given on a command
  line (`--plugin-dir`: Yulius passes none). In these a server can still start; a
  marketplace whose folder was not opened is named in the launch's warning, and the
  operations guide says how to see the rest.
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

## Evidence

Run in this task's worktree and sandbox on 2026-11-13 with Claude Code 2.1.323. Scripts and logs
are under the ignored `target/task467/` (the probe scripts also in `$HOME/.cache/yulius-467/`).

**The cause, with the real CLI.** `probe/drive.py` runs the installed `claude` in a terminal
against a local stand-in for the model API whose "model" writes one `.rs` file; the config dir
holds a copy of this sandbox's plugin folder. `probe/shapes.py` builds a marketplace of its own.

| Run | Plugin state | Result |
| --- | --- | --- |
| a | `true` in the home's `settings.json`, the proxy on `PATH`, no toolchain file | the notice, word for word; log: `error: Unknown binary 'rust-analyzer' in official toolchain 'stable-x86_64-unknown-linux-gnu'.`, `crashed with exit code 1` |
| b | `true`, `PATH` without `~/.cargo/bin` (also with Yulius's shell prefix kept) | another notice: `its program "rust-analyzer" wasn't found. Check that it's installed and on your PATH.` |
| — | `false` (this sandbox's state), also with the sandbox's own `.claude.json` | `Total LSP servers loaded: 0`, no notice |
| — | no entry | not loaded |
| d | `true` in the home, `false` in `--settings` | `Total LSP servers loaded: 0`, no notice |
| e | `false` in the home, `true` in `--settings` | loaded, started, the notice |
| f | not installed, the proxy on `PATH` | the dialog "LSP plugin recommendation"; an Enter: `User response: yes`, the plugin in `installed_plugins.json` (scope `user`) and `true` in `settings.json` |
| f3 | as f, `lspRecommendationDisabled: true` in `.claude.json` | no dialog; log: `[lspRecommendation] Recommendations are disabled` |
| g | installed, `false` | no dialog (`Skipping … (already installed)`) |
| shapes | servers named in the marketplace entry, in the manifest, in `.lsp.json` | `Loaded 1 LSP server(s) from plugin:` for each of the three |

A read alone starts no server (a run whose "model" only read the file loaded the server and
never started it).

**What a working server costs here.** `ra_measure.py` (an LSP client: initialize, one file
opened and saved, 4 minutes) in this worktree: `rust-analyzer peak RSS 2.60 GiB; with children
6.18 GiB; threads 109`; first diagnostics after 9 s; `cargo check` (flycheck) of the workspace
from second 0 to 80, with `cc` and build scripts in the worktree's `target/`.

**Transcripts.** `scan_notices.py` over the 175 transcript files of the last three days: the
counts in Decisions. No record holds the language-server notice (Claude Code does not write it).

**Tests.**
- `cargo nextest run -p yulius-harness`: 168 passed. New: 11 in `claude_plugins`, 1 in
  `claude_config`, 2 in `tests/launch.rs`; each was seen failing first (the first lookup against
  a stub that finds nothing: 4 of 6 failed, the other two assert absence and got a positive
  control or are about a missing folder).
- `cargo nextest run -p yulius -E 'test(/wiring::harness::/)'`: 6 passed; the new one failed
  with the list left empty (`left: Null`).
- Mutations, each on the committed tree and reverted. First round: `plugins_off` never
  returning a value (2 tests fail); the `lspRecommendationDisabled` line removed (1); every id
  taken as a plain name (1: `..` reaches a folder outside); `O_NOFOLLOW` dropped from the folder
  flags (5, the new link test among them). On the reworked lookup: the bytes budget never
  charged (1); the folder budget never charged (1); the worktree's local settings not read (1);
  ids of any length (1); `..` in a `source` walked (1); a path outside the sandbox's folders
  opened as written (2). One mutant survived and is harmless: a path outside taken as relative
  to the sandbox directory opens nothing outside it. On what the wiring hands to the lookup
  (round 2): a task run handing the home over as the worktree (1 test fails), a Superagent
  handing the sandbox directory over as its working directory (1), or its config dir as the
  sandbox directory (1). A task run handing the home over as the sandbox directory survives
  and is the same program: the lookup takes the folder above the home, which is the sandbox
  directory of a task run, by itself.
- Live, against the real CLI: `YULIUS_LIVE_CLAUDE=1 cargo nextest run -p yulius -E
  'binary_id(yulius::main) & test(/^live_claude::a_runs/)' --run-ignored only`: passed (5.4 s).
  In each of the eight layouts the CLI alone loads the three servers; with the run's settings
  none. With the plugins' folders gone from a cloned marketplace the CLI loads two of them from
  the copies their install records name, which only `installPath` finds; with a marketplace
  that is a folder it loads in place and ignores the copies (seen as a failed control while
  writing the layout).
- The whole lookup on the layout the reviewer measured at 370 s and 32 GiB with the first
  bounds (256 marketplaces, 4096 manifests, here 2 MiB each): the unit test runs in 0.2 s
  (debug build) and reads at most 16 MiB.

**Real sessions with this build's preparation** (`real-sessions.log`, `sessions/`): the same
full sessions as above (terminal, the stand-in model writes `src/extra.rs`), the config dir
prepared and the run's settings planned by this branch's `HarnessAdapter::plan`.

| Session | Notice | Dialog | Server started |
| --- | --- | --- | --- |
| plugin `true` in the home, as before this change | yes | no | yes (and exits) |
| plugin `true` in the home, Yulius's run settings (`enabledPlugins = {"rust-analyzer-lsp@claude-plugins-official": false}`) | no | no | no |
| plugin not installed, as before this change | no | yes | no |
| plugin not installed, Yulius's config (`lspRecommendationDisabled = True`) | no | no | no |

In all four the file was written (`Wrote 3 lines`, two model requests). The two sessions with
Yulius's preparation were run again with the final build (`real-sessions-2.log`): the same.

**A real task session of this project.** This task's own session (started by the deployed
server at 16:23 UTC, Claude Code 2.1.323): after two `.rs` files written with the Write tool,
the session's process has no language server among its children and the sandbox no
`rust-analyzer` process; `claude plugin list` there: `rust-analyzer-lsp@claude-plugins-official
… Status: ✘ disabled`.

**Gates.** `scripts/dev ci` at 6539ce637, after the second review round's fixes: exit 0; `4221
tests run: 4221 passed, 17 skipped` (`gate4.log`). After it dev was merged once more (#1254, CI
code): `scripts/dev check` exit 0, the two changed crates' tests and the live test passed
again, and only this section changed; the whole gate on that head is the reviewer's run and
Yulius CI's. Earlier: at 2fd99c346 with #1253 merged 4220 passed (`gate3.log`); at 49d9ebfac
4217 (`gate2.log`); before the review, at e877462e9, 4213 (`gate.log`). `yulius security-scan`: "Security scan disabled: Vidoc is
turned off in Yulius, for this project or the whole instance; nothing was scanned." (disabled,
not passed). The end-to-end suite was not run: no web UI change.

**Independent review, round 1** (a fresh session given the diff, the spec and the review skill;
head e877462e9; its notes and probes in `$HOME/.cache/yulius-467-review/`). Verdict: "**REQUEST
CHANGES** — two major findings in the change's own code: the guarantee the documents state does
not hold in layouts the real CLI produces (F1), and the lookup's bounds multiply into minutes
and tens of GiB on the host (F2)." It ran the gate itself (exit 0), the live test, 14 mutants
(all killed) and differential probes of the real CLI. All findings were own code; each answered:
- F1 (major): "a language server still loads with Yulius's settings when the marketplace is a
  directory source", and with plugins switched on with no install record. Fixed: the lookup
  reads candidates from the settings files too and a marketplace where
  `known_marketplaces.json` says; the live test now runs those layouts, control first. The
  documents name what stays uncovered.
- F2 (major): "each bound is fine alone, their product is not" (370 s, 32.1 GiB). Fixed: one
  budget for the whole lookup, tested on that layout.
- F3 (minor): an id of 200 kB made a Superagent's `--settings` argument too long to start.
  Fixed: ids over 200 bytes are left out and said.
- F4 (minor): the spec named an Evidence section it did not have. This section.
- F5 (minor): a plugin with a server and skills is switched off whole. Now said in the spec,
  ADR-0327 and the operations guide.
- F6 (minor): "the fail-open is silent". Fixed: the launch logs what it switched off and what
  it left unread.
- Nits: `.lsp.json` is now a `stat`; the test comment and the spelled-out type are gone.

**Independent review, round 2** (the same reviewer, head 608e1f44a; it ran the gate, the live
test, 18 mutants of the lookup and its probes again). Verdict: "**REQUEST CHANGES** — one major
finding in the change's own code (R2-1): one case on the "not covered" list is described with
a fact that is false in a terminal session, and the leak behind it recurs." It found the
round-1 findings fixed ("The round-1 layout costs 0.01 s and 2 MiB; the worst valid-JSON
layout 0.19 s and 41 MiB RSS (was 370 s, 32 GiB)") but for these, all own code, each answered:
- R2-1 (major): "a repository's own marketplace still loads its language servers, and the
  documents say it does not": in a terminal the registering session logs `Total LSP servers
  loaded: 0` and then `3`, and it recurs at each new worktree. Fixed: the marketplace's folder
  is read from the settings that bring it; a unit test, and two live layouts (absolute path,
  where the registering session is its own control; relative path, the run planned before
  anything is registered). The false sentence is gone from the spec and ADR-0327.
- R2-2 (minor): 256 ids of control characters made a Superagent's `--settings` 303,124 bytes,
  too long to start. Fixed: an id is taken only as two names of Claude Code's characters;
  tested in the lookup and as the size of the longest argument (below 64 KiB).
- R2-3 (minor): "a file that is not UTF-8 is read and not charged" (2,560 MiB read). Fixed:
  bytes are read and charged whatever they hold; tested with such files.
- R2-4 (minor): the wiring's arguments were unproven. Fixed: the wiring test has a plugin only
  the repository switches on, from a folder of the repository, for a task run and a
  Superagent (the mutants above).
- R2-5 (minor): a contained Superagent's Claude Code names paths in the person's home as it
  sees them, which the host does not open. Named in the "not covered" list of the spec,
  ADR-0327 and the operations guide; a marketplace whose folder is not opened is now said in
  the launch's warning (its nit).
- Nits: the cap of 8 notes is tested; the live layout of copies outside the cache now depends
  on `installPath`; the anchors' real paths are resolved once.

**Not tested.** Which session showed the person the notice (not written to transcripts;
terminals are on the host). Other projects' sandboxes and other languages' servers (the rule is
by plugin, not by language; the live test uses made-up servers). A Superagent started by the
real server with a language-server plugin in a person's home (its `--settings` are unit-tested
and the flag has the same effect as a task run's file). The cases listed under "Not covered, by
decision".
