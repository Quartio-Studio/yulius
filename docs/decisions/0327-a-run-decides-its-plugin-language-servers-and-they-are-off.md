# 0327. A run decides its plugin language servers, and they are off

## Status

Accepted. Specified in [SPEC-485](../../.ai/specs/SPEC-485-task-sessions-start-without-yellow-error.md);
task #467.

## Context

A person saw this in a task's terminal, between the agent's steps (2026-11-13): `Language server
"plugin:rust-analyzer-lsp:rust-analyzer" couldn't start. The Errors tab in /plugin may have the
reason.`, and asked for such errors to be fixed.

Measured with Claude Code 2.1.323 run in a terminal against a local stand-in for the model API:

- **The notice.** Claude Code starts a plugin's language server when a session first writes or
  edits a file of the server's language, one server per session. The official plugin runs
  `rust-analyzer`, here rustup's proxy, which picks a toolchain where it is started. Enabled,
  with a toolchain that lacks the component, it exits at once (`error: Unknown binary
  'rust-analyzer' in official toolchain 'stable-x86_64-unknown-linux-gnu'.`, exit code 1) and the
  terminal shows that notice. Whether a server can start therefore depends on a program and a
  toolchain Yulius does not provision.
- **How a plugin gets in.** When a session writes such a file, the server's program is on
  `PATH` and the plugin is not installed, Claude Code opens a dialog in the terminal ("LSP
  plugin recommendation"). Its default answer is yes: one Enter installs the plugin for the
  user and writes `"enabledPlugins": {"<plugin>": true}` into the home's `settings.json`. A
  sandbox's sessions share one home, so every session of the sandbox starts the server from
  then on. It holds for any language whose server is installed.
- **Which setting wins.** `enabledPlugins` is read from every settings layer. The home's file is
  the lowest: a repository's `.claude/settings.json`, a worktree's `.claude/settings.local.json`
  and a run's `--settings` outrank it both ways. `false` in the home's file, which a person set
  here on 2026-10-13, is therefore no decision.
- **What a working server costs.** In October the plugin ran in every session of this project:
  20 GB and OOM kills in the sandbox (SPEC-082). Measured again on this repository, one server:
  2.6 GiB at rest, 6.2 GiB with its children, 109 threads, and after a saved file a `cargo
  check` of the whole workspace in the worktree's own `target/` (80 s), which holds the build
  directory's lock while the agent's own cargo waits; concurrent cargo runs break the gate's
  doctest step here. The plugin was used while it ran (411 requests in a day and a half).

## Decision

- **Yulius decides a run's language servers, per run, and they are off.** For every session
  Yulius starts (task run, Superagent, Global Superagent) it switches off the plugins it finds
  to bring a language server; Consequences name what it does not look at.
- **Off in the run's own settings.** Before each launch Yulius reads which plugins of the
  session bring a language server (`claude_plugins::language_server_plugins`) and writes
  `"enabledPlugins": {"<id>": false}` for each into the settings it gives the session
  (`claude-settings.json` of a task run, `--settings` of a Superagent; `disabled_plugins` of the
  launch requests). Those outrank the home's, a repository's and a worktree's settings, so for
  the plugins found the server is neither loaded nor started nor reported, whatever a file, a
  `/plugin` or a dialog set. Other plugins stay as they are; a plugin that brings a server and
  other things (skills, commands) is switched off whole, since Claude Code has no switch for a
  plugin's language server alone.
- **Which plugins: as the real tool loads them.** The installed ones
  (`plugins/installed_plugins.json`) and the ones the home's `settings.json`, the worktree's
  `.claude/settings.json` or `.claude/settings.local.json` switch on, which Claude Code loads in
  place from a marketplace that is a folder, with no install record. A marketplace lies where
  `plugins/known_marketplaces.json` says, else where one of those settings files names it as a
  folder (`extraKnownMarketplaces`: in a terminal Claude Code loads from it in the very session
  that registers it), else at `plugins/marketplaces/<marketplace>`.
- **"Brings a language server" is read by presence, from the three places Claude Code reads.**
  `lspServers` other than `null` in the plugin's entry of its marketplace's `marketplace.json`,
  the same key in the plugin's manifest, or a file `.lsp.json`, in any of the plugin's folders
  (its copies in the cache, where an install record says a copy lies, its `source` in the
  marketplace). Their grammar is not mirrored.
- **The lookup is read-only, follows no link, has one budget and never fails a launch.** All of
  it is the agent's: everything is opened one name at a time with `O_NOFOLLOW` from the folder
  above the home ([ADR-0174](0174-host-access-to-agent-owned-folders-goes-through-handles.md));
  a path the agent wrote is opened only below the sandbox directory or that folder. The whole
  lookup reads at most 16 MiB (2 MiB a file) and looks into at most 1024 plugin folders, for
  at most 256 plugins with ids of at most 200 bytes, each two names as Claude Code takes
  them, so that a run's settings cannot outgrow a command line. What cannot be read brings nothing and is
  logged with the launch: this is tidiness, not a boundary, since a language server runs
  inside the session's containment like any program the agent starts.
- **Nobody is asked to install one.** `ensure_claude_config` sets `"lspRecommendationDisabled":
  true` in the config dir's `.claude.json` at every launch, Claude Code's own switch for that
  dialog.

## Alternatives

- **Make the server work in every task session** (the component in every toolchain, the proxy
  resolved from the worktree). It already starts in a worktree of this repository; the cost
  above is the reason against: gigabytes per session with parallel tasks, and its checks
  competing with the agent's cargo for the build lock. The agent gets the same diagnostics
  from the checks it runs anyway. Rejected as the default.
- **On per project, with a budget and a build directory of its own.** Navigation has value.
  It needs a project setting, a memory and CPU budget for the server
  ([ADR-0065](0065-processes-plan-for-a-cpu-and-memory-budget.md)'s kind), and a server
  configuration that keeps its checks out of the worktree's `target/`,
  for every language. Left to a task that asks for it; this decision does not close it.
- **Keep `false` in the home's `settings.json`** (today's state). It is the lowest layer and
  shared: a repository's settings, a `/plugin` in any terminal or the dialog's Enter change it
  for every session. Rejected: the half-enabled state the request names.
- **Write `false` into the home's file at every launch.** Still outranked by a repository's and
  a worktree's settings, and it edits a file that is the person's and Claude Code's.
- **Uninstall such plugins.** Yulius would delete what a person installed, and the dialog would
  offer it again on a server from before this change.
- **Switch every plugin of a sandbox's home off.** Skills and commands a person installed there
  gave no notice of this kind; a product decision nobody asked for.
- **A check that fails when an enabled server cannot start** (a doctor line per sandbox). With
  the servers off there is nothing to start; the check is the test that a run switches them
  off, against the real CLI.

## Consequences

- A session shows neither the "couldn't start" notice nor the install dialog, in any language,
  in the layouts the lookup reads. Agents have no go-to-definition from a language server; they
  read and search code as before.
- Not covered, where a server can still start: a marketplace or an installed copy outside the
  sandbox's folders (the host does not open a path the agent names there), which for a
  contained Superagent includes one in the person's own home, named by the path its Claude Code
  sees (`/home/<login>/…`); a plugin installed or switched on while a session runs (off from
  the next launch); plugins given on a command line (`--plugin-dir`, which Yulius does not
  pass). The launch's warning names a marketplace whose folder it did not open, and
  the operations guide says how to see the rest.
- A plugin a person installs by hand in a terminal that brings a language server works until
  that session ends and is off in the next launch. Who wants one for a project asks for the
  alternative above.
- An existing instance needs the update only. Sessions get the change at their next launch;
  nothing in a sandbox's home has to be changed, and an installed plugin may stay.
- The opt-in live check (`YULIUS_LIVE_CLAUDE=1`, no account) keeps it true against the installed
  Claude Code: in eight layouts (cloned and folder marketplaces, with and without install
  records, switched on by the home, the repository or the worktree, a marketplace the
  repository brings) the real CLI loads three
  plugins' servers with the files alone and none with a run's settings. A later Claude Code
  that loads a layout differently shows there, as a control that no longer loads or a server
  that still does.
