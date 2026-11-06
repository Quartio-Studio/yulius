# 0271. A Global Superagent acts in every sandbox of its person, through Yulius tools only

## Status

Accepted. Specified in [SPEC-427](../../.ai/specs/SPEC-427-there-should-be-2-superagens-global-and.md);
task #409 ("there should be 2 superagents: global and sandbox-wise. Global has access to all
sandboxes and can synchronise them"). Adds a second Superagent beside the one of
[0020](0020-cockpit-as-a-plain-shell-session.md) and
[0029](0029-cockpit-manages-through-mcp.md); leaves
[0227](0227-the-superagent-starts-tasks-in-other-sandboxes-only-on-its-persons-approval.md) (the
sandbox Superagent's approved cross-sandbox requests) as it is, and reuses the tools of
[0188](0188-remote-control-uses-scoped-personal-api-tokens.md) (the remote MCP).

## Context

Each person has a Superagent per sandbox: Claude Code in that sandbox's repository, contained in
it, managing that project. People with several repositories (a service and its clients, a website
and its API) want one agent that sees all of them and keeps them in sync, without switching
sandboxes or approving each step. The sandbox Superagent may reach another sandbox only on the
person's click per request (ADR-0227), because it reads repository files and runs shell commands
without approvals: text it reads could otherwise start work everywhere.

The remote MCP already serves a person's own agent every project they can see, each tool checked
against the person's access at the call, changes audited, merges only on green CI. Every run
Yulius starts is contained in a sandbox (ADR-0003), and the person's home is writable by all
their task runs.

## Decision

- **One Global Superagent per person**, `POST /api/superagent`, beside the sandbox Superagents.
  It is a Cockpit shell marked `global`: start lock, usage-limit moves, update refreshes, adoption
  after a restart and the terminal protocol are the sandbox Superagent's.
- **It acts only through Yulius, and nothing runs in it.** Claude Code runs with no built-in tools
  (`--tools ""`) and only the Yulius MCP (`--strict-mcp-config`): no file reads or writes, no web
  fetches, no other MCP server, no agent secrets. `--tools ""` does not stop the person's own `!`
  commands, nor hooks and status lines from its settings (measured on 2.1.311), so it also reads
  no settings file (`--setting-sources ""`, `disableAllHooks` and its Concise style in
  `--settings`), and `CLAUDE_CODE_SHELL_PREFIX` and `SHELL` name a read-only script outside its
  folder that refuses every command: Claude Code runs `'<prefix>' '<command>'`, so a command
  (quotes, `;`, `$(…)` included) is only ever that script's argument. With nothing to print a
  secret there is no redaction hook.
- **The person talks to it by message, not keys.** Claude Code's keys reach more than its shell:
  Ctrl+G and `/memory` start an external editor (`vi` from the host) without any shell prefix,
  `/plugin` installs code, and `@path` attaches a file read as the server. So its terminal is
  read-only, and `POST /api/superagent/message` types each message as one bracketed paste and
  Enter, after the label `Message from <login>:` (one of `/clear`, `/compact`, `/context` and
  `/cost` alone). Claude Code then never reads the input as a command, a `!` line or an exit
  word, whatever it does with the characters of a message: when it strips some at submit it holds
  the text back unsent, and the next message joins it, but every person's word comes after a word
  of Yulius's. On top of that, control characters are removed (no ESC ends the paste), `@` is
  typed as `＠`, and `!` lines, other commands, characters nobody sees, exit words, image paths and
  a keyword command are refused (below); `POST …/interrupt` sends Esc.
  `EDITOR` and `VISUAL` name the refusing script as well.
- **Its tools are the remote MCP's for its person, minus the person's decisions.** Its `gs_`
  bearer resolves to a caller acting as an operate token of its person would, with the actor
  `superagent:<login>`, the Superagent channel ("through their Superagent") and `remote.tool`
  audits marked `global-superagent`. It is not served the vault and variable tools, `cd_deploy`,
  `project_import`, `onboarding_start`, `onboarding_skip` or `change_merge` (text it reads in
  one sandbox must not merge code into another; a person or the task's delivery merges what it
  opens). A `ck_` bearer never reaches these
  tools, and a `gs_` bearer never the sandbox Superagent's.
- **No approval per action.** The person asked for an agent with access to all their sandboxes
  that keeps them in sync, unlike the per-request approval they asked of the sandbox Superagent
  (ADR-0227); its reach is exactly theirs, checked per call, and what it changes goes through
  tasks, pull requests a person or a task merges, automations and task messages, each visible in
  its sandbox and audited. Its instructions treat what it reads as data, never as instructions to
  act elsewhere.
- **Hosted by a sandbox, contained in its own root.** Shells are registered per sandbox (private
  terminal, idle end, removal). The Global Superagent is registered under the oldest live sandbox
  its person can work in; the process itself runs in its own folder `users/<id>/superagent` (its
  working folder and Claude Code config), on a contained server in a bubblewrap namespace whose
  only writable root is that folder, without a sandbox holder, repository or home, the host's
  `/etc/shadow` and `/etc/gshadow` covered. Its bearer never travels a sandbox's network, where
  another run could read it. A removed host ends it; a host where its person lost write access is
  left by the next open (only on that refusal); then it starts under another sandbox, resuming the
  conversation kept in its folder. Its record is version 2 of `shell.json`, which a Yulius from
  before ends and removes. Its bearer has an operate token's bound of 120 requests a minute.
- **The web UI opens it outside a sandbox.** On All sandboxes, Settings and the other pages without
  a sandbox, the rail's Superagent opens the Global Superagent; in a sandbox, that sandbox's.

## Alternatives

- **Give the sandbox Superagent a `sandbox` argument on every tool.** One agent, but it would run
  shell commands in one repository while acting in all of them, the risk ADR-0227 refused; with
  per-action approval it would not be the hands-off coordinator asked for.
- **Approve every cross-sandbox action of the Global Superagent too.** Safe but not what was asked
  ("has access to all sandboxes and can synchronise them"); its lack of shell and file tools and
  the per-call access checks carry the risk instead.
- **A sandbox-less shell kind with its own registry and routes.** Cleaner in the model, but it
  would duplicate the terminal, adoption, idle and limit machinery the Cockpit already has; a host
  sandbox costs one lookup.
- **Running it in its host sandbox's holder.** It would be contained as a person's terminal is,
  but its bearer, which reaches every sandbox of its person, would cross that sandbox's network,
  where every run (a teammate's task included) holds `CAP_NET_RAW` and could read it.
- **A holder of its own.** The strongest containment, but the isolation adapter makes holders for
  sandboxes (system layer, quota, Docker, relay), and one per person needs that adapter to know a
  sandbox-less holder; not needed while nothing runs in it.
- **Its config in the person's home.** Every task run of the person can write there (Claude Code
  settings with hooks), and the Global Superagent runs outside every sandbox.

## Consequences

- "Which of my sandboxes have failing CI?", "the API changed in acme/api: make its clients follow"
  and "tell every running task to rebase on dev" work from one panel, as tasks, pull requests and
  messages in each sandbox, audited.
- It cannot read a file except through `project_file_read` and cannot write one except through a
  `project_change` pull request; code work stays in tasks.
- An injected instruction in something it reads can act with the person's reach over Yulius's
  tools (start tasks and automations, open pull requests, message tasks); it cannot merge, deploy, change
  the vault or add projects itself, but a task it starts has that task's own tools (a dev deploy,
  the vault tools a task agent has). Every change is audited under the person's name.
