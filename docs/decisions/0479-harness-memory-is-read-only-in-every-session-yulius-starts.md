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
  `autoMemoryDirectory` to `<config>/projects/<key>/memory`, with `<key>` its own rule (the git
  root, a worktree's main repository) and deny `Edit` there; the folder already used keeps
  loading.

## Alternatives

- **An immutable flag on the host** (the stopgap): breaks a removed sandbox's purge and a
  person's edits, and misses folders made later.
- **Deny rules and hooks only**: a script or a subprocess writes past them.
- **`autoMemoryEnabled: false`**: stops loading the notes too.
- **Landlock**: rules only allow; a folder cannot be carved out of a writable home.
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
- An agent that renames its whole home or Codex home away while its session runs in it could
  start a new one there; the session it runs in breaks with it.
- A protected path that is a link shows an empty placeholder instead of the link's target.
