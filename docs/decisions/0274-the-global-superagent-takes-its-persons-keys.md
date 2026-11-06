# 0274. The Global Superagent takes its person's keys, like the sandbox Superagent

## Status

Accepted. Specified in [SPEC-430](../../.ai/specs/SPEC-430-this-should-be-exactly-like-normal-super.md);
task #412 ("this should be exactly like normal superagent without some other input", "i cant
click things there, like i cant select 'yes'"). Supersedes the "person talks to it by message, not
keys" part of [0271](0271-a-global-superagent-acts-in-every-sandbox-of-its-person-through-yulius-tools-only.md)
and the message box of [0272](0272-the-two-superagents-are-tabs-of-one-panel.md); everything else
of 0271 stands.

## Context

ADR-0271 made the Global Superagent's terminal read-only and gave it a message box, typed by
Yulius after a label, because Claude Code's keys reach an external editor, `!` commands, plugin
installs and `@` file mentions. In use the person got a terminal that differs from the sandbox
Superagent's, could not click anything in it (the web terminal forwards the mouse only to a
writer), and once Claude Code showed its Bypass Permissions warning at start ("No, exit / Yes, I
accept") the session could not be used at all: neither a click nor a message answers it.

That warning comes from Claude Code 2.1.311 reading the accepted warning only from a settings
source (`skipDangerousModePermissionPrompt`): Yulius stores `bypassPermissionsModeAccepted` in
`.claude.json`, which Claude Code migrates into the user settings file, a file the Global
Superagent deliberately does not read (`--setting-sources ""`).

Every refusal that keeps the session harmless stands on its own, without the read-only terminal:
no built-in tools, only the Yulius MCP (`--strict-mcp-config`), no settings file (so no hooks,
permission rules or enabled plugins), `disableAllHooks`, a refusing script as
`CLAUDE_CODE_SHELL_PREFIX`/`SHELL` (every `!` command) and as `VISUAL`/`EDITOR` (Claude Code takes
`VISUAL`, then `EDITOR`, and falls back to `code`, `vi` or `nano` only when both are unset), and
on a contained server a bubblewrap root whose only writable path is its own folder, with the
host's system folders read-only, `/etc/shadow` and `/etc/gshadow` covered, and no Yulius data
directory, repository or home.

## Decision

- Its terminal takes its person's keys, pastes, resizes and clicks exactly as the sandbox
  Superagent's (`can_write: true`); only its person attaches to it, and every key is checked again
  as for any shell. `POST /api/superagent/message` and `…/interrupt`, with the label, the message
  checks and the interrupt gap, are removed; the web UI's All sandboxes tab is the same view as
  This sandbox, its example ("check what my sandboxes need from each other and solve it") moving
  into its note.
- Every one of its tools stays pre-allowed, on an uncontained server too, although a person could
  now answer a question: its tools are only Yulius's, each checked against its person's access at
  the call, its changes audited, as for the person's own agent over the remote MCP, so a question per call
  would add a click and no safety. It has no built-in tool that a question would guard.
- Its approval bypass carries its consent as a flag setting: `skipDangerousModePermissionPrompt:
  true` in its `--settings` whenever `--dangerously-skip-permissions` is passed. Flag settings are
  read whatever `--setting-sources` says, and nothing the session writes removes them. Its launch
  revision is bumped, so a running one restarts once idle with it.

## Alternatives

- **Keep the box and answer the warning for the person.** Fixes the stuck start but keeps a second
  input the person asked to drop, and every other Claude Code menu stays unreachable.
- **A holder of its own** (0271's next step): the strongest containment, but the isolation adapter
  makes holders only for sandboxes; what keys reach does not need it (below).
- **Filter keys** (refuse `@`, `/plugin`, Ctrl+G sequences): a terminal UI cannot be filtered
  reliably by bytes, and the refusals that matter are already in Claude Code's launch.

## Consequences

- Keys reach Claude Code's own UI: menus, slash commands, `@` mentions of files in its root (on a
  contained server system folders and its own folder, nothing of Yulius's data; on an uncontained
  server anything the service user reads, as every agent there can, operations.md), `/plugin`
  installs that no settings source it reads enables, `/login`, `/model`. Its environment holds the
  subscription token and its own `gs_` bearer, both of which its person already reaches (the
  sandbox Superagent gives the same person a shell with the same kind of token).
- Claude Code runs its shell prefix through a login bash or zsh, which reads startup files from
  its home before the refusing script: on a contained server those files (`.profile`,
  `.bash_profile`, `.bash_login`, `.bashrc`, `.bash_logout`, `.zshenv`, `.zprofile`, `.zshrc`,
  `.zlogin`, `.zlogout`) are empty read-only binds, since keys now reach Claude Code commands that
  write files in its root (`/export`).
- It still runs no command, no hook and no editor, reads no settings file, and works only through
  Yulius's tools, minus the withheld ones; a Claude Code release that ran code on a key would need
  a holder of its own for it, as 0271 says for a pasted message.
- What it relies on was measured on 2.1.311 in a PTY with a synthetic token: without stored
  consent the warning shows, with the flag setting it never does.
