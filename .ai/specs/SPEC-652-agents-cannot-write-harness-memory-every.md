---
yulius-task: f044786c-a9a7-4102-9b19-9623a7bd41f3
status: in-progress
---

# SPEC-652: Agents cannot write their harness's memory; every session starts with it read-only

Status: in-progress
Date: 2026-11-25
Owner: Yulius task #636
Scope authorization: task #636 (`/goal`, the owner's decision of 2026-11-25): "an agent that
Yulius runs must not be able to save anything to its harness's persistent memory, in any sandbox
and in a person's Superagent, while it can still read what is already there." Amends SPEC-002
ADR-03 (bubblewrap containment) and ADR-22 (runs that join a sandbox's holder).

## Goal

No session Yulius starts, on any harness, can create, change or delete a file in its harness's
persistent memory or user-level instruction files, by an edit tool, the shell or a script. What
is there still loads. A refused write tells the agent why in one sentence and that lasting
knowledge goes into the repository through a pull request. Twice in one week a remark about a few
tasks became a standing note and slowed every later task for days (review rounds of 20 to 28
minutes instead of 6; tasks of 7 to 32 hours) before anyone found the note.

## Context

Confirmed against `dev` at ecb57ae03 and the pinned CLIs.

- **Where each harness keeps it.**
  - Claude Code 2.1.332 (`claude.exe` strings): auto memory is
    `<config>/projects/<key>/memory/` (`MEMORY.md` and topic files), where `<config>` is
    `CLAUDE_CONFIG_DIR` (Yulius sets `<home>/.claude`) and `<key>` is the git root of the
    session's folder with every character outside `[a-zA-Z0-9]` replaced by `-` (a worktree's key
    is its main repository's: this task runs in `worktrees/f044…` and its memory is
    `projects/-var-lib-yulius-sandboxes-d62b…-repo/memory`). Keys over 200 characters get a hash
    suffix. The setting `autoMemoryDirectory` (policy, `--settings`, user settings) replaces that
    path; `autoMemoryEnabled: false` stops reading too, so it cannot be the answer. User-level
    instructions are `<config>/CLAUDE.md` and `<config>/rules/`.
  - Codex 0.170.0 (the pinned CLI): user-level instructions `<CODEX_HOME>/AGENTS.override.md`,
    else `<CODEX_HOME>/AGENTS.md` (an empty or blank file is skipped); memories
    `<CODEX_HOME>/memories/` (feature `memories`, "stable", off by default: `codex features
    list`). Yulius's Codex homes are `<sandbox>/codex/<account>`, kept between runs.
  - Antigravity 1.5.12: rules `~/.gemini/GEMINI.md`, and in its global customization root
    `~/.gemini/config/` the files `GEMINI.md`, `AGENTS.md` and the folder `rules/`. No local
    auto-memory folder is in the binary (its memory protos are server calls). Yulius gives each
    Antigravity run a fresh `.gemini` in the run's own folder (`<run>/antigravity-home`, SPEC-505
    AGY-003), so nothing there outlives the run; only `brain/` and `conversations/` are shared,
    and those are conversation state, not instructions.
