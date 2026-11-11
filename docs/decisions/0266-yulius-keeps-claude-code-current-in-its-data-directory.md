# 0266. Yulius keeps Claude Code current in its data directory, and runs bind the newest

## Status

Accepted. Specified in [SPEC-421](../../.ai/specs/SPEC-421-claude-should-autoupdate-when-new-versio.md);
task #403 ("Claude should autoupdate when new version is released without logging out"). Amends
SPEC-002's launch contract row "Updates" (the operator alone updated the CLI, #45). Amended by
[0293](0293-claude-code-follows-latest-and-must-know-the-default-model.md): `latest` is the
default channel, a release must know the default model, the channel is an instance setting, and
a running task moves at a turn boundary.

## Context

Claude Code's own updater is off in every session (`DISABLE_AUTOUPDATER=1`, #45): a contained run
cannot write the system-wide npm install, so it only failed. Since then nothing updated Claude
Code on an instance: the operator had to install each release as root, and the instance ran
2.1.292 for a month while npm's `stable` reached 2.1.311. Runs in an isolated sandbox already see
the host's npm package bound read-only at its own path, chosen per run; sign-ins are sealed tokens
handed to each run as `CLAUDE_CODE_OAUTH_TOKEN`, so a new binary invalidates none of them.

## Decision

- **The server installs releases itself, as its service user, under `<data>/claude-code`.** About
  every hour (first 2 minutes after a start) it reads npm's dist-tag of `YULIUS_CLAUDE_UPDATES`
  (`stable` by default, `latest`, or `off`). A version newer than the one runs get is downloaded as
  the platform package's tarball from `registry.npmjs.org` (bounded), checked against the
  registry's sha512 `integrity`, and only its native binary `package/claude` is taken. It must be a
  64-bit ELF for the platform that embeds `VERSION:"<v>"`; it is never run on the host. It then
  moves to `versions/<v>/bin/claude.exe`, and `current` is switched by an atomic rename; the
  version before stays, older ones are removed once no live process executes them. A failure
  changes nothing and is tried again an hour later. Only a host package laid out as
  `bin/claude.exe` is updated or bound over.
- **Runs bind the newest.** Every run joining an isolated sandbox (agent sessions and resumes,
  Superagent sessions, people's shells, CI jobs, dev CD runs) gets, read-only at the host
  package's path, the newer of the host's package and Yulius's `current`, chosen when the run
  starts (`SandboxJoin.extra_binds`, a `Bind` whose source may differ from its destination). A
  running process keeps its binary; its next start gets the new one. Nobody signs in again and no
  session is stopped, frozen or restarted. Production CD runs still get no agent CLI.
- **The host's install stays the operator's.** It is never written; a newer host version (a manual
  update) wins; without an npm install on the host there is nothing to bind over and nothing is
  updated. `yulius doctor` names the version isolated runs get and where it comes from.

## Alternatives

- **A root systemd timer that runs `npm install -g` on the host.** It keeps one install, but needs
  `yulius host setup` rerun by a person on every instance, a helper able to write
  `/usr/local/lib/node_modules`, runs package scripts as root, and replaces the files a running
  session executes from in place, with no atomic switch or way back.
- **A new action of the update helper (ADR-0013).** Same root concerns, and a contract change of
  the helper's request for something that needs no root at all.
- **Turning Claude Code's own updater back on.** It cannot write a root-owned or read-only
  install from inside containment (#45); a per-sandbox writable install would download the same
  300 MB once per sandbox.
- **`npm install --prefix <data>/claude-code`.** Needs Node.js on the server's path and runs the
  wrapper's `postinstall`; taking the binary from the platform package with its registry checksum
  gives the same file without either.
- **Running the download's `--version` to check it.** It would run a freshly downloaded binary as
  the service user, outside any sandbox, with the database and the vault key in reach; the ELF
  header and the embedded version check the same without running it.
- **Following `latest` by default.** Unattended sessions would get every release on its day;
  `stable` lags about a week and skips releases with major regressions, and `latest` is one
  setting away.

## Consequences

- Instances follow Claude Code releases within an hour of their dist-tag moving, with no person
  involved; the operator's prerequisite stays one npm install of Claude Code on the host.
- About 300 MB per kept version (two kept) in the data directory, and one download per release.
- Behaviour read from one Claude Code version (transcript shapes, `CLAUDE_CODE_RESUME_INTERRUPTED_TURN`,
  `updatedToolOutput`, first-run keys) now meets newer versions without a person choosing them;
  `stable` and `YULIUS_CLAUDE_UPDATES=off` are the levers when a release breaks something.
- The sign-in terminal (`claude setup-token`), the Jev split proposer and uncontained runs on
  development hosts keep the host's `claude` on `PATH`: they run outside the bound system layer and
  do not depend on the CLI's version.
- A shell started before two switches that runs `claude` again, after no process ran that version
  any more, finds it removed; a new shell gets the current one.
- An instance export leaves `claude-code/` out: the new host downloads its own build.
