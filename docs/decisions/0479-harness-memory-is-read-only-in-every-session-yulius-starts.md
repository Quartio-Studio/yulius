# 0479. Harness memory is read-only in every session Yulius starts

## Status

Accepted (2026-11-25). Specified in
[SPEC-652](../../.ai/specs/SPEC-652-agents-cannot-write-harness-memory-every.md); task #636 (the
owner's decision of 2026-11-25). Amends SPEC-002 ADR-03 and ADR-22 (what a contained run's
namespace binds).

## Context

- Twice in one week an agent turned a remark about a few tasks into a standing memory note
  ("every subagent runs at max effort", "big tasks do all their work in one task"), and every
  later task followed it for days before anyone found the note.
- Claude Code keeps auto memory in `<config>/projects/<key>/memory/` and user instructions in
  `<config>/CLAUDE.md` and `<config>/rules/`; it has no read-only mode (`autoMemoryEnabled: false`
  stops reading too). Codex reads `AGENTS.override.md`, `AGENTS.md` and `memories/` in its
  `CODEX_HOME`; Antigravity reads `~/.gemini/GEMINI.md` and `GEMINI.md`, `AGENTS.md` and `rules/`
  in `~/.gemini/config/`.
- Claude Code's `permissions.deny` stops only its own tools; a script opens the file itself. The
  host stopgap (`chattr -R +i`) misses new sandboxes and homes and breaks a removed sandbox's
  purge.
- bwrap resolves a bind's source by name when it starts; a folder an agent writes can be swapped
  for a link to a host path in between. bwrap 0.11 binds an open descriptor (`--bind-fd`,
  `--ro-bind-fd`) and reads arguments from a descriptor (`--args`).

## Decision

- **Every contained session binds its harness's memory and user-level instruction paths
  read-only over themselves**: task runs of each harness, a person's Superagent and the Global
  Superagent. The kernel refuses every write, create, rename, link and delete there to every
  process of the session; reads give what is there.
- **Opened, not named.** The launch lists each protected path with a server-made anchor (the
  sandbox folder, the person's home, the Global Superagent's folder, the run's folder). `yulius
  sandbox-exec`, in the namespace view bwrap starts in, opens each from its anchor without
  following a link and writes the binds of those descriptors into two `--args` pipes that
  `bwrap_argv` placed in the argument list: the pins before the binds beneath them, the
  read-only binds after every bind. A missing path, a link or the wrong kind gets an empty
  read-only placeholder, so a session always starts and never shows a link's target.
- **Pins inside Claude Code's configuration.** `<config>`, `<config>/projects` and
  `<config>/projects/<key>` are bound over themselves, so renaming the memory folder's parents
  away fails (`EBUSY`).
- **Claude Code reads exactly the protected folder**: its run settings pin
  `autoMemoryDirectory` to `<config>/projects/<key>/memory`, name it also as
  `CLAUDE_COWORK_MEMORY_PATH_OVERRIDE` in their `env` (which Claude Code reads first, and the
  user's `settings.json` could set otherwise), and deny `Edit` there. `<key>` is
  Claude Code's own key of the sandbox's repository (`<sandbox>/repo`, which is what it uses for
  the repository and every worktree of it), derived from that path alone: nothing the agent writes
  (a worktree's `.git`) steers it or can stall a launch. The folder already used keeps loading.
- **No instructions from above the project.** Claude Code reads `CLAUDE.md`, `CLAUDE.local.md`
  and `.claude/` in every folder up to `/`; the run settings exclude those of every folder above
  the session's folder (`claudeMdExcludes`), so a file an agent leaves in the sandbox folder
  loads nowhere.
- **Every other process of the sandbox that runs code an agent chose** (its Docker engine, so
  a container's `-v`; its CI jobs; its script watches) sees the home's `.claude` and the Codex
  homes as empty read-only folders. None of them needs either. An engine front started before
  this change is replaced once its engine is not running.
- **A start that cannot be guarded does not happen**: a guarded path outside its anchor, or one
  that cannot be opened for another reason than being missing or a link, fails the start.

## Alternatives

- **An immutable flag on the host** (the stopgap): breaks a removed sandbox's purge and a
  person's edits, and misses folders made later.
- **Deny rules and hooks only**: a script or a subprocess writes past them.
- **`autoMemoryEnabled: false`**: stops loading the notes too.
- **Landlock**: rules only allow; a folder cannot be carved out of a writable home.
- **Read-only binds of the real folders in the engine's view**: dockerd has every capability in
  the sandbox's user namespace, and a container's bind is remounted with the flags it asks for, so
  the engine sees an empty folder instead.
- **A server-held copy bound over the path**: two places to edit, and a person's own terminal
  would read the other one.
- **Binding the home or a Codex home over itself too**: also closes renaming the whole home
  away, but makes renames and hard links between the home and the rest of the sandbox fail
  (`EXDEV`: the compiler cache, Codex's rollouts).

## Consequences

- A refused write fails with "Read-only file system" or "Device or resource busy"; the hooks say
  why (SPEC-652 MEM-006).
- A person with access to the host edits memory in place; a person's own terminal in a sandbox
  writes it as before.
- An uncontained server (development) has no namespace: only the deny rules and hooks hold there.
- Limits, none tried in a real session:
  - Renaming the whole home (or a Codex home) away and copying it back (`mv ~ ~.old && cp -a
    ~.old ~`) leaves a writable copy at the same path, which the next session guards with
    whatever was written into it. Pinning the home would make renames and hard links between the
    home and the rest of the sandbox fail (`EXDEV`: the compiler cache, package stores).
  - The engine's masks are submounts in a namespace whose capabilities dockerd holds, so they
    are not locked: a container that binds a folder above `.claude` without its submounts
    (`--mount …,bind-recursive=disabled`), or a privileged one that unmounts the mask, reaches
    the real files. Locking them would mean running dockerd or its containers in a child user
    namespace, which breaks the sandbox's network and the ownership of bound files; filtering
    the Docker API is a larger change of its own.
  - Other user-level settings of Claude Code in `<config>` stay writable: `settings.json` (its
    hooks run in every later session; its `env` reaches every session, though the run settings
    name the memory folder's variable themselves), `agents/`, `skills/`, `commands/` and
    `agent-memory/`.
  - CD runs (reviewed, merged code) and a person's terminals are not masked.
  - Antigravity reads `AGENTS.md` and `GEMINI.md` in folders above its working folder; those
    are not guarded.
  - Memory folders of other keys are not guarded; no session Yulius starts reads them.
- A protected path that is a link shows an empty placeholder instead of the link's target.