- **Who starts sessions.** Task runs (`supervisor.rs`, every harness; contained runs are a
  bubblewrap namespace, most joining the sandbox's holder), a person's Superagent (Cockpit,
  `cockpit.rs`, Claude Code in the person's `users/<id>/home` bound at `/home/<login>`), and the
  Global Superagent (`global_superagent.rs`, its own folder, no shell). An uncontained server
  (`Containment::None`, development) is not a security boundary (ADR-0003).
- **What the stopgap does not cover.** `chattr -R +i` on existing memory folders and
  `permissions.deny` in each home's `settings.json` miss new sandboxes and homes, a deny rule
  stops only Claude Code's own tools (not a script), and an immutable folder makes the purge of a
  removed sandbox fail (`playground/purge.rs` deletes every file).
- **Binding an agent-controlled path is unsafe by name.** bwrap resolves a bind's source path
  when it starts. An agent of the same sandbox could swap a folder in its home for a symbolic link
  to a host path between Yulius's check and the bind, and the link's target would appear in the
  sandbox. bwrap 0.11 takes an open descriptor instead (`--bind-fd`, `--ro-bind-fd`) and
  arguments from a descriptor (`--args FD`).
- **Not in scope.** Skills, agents, plugins, project files (`CLAUDE.md` or `AGENTS.md` in a
  repository: they change by pull request already), a person's own terminal and processes (a
  person edits by hand), CI and CD runs (no harness session).

## Requirements

- **MEM-001** Every contained session Yulius starts (task run of Claude Code, Codex or
  Antigravity, a person's Superagent, the Global Superagent) sees these read-only, from its first
  session in a new sandbox or home and after every update: Claude Code's memory folder of the
  session (`<config>/projects/<key>/memory`), `<config>/CLAUDE.md`, `<config>/rules`; Codex's
  `AGENTS.md`, `AGENTS.override.md` and `memories` in its `CODEX_HOME`; Antigravity's
  `.gemini/GEMINI.md`, `.gemini/config/GEMINI.md`, `.gemini/config/AGENTS.md` and
  `.gemini/config/rules`. A write, create, rename, link or delete there by any process of the
  session fails (`EROFS`/`EBUSY`).
- **MEM-002** What exists there still loads: the paths are bound read-only over themselves, so
  their contents are unchanged. Claude Code's run settings pin `autoMemoryDirectory` to the
  protected folder, so the session reads and would write exactly the folder that is protected.
- **MEM-003** A protected path, or a folder on the way to it, that is a symbolic link or of the
  wrong kind is moved aside to `<name>.yulius-moved-<ms>` (without following it) before the
  session starts; a path that does not exist then gets an empty read-only placeholder (an empty
  folder or file). A session never shows a link's target and never fails to start because of
  what an agent put at such a path.
- **MEM-004** No bind source is resolved by name inside a folder the agent writes. The launch
  opens each protected path from a server-made anchor (the sandbox folder, the person's home, the
  Global Superagent's folder, the run's own folder) without following a link, and hands bwrap the
  open descriptor. bwrap gets these binds through `--args` descriptors placed in its argument
  list after the binds they must cover; nothing of it reaches the agent's process.
- **MEM-005** Renaming or removing a folder above Claude Code's memory folder within its
  configuration (`<config>`, `<config>/projects`, `<config>/projects/<key>`) fails: each is bound
  over itself, so it is a mount point (`EBUSY`).
- **MEM-006** A refused write is explained in one sentence: "Yulius keeps this harness's memory
  and user-level instructions read-only (the owner's decision of 2026-11-25): put lasting
  knowledge in the repository through a pull request." Claude Code's edit tools on a protected
  path are denied with it (a `PreToolUse` hook, before the tool runs) and its run settings deny
  `Edit(...)` on those paths; a shell result that fails with "Read-only file system" on a
  protected path gets it as added context (Claude Code and Codex `PostToolUse`); an Antigravity
  session gets it with its start context (its CLI has no hook after a tool). Claude Code's memory
  instructions say it (`CLAUDE_COWORK_MEMORY_EXTRA_GUIDELINES`).
- **MEM-007** Removing and purging a sandbox that has notes works: the protection is only in each
  session's namespace, nothing on the host is immutable.
- **MEM-008** A person with access to the host edits these files in place, as before.
- **MEM-010** Every other process of a sandbox that runs code an agent chose sees the sandbox
  home's `.claude` and its Codex homes as empty read-only folders: its Docker engine (so a
  container's `-v` never reaches them), its CI jobs and its script watches.
- **MEM-011** A Claude Code session reads no instruction file of a folder above its own
  (`claudeMdExcludes`), so a `CLAUDE.md` an agent leaves in the sandbox folder loads nowhere.
- **MEM-012** The memory key of every task and Superagent session of a sandbox is Claude Code's
  key of the sandbox's repository, from its path alone; a guarded path that is not beneath its
  anchor, or that cannot be opened for another reason than being missing or a link, fails the
  start.
- **MEM-009** The update's host check (root) clears the stopgap's immutable flag on
  `sandboxes/*/home/.claude/projects/*/memory` and `users/*/home/.claude/projects/*/memory`
  (folder and files) and reports what it cleared; the `MEMORY.md` line and the deny rules in
  `settings.json` are left to the owner.

## Plan

- [x] Spec and decision record of the read-only memory guarantee
- [x] Protected paths of each harness and session, with Claude Code's memory key
- [x] bwrap binds of opened descriptors: placeholders, pins and the argument descriptors
- [x] `sandbox-exec` opens the protected paths without following links and starts bwrap
- [x] Task runs, a person's Superagent and the Global Superagent launch with the guard
- [x] Claude Code run settings pin the memory folder and deny edits there
- [x] Real-bwrap proof on a guest: writes refused per harness, reads kept, purge works
- [x] Architecture, operations and changes updated
- [x] Review round 1 of the guarantee answered
- [x] Containers, CI jobs and script watches see the harness folders masked
- [x] Memory key from the sandbox's repository; ancestors' instructions excluded; guard errors fail the start
- [x] Joined runs and the Docker mask proven on a guest
- [ ] Independent review of the guarantee approved
- [ ] The refusal sentence for edit tools, shell results and the Antigravity start
- [ ] The update's host check clears the stopgap's immutable flags
- [ ] Independent review of the refusal messages and the stopgap clean-up approved

## Acceptance criteria

- **AC-1** (MEM-001, MEM-004) The bwrap argument list of a Claude Code task run, a Codex run, an
  Antigravity run, a Superagent and the Global Superagent holds `--args` descriptors after its
  binds, and `sandbox-exec` writes into them `--ro-bind-fd` (or a placeholder) for each protected
  path and `--bind-fd` for each pin.
- **AC-2** (MEM-001, MEM-005) In a real bwrap namespace, for each harness's layout: appending to,
  replacing, creating and deleting a memory or instruction file with the shell and with a script
  (Python `open`, `os.rename`, `os.unlink`) fails; renaming the memory folder, its parent or
  `<config>` fails; reading the existing note gives its text.
- **AC-3** (MEM-003, MEM-004) A protected path that is a link to a host file shows an empty
  read-only placeholder, never the file's text; a missing one shows an empty read-only folder or
  file and the session starts.
- **AC-4** (MEM-002) A Claude Code run's settings pin `autoMemoryDirectory` to the protected
  folder of its key; the key of a worktree is its main repository's.
- **AC-5** (MEM-006) A `Write` to a protected path is denied with the sentence; a Bash result
  with "Read-only file system" naming a protected path gets the sentence as added context; others
  are untouched.
- **AC-6** (MEM-007) After a session with notes, the sandbox's purge deletes its files.
- **AC-7** (MEM-009) The host check clears an immutable memory folder and reports it, and leaves
  a clean one alone.

## Test plan

- Unit (`yulius-app`): the protected paths for each harness and session kind, Claude Code's key
  (worktree, plain folder, long path), anchors and pins.
- Unit (`yulius-ptyhost`): `bwrap_argv` places the `--args` descriptors after the binds and the
  pins before binds beneath them.
- Unit (`yulius-sandbox`): the guard opens a real folder and file without following links, gives
  placeholders for a link, a missing path and a wrong kind, and writes the bwrap arguments.
- Unit (`yulius-harness`): run settings of Claude Code (task and Superagent) carry
  `autoMemoryDirectory` and the deny rules; the hook client's deny and added context.
- Opt-in real-namespace test (`YULIUS_ISOLATION_TEST=1`) and a run on a QEMU guest (Ubuntu
  24.04, this sandbox has no user namespaces): the shell and script attempts of AC-2 and AC-3
  through `yulius sandbox-exec`, and a purge of a sandbox with notes.

## Decisions

- **Read-only binds in each session's namespace, not a filesystem flag on the host**
  (ADR-0479). They hold for every process of the session, cost nothing on the host, keep a
  person's hand edits and a sandbox's removal working, and cover new sandboxes and homes at their
  first session. Rejected: `chattr +i` (blocks removal and the person's edits, misses new
  folders); Claude Code deny rules alone (a script passes); `autoMemoryEnabled: false` (stops
  reading); Landlock (allow-lists only, cannot carve a folder out of a writable home); a copy of
  the notes in a server store (two places to edit; the person's own terminal would read the other
  one).
- **Descriptors, not names.** See MEM-004. Placeholders instead of failing a start, so an agent
  cannot stop its sandbox's later sessions with a link.
- **Review round 1.** A worktree's `.git` is no longer read to find the key (a FIFO there hung
  the launch; a crafted pointer chose the folder), the engine and CI get masks (a container's
  `-v` reached the files), a guard error fails the start instead of an empty guard or
  placeholder, `claudeMdExcludes` closes the folders above the worktree, and the limits are named
  in ADR-0479. Masks rather than read-only binds in the engine's view: dockerd and a container
  could remount a read-only bind of the real folder read-write.
- **Pins only inside Claude Code's configuration.** Binding the home or a Codex home over itself
  would also close renames out of the home: a mount boundary there makes renames and hard links
  between the home and the rest of the sandbox fail (`EXDEV`: the compiler cache's hard links,
  Codex moving rollouts between its home and the shared sessions). Remaining: an agent that
  renames its whole home or Codex home away while its own session runs in it could make a new
  one; recorded as a limit.
- **Antigravity's per-run home is protected too**, though nothing there outlives the run, so a
  rule written mid-session does not change the session's next turns either.
- **The stopgap: cleared by the update's host check** (MEM-009), because an immutable folder
  breaks the purge and is no longer needed.

## Evidence

First pull request (MEM-001 to MEM-005, MEM-007, MEM-008; the refusal sentence and the stopgap's
clean-up, MEM-006 and MEM-009, follow in the second).

- Real CLIs. Claude Code 2.1.332 against a local stub API (`claude -p` with `--settings`):
  a note in the default `projects/<key>/memory` loads (1 request holds it); with
  `autoMemoryDirectory` naming another folder, that folder's note loads and the default one does
  not, so pinning it keeps reading on the guarded folder. Codex 0.170.0 `codex debug
  prompt-input`: with an empty `AGENTS.override.md`, `AGENTS.md`'s text is in the instructions;
  with text in the override, the override's. `codex features list`: `memories` stable, off.
  Antigravity 1.5.12 (`strings`): rules from `~/.gemini/config/{rules/,GEMINI.md,AGENTS.md}` and
  `~/.gemini/GEMINI.md`; no local memory folder.
- Real namespace, Ubuntu 26.04 QEMU guest (kernel 7.0.0-43, bubblewrap 0.11.1), test
  `memory_guard::a_guarded_session_cannot_write_its_memory_but_reads_it` through `yulius ptyhost`
  → `yulius sandbox-exec --guard` → bwrap, with a Claude Code, a Codex and an Antigravity layout:
  as root and as an unprivileged user (bwrap making the user namespace), 1 passed each; 50
  attempts refused each (append, replace by rename, truncate, delete, hard link, Python
  `open`/`unlink` on `MEMORY.md`, `CLAUDE.md`, Codex `AGENTS.md`, Antigravity `AGENTS.md`; create,
  `mkdir`, Python create and `rm -rf` in the memory folder, `rules`, Codex `memories` and
  Antigravity `rules`; renaming the memory folder, its key folder, `projects` and `.claude`;
  `unshare -Urm` remount read-write and `umount`), every note read back, a link to a host file
  and a link to a folder read empty, no descriptor reached the run, the worktree stayed writable,
  and the sandbox with its notes was removed.
- Mutation by hand: without the read-only binds' `--args 199` the same guest test failed with 32
  writes that went through; restored.
- Placeholders found on the guest first: a link whose target the namespace lacks, at a guarded
  path, made bwrap refuse to start (`Can't create file at …`); fixed test-first by moving such an
  entry aside (`guard::tests::links_and_wrong_kinds_are_moved_aside`).
- `scripts/cargo nextest run -p yulius-contracts -p yulius-sandbox -p yulius-ptyhost -p
  yulius-harness -p yulius-app`: 1439 passed, 2 skipped. `-p yulius` outside the main binary: 706
  passed. Main binary, files `memory_guard`, `global_superagent`, `playground`, `cockpit`,
  `codex`, `antigravity`, `supervision`, `shells`, `isolation`, `docker_smoke`, `live_display`:
  267 passed (the real-namespace test skips in a task's sandbox: "Creating new namespace failed").
- Review round 1 (request changes; every finding answered in code):
  - Real CLI, Claude Code 2.1.332 against the stub API: a `CLAUDE.md` and a `.claude/rules` file
    in a folder above the working folder load (1 request each); with `claudeMdExcludes` naming
    them in `--settings`, neither does (0).
  - Real host, Ubuntu 26.04 QEMU guest after `yulius host setup` (service user `yulius`,
    subordinate ids, AppArmor rule, base-1, Docker 29.10.1; the doctor's new line: "ok
    bubblewrap 0.11.1"), the isolation suite as the service user in a delegated transient unit:
    - `isolation_end_to_end` passed: in a joined agent run (the holder's namespaces, seccomp
      filter, reduced capabilities) `mem-append`, `mem-create`, `mem-delete`, `mem-rename` and
      `claude-md` were refused and `mem-read=existing note`.
    - `isolation_docker`: `docker run -v <home>/.claude:/c -v <home>:/h` saw `c-entries=0
      h-claude=0`; writing `/h/.claude/CLAUDE.md` gave "Read-only file system"; the host note
      was unchanged. With the engine's masks removed by hand the same check saw 5 entries and
      failed. The test then stopped at its existing `--privileged` step ("error mounting sysfs"),
      which fails the same way without the masks: #1765.
  - The hardened real-namespace test (tools required, every refusal's whole message checked) on
    the guest as root and as the service user: 1 passed each, 50 refusals each (19 "Read-only
    file system", 13 "Device or resource busy", 4 "Invalid cross-device link", the rest Python's
    and `mount`'s "permission denied" and `umount`'s "not mounted" of a locked mount).
  - `scripts/cargo nextest run` on the five touched library crates: 1443 passed; `-p yulius`
    outside the main binary: 707 passed; main binary files `memory_guard`, `global_superagent`,
    `playground`, `cockpit`, `codex`, `antigravity`, `supervision`, `shells`, `isolation`,
    `docker_smoke`, `live_display`, `local_ci`, `watches`: 420 passed.
- `scripts/cargo clippy … --all-targets -D warnings` on the touched crates, `scripts/dev macos`,
  `RUSTDOCFLAGS=-D warnings cargo doc --no-deps` on the touched crates and `scripts/dev quick`:
  clean.
