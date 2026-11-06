---
yulius-task: 8cb194b5-003b-4501-b0b0-50b032b36fb2
status: done
---

# SPEC-427: A Global Superagent sees every sandbox of its person and keeps them in sync

Status: done
Date: 2026-11-05
Owner: Yulius task #409
Scope authorization: task #409 (`/goal`): "there should be 2 superagens: global and
sandbox-wise. Global has access to all sandboxes and can synchronise them. Implement".
Extends SPEC-021/SPEC-035 (the Superagent, `cockpit.rs`) and SPEC-247 (the remote MCP's tools);
decided in [ADR-0271](../../docs/decisions/0271-a-global-superagent-acts-in-every-sandbox-of-its-person-through-yulius-tools-only.md).
GSA-006's web UI is amended by [SPEC-428](SPEC-428-this-2-superagents-thingy-should-be-some.md): the two
Superagents are tabs of one panel, and a sandbox entered with the Global one in front keeps it open.
GSA-007 is superseded by [SPEC-430](SPEC-430-this-should-be-exactly-like-normal-super.md): its person
types into its terminal as into the sandbox Superagent's; the message endpoints are removed.

## Goal

Every person gets two Superagents. The sandbox Superagent stays as it is: one per sandbox, in that
sandbox's repository, managing that project. The new **Global Superagent** is one per person,
belongs to no sandbox, and reaches every sandbox the person can see: it says what runs where,
reads tasks, CI and files across them, and keeps them in sync — the same change in several
repositories, a client after an API change, a follow-up task in each — by acting through Yulius,
as the person's own agent does over the remote MCP.

## Context

- The sandbox Superagent (`cockpit.rs`, ADR-0020/0029) is a shell of kind `cockpit` per (person,
  sandbox): plain Claude Code in the sandbox repository, contained in the sandbox, its config in
  the person's home, its `ck_` bearer resolved by `cockpit_caller` to a `ProjectCaller` pinned to
  that sandbox. Reaching another sandbox takes `sandbox_task_create` and the person's click on
  every request (ADR-0227), because it reads repository files and runs without approvals.
- The remote MCP (SPEC-247, ADR-0188, `remote.rs`) already serves a person's own agent every
  project they can see: `projects_list`, `tasks_list`/`task_get`/`task_create`/`task_message`…
  with `project`, CI, files (`project_file_read`, `project_change` → pull request, `change_merge`
  on green), checked per call against the person's access, audited `remote.tool`.
- Every run Yulius starts lives in a sandbox's containment (ADR-0003). A run that needs no
  repository can be contained in its own bubblewrap root without a sandbox holder, as production
  CD runs without a holder are (`ci/cd.rs`).
- The person's home (`users/<id>/home`) is bound writable into every task run of theirs, so a task
  agent could write Claude Code settings (hooks) there: a session outside every sandbox must not
  take its config from it.
- The web UI's tools rail is disabled outside a sandbox (All sandboxes, Settings): "Open a sandbox
  first."

## Requirements

- GSA-001: `POST /api/superagent` returns the caller's running Global Superagent or starts it,
  resuming its newest conversation; concurrent opens start one. It needs a sandbox the person can
  work in (write access) to be hosted by, else 409 "the Global Superagent needs a sandbox you can
  work in"; without a usable Claude account or capacity it answers as the sandbox Superagent does.
- GSA-002: it is plain Claude Code on a pool subscription with Yulius's default model and effort,
  **without built-in tools** (`--tools ""`) and with only the Yulius MCP (`--strict-mcp-config`):
  it reads and changes nothing but through Yulius. It reads no settings file
  (`--setting-sources ""`; hooks off and its Concise style in `--settings`), and every shell
  command Claude Code would run for it (a `!` command, a hook, a status line) goes through
  `CLAUDE_CODE_SHELL_PREFIX` (also `SHELL`), a read-only script outside its folder that refuses
  it, so the person gets no shell there either. Its working folder and its Claude Code config
  are its person's own (`users/<id>/superagent`), never the person's home and never a sandbox
  repository. On a contained server it runs in a bubblewrap namespace of its own with that folder
  as its only writable root (no sandbox holder, no repository, the host's `/etc/shadow` and
  `/etc/gshadow` covered); it gets no agent secrets, no
  sandbox MCP servers and no egress proxy. With bypass on a contained server as SPEC-208; every tool of
  its own is pre-allowed, on an uncontained server too (they are only Yulius's tools, each checked at the call; SPEC-430).
- GSA-003: its bearer (`gs_…`, distinct from the sandbox Superagent's `ck_…`) is served on the
  internal `/mcp` the remote MCP's tools for its person, each project tool with `project`, checked
  per call against the person's access to that project as an `operate` token is, except the tools
  that change the vault or variables, deploy, or add, onboard or skip projects (`secret_update`,
  `secret_hosts_answer`, `secret_delete`, `variables_set`, `variable_delete`, `cd_deploy`,
  `project_import`, `onboarding_start`, `onboarding_skip`, and `change_merge` (GSA-007)): those are not listed and refused when
  called. Like an operate token it is bounded to 120 requests a minute. A `ck_` bearer is never served these tools and a `gs_` bearer never the sandbox
  Superagent's. It acts as `superagent:<login>` on the Superagent channel ("Message from alice
  (through their Superagent)"); its changes are audited `remote.tool` with `via:
  global-superagent`; a disabled person or one who lost every sandbox they could work in is
  refused as unauthenticated.
- GSA-004: its MCP instructions say what it is for: seeing what runs in the person's sandboxes and
  keeping them in sync (status across projects, the same change in several repositories as one
  task per repository or a `project_change`, follow-ups after another project's change), that work
  needing judgment or code is a task in that sandbox, and to act only on what the person asks.
- GSA-005: it is its person's private session: only they attach to its terminal; it is never
  listed, renamed or closed as a terminal; it is hosted (registered) under one sandbox the person
  can work in. When that sandbox is removed, it ends; when the person lost write access there, the
  next open ends it (only on an access refusal, not on a failed lookup); either way the next open
  starts it under another and resumes the same conversation. Its shell record has version 2, so a
  Yulius from before it ends and removes it instead of adopting it as a sandbox Superagent. A usage limit moves it to
  a subscription with capacity, and an update that changes what it is served restarts it once
  idle, as the sandbox Superagent (SPEC-074, SPEC-186 SAV-006).
- GSA-006: the web UI: outside a sandbox (All sandboxes, Settings) the rail's Superagent button
  is enabled and opens the Global Superagent, titled "Global Superagent", its read-only terminal
  above a message box (Enter sends, Shift+Enter a new line, Interrupt; a refusal shows above the
  box with the message kept), with a note: "Sees all
  your sandboxes and keeps them in sync: what runs where, the same change across repositories.
  Code work still belongs in a task in its sandbox." Inside a sandbox it opens that sandbox's
  Superagent, whose note now also says the Global Superagent works across sandboxes, from All
  sandboxes. `/superagent` opens All sandboxes with the Global Superagent. A sandbox entered while
  the Global Superagent is open closes the panel instead of starting that sandbox's Superagent.

- GSA-007: the person talks to it by message only, because Claude Code's own keys reach an
  external editor, `!` commands, plugin installs and file mentions that would run or read as the
  server: its terminal is read-only (`can_write` false; a typed key is refused), and
  `POST /api/superagent/message {text}` types one bracketed paste and Enter, the text after the
  label `Message from your person:` unless it is one of the commands below (so held text Claude Code
  stripped and did not submit can never make the next message a command or exit word), with control
  characters but line breaks and tabs removed and `@` as `＠`; a message that starts with `!`, or
  with `/` but is not `/clear`, `/compact`, `/context` or `/cost`, is refused (422), as is one
  that holds a character nobody sees but an emoji's own (Claude Code's trim drops U+FEFF before a
  `/`, and it strips the rest at submit without submitting), one of Claude Code's exit words, one
  naming an image file Claude Code would attach, or one holding `ultraplan` (a command by keyword). The terminal still fits
  its panel (the page sends resizes, not keys). `POST /api/superagent/interrupt` sends Esc; a
  second one within 2 s is dropped (two open Claude Code's rewind menu). A message is typed even if
  its request goes away meanwhile. Both answer 404 to anyone without a running one.
  `EDITOR` and `VISUAL` name the refusing script too. It is not served `change_merge`: text it
  reads in one sandbox must not merge code into another.

## Plan

- [x] Read the product context and related work, then write and validate the spec
- [x] A person opens their Global Superagent in its own folder, without built-in tools
- [x] The Global Superagent's bearer is served every sandbox's tools for its person
- [x] The Global Superagent survives its host sandbox, usage limits and updates
- [x] The rail opens the Global Superagent outside a sandbox
- [x] ADR, architecture, product and API docs describe the two Superagents
- [x] Review round 1 fixed: no shell, hooks or settings files, a recorded contained plan, bounds
- [x] Review round 2 fixed: messages instead of keys, no editor, no merges
- [x] Review round 3 fixed: hidden characters, exit words and paths refused, resizes, one interrupt
- [x] Review round 4 (approved) minors fixed: image paths, keyword, emoji, interrupt gap, detached typing
- [x] Review round 5 fixed: invisible characters anywhere, multi-part lines, a clock stepped back
- [x] Review round 6 fixed: a label before every message, stricter emoji, UNC paths
- [x] Review round 7 (approved) fixed: a fixed label, its meaning in the instructions
- [x] Independent review in a fresh session, findings fixed test-first
- [x] Every gate green, docs and the changes log updated

## Acceptance criteria

- AC-GSA-1: alice opens `POST /api/superagent` twice at once and gets one session; it runs the
  fake `claude` with `--tools "" --strict-mcp-config`, the default model and effort, in
  `users/<alice>/superagent`, its `CLAUDE_CONFIG_DIR` there, its Yulius MCP bearer `gs_…`; it is
  not listed among `s1`'s terminals; carol cannot attach to it; dave, who can work in no sandbox,
  gets 409.
- AC-GSA-2: with that bearer, `tools/list` lists `projects_list`, `tasks_list` and `task_create`
  with `project`, and none of the withheld tools; `task_create` in `s2` (another sandbox of alice)
  creates the task there for alice at once; a sandbox alice cannot see is not found; a withheld
  tool is refused; the `ck_` bearer of alice's sandbox Superagent gets no `projects_list`.
- AC-GSA-3: alice's host sandbox removed → the session ends; the next open is hosted elsewhere
  and resumes the same conversation. alice disabled → the bearer is refused.
- AC-GSA-4 (contained): its containment is a bubblewrap root of `users/<alice>/superagent`, no
  join, no identity, with no sandbox repository or person's home bound.
- AC-GSA-5 (e2e): on All sandboxes the Superagent button is enabled and opens "Global
  Superagent" with its note and terminal; in a sandbox it opens that sandbox's Superagent.

## Test plan

- Integration (`crates/yulius/tests/global_superagent.rs`, reusing the `cockpit` fixture): AC-GSA-1
  to AC-GSA-3 through the router, the real ptyhost and a fake `claude`; `/mcp` with the printed
  bearer.
- Unit (`cockpit.rs`/`remote.rs`/`yulius-harness` launch tests): the contained launch's
  containment (AC-GSA-4), the served tool set and the plan's arguments.
- e2e (`e2e/tests/global-superagent.e2e.ts`) against the mock: AC-GSA-5.

## Decisions

- **Two seats of one kind.** The Global Superagent is a `cockpit` shell with a `global` mark, not a
  second session implementation: the start lock, usage-limit moves, update refreshes, adoption
  after a restart and the terminal protocol stay one code path. Its seat is (person, global)
  instead of (person, sandbox).
- **No built-in tools instead of approvals.** It acts in every sandbox without asking, as the
  person asked; what keeps that safe is that it can only call Yulius, whose tools check the
  person's access per call and audit changes; it merges nothing. Claude Code's file and shell
  tools would reach a folder outside every sandbox and are not needed to coordinate.
- **Reuse the remote MCP's tools** rather than give the sandbox Superagent's tools a `sandbox`
  argument: they already take `project`, check access per call and are tested, and the person's
  own agent and their Global Superagent then see the same tools. The vault, deploy and project
  onboarding tools are withheld: they are the person's decisions (ADR-0110, ADR-0188) and an agent
  reading other repositories' text should not widen where secrets go.
- **Hosted under a sandbox, contained on its own.** Shells are registered per sandbox (routes,
  idle ends, removal); a host sandbox the person can work in keeps that machinery, while the
  process itself is contained in its own root, so it sees no repository. Its conversation and
  config live in its own folder, so a new host resumes it.
- **Own config folder.** The person's home is writable by their task runs; a session outside every
  sandbox takes its settings only from a folder no run can write.

## Evidence

At `c8534735` (the branch before it merged `origin/dev`), on 2026-11-06; after the merge, `scripts/dev ci` at
`589bf036` exited 0 again with 3164 tests passed, 16 skipped:

- `scripts/dev ci`: exit 0. Repository contracts, format, clippy, docs, macOS targets and deps
  pass; `cargo nextest run --workspace --profile ci` ran 3155 tests, 3155 passed, 16 skipped.
- `cd e2e && npx e2e run`: run in two parts, because the first run hit its 50-minute bound. The
  first part had 63 files pass, `original-prompt.e2e.ts` fail once under the full suite's load,
  and 2 files interrupted. The second part re-ran the remaining 13 files plus
  `global-superagent.e2e.ts`: 160 tests passed, and `agent.e2e.ts` was skipped (it needs a
  model). `original-prompt` passed there. After the web changes of review rounds 2, 3 and 5,
  `global-superagent`, `agent-panel`, `superagent-requests` and `terminal` passed again (68
  tests).
- New tests:
  - `crates/yulius/tests/global_superagent.rs`, 12 tests:
    - one session per person in its own folder, without built-in tools or settings files, its
      shell prefix refusing;
    - the remote tools in every sandbox, the withheld ones refused in both `tools/list` and
      `tools/call`, the audit;
    - a contained launch recorded at the session host;
    - the host removed or lost, adoption after a restart, a usage-limit move and an update
      refresh, each staying global;
    - the bearer refused without a sandbox and bounded per minute;
    - messages only: keys refused, the label, commands, refusals, interrupts and their gap;
    - a dropped request still submits.
  - `task_message::a_global_superagent_message_names_the_superagent`.
  - `yulius-harness` `the_global_superagent_runs_with_only_the_yulius_mcp`.
  - Unit tests of `global_message`, `global_paste` and the withheld tools (`global_superagent.rs`).
  - `e2e/tests/global-superagent.e2e.ts`, 5 tests.
- Mutation checks, each making its test fail:
  - without the withheld-tool filter;
  - without the `--tools ""` launch flags;
  - without the key refusal;
  - without `resizeReadOnly`;
  - without the detached typing;
  - with the old interrupt-gap condition.
- Claude Code 2.1.311 was probed in a PTY with a synthetic token, so no model was called:
  - `--tools ""` and `--restricted` still run `!` commands and hooks;
  - `--setting-sources ""` and `disableAllHooks` stop hooks and status lines;
  - a refusing `CLAUDE_CODE_SHELL_PREFIX` blocks `!` commands, quote, `;` and `$()` escapes
    included;
  - the CLI accepts the launch flags.
- Independent review in fresh sessions, 7 rounds. Every finding was in this change's own code
  and was fixed test-first:
  - Round 1: commands could run in the session; the containment test could not fail; host loss;
    a panel starting a sandbox Superagent; no rate limit; the sweep; the record version; docs.
  - Round 2: Ctrl+G opened an editor, so the person now talks to it by message only;
    `change_merge` withheld; a canonical root; the 1 MiB limit noted.
  - Round 3: U+FEFF before a `/`; resizes; a double interrupt; detached typing; prompts on
    uncontained servers.
  - Round 4 (approved, with minors): image paths as Claude Code reads them; `ultraplan`; emoji;
    the clock; tests.
  - Round 5: invisible characters anywhere; multi-part lines; a test that steps the clock back.
  - Round 6: characters Claude Code strips and then holds back, after which every message comes
    after a label (`Message from <login>:`, fixed in round 7); stricter emoji; UNC paths.
  - Round 7: **APPROVE**. Its minors were fixed: a fixed label (no login carries a keyword) that
    its instructions name; tests; docs.
- Not run: `yulius security-scan` reports "Security scan disabled". Vidoc is turned off for this
  project in Yulius, so nothing was scanned; this is not a pass.
- Not tested against a live model or a real isolated instance:
  - a real conversation of the Global Superagent;
  - the bubblewrap launch on a contained server (its plan is tested, not its run);
  - whether the host's Claude Code version behaves as 2.1.311 did in the probes.