- Its containment is a root of its own folder in the server's network and as the server's user,
  without a user namespace: what keeps that safe is that nothing runs in it but Claude Code (no
  tools, hooks, settings files, shell or editor) and that no key of the person reaches Claude
  Code. A Claude Code release that ran code from a model's answer or a pasted message would need
  a holder of its own for it, which is the next step if that happens.
- Contained without a holder, it runs the host's Claude Code installation, which may lag the copy
  isolated runs bind (ADR-0266). What it relies on was measured on 2.1.311: `--tools`,
  `--strict-mcp-config`, `--setting-sources`, `disableAllHooks`, `CLAUDE_CODE_SHELL_PREFIX`, and
  how Claude Code reads a message: its JavaScript trim drops U+FEFF before a `/`, and at submit it
  strips characters nobody sees (and a few more, such as U+16FE4 and U+1107F) and then does not
  submit. The label keeps such held text harmless; Yulius still refuses those characters, except
  in a conservative set of emoji (a selector after a pictograph or in a keycap, a joiner between
  pictographs, a subdivision flag's tags), refusing some Claude Code would keep (in other scripts,
  rarer emoji). Its exit words (`exit`, `quit`, `:q` and the like) are refused. Another release
  may read messages differently; the label does not depend on that.
- Claude Code attaches an image file a pasted line names: the line is split before each space a
  path follows, and when every part is a path (or there is one part), each one unquoted and
  unescaped that is an absolute path ending in `.png`, `.jpg`, `.jpeg`, `.gif` or `.webp` is read;
  such a message is refused, as is a one-part line of a bare image name (which it compares with
  the clipboard) and, to be safe, a drive path; a part starting `\\` counts as a path. A path in
  a sentence is text to it and passes. A one-part line naming an image with spaces in it (`look
  at shot.png`) passes; Claude Code compares it with the clipboard by a fixed command, which finds
  nothing on the server. The
  word `ultraplan` can start a command of its own (2.1.311, behind a feature flag); a message
  holding it anywhere is refused, more than Claude Code would act on.
- Every one of its tools is pre-allowed, on an uncontained server too: no key reaches it, so no
  question Claude Code asked about a tool could be answered.
- A person who can work in no sandbox (read-only access, a guest without one) has no Global
  Superagent.
