---
yulius-task: b5fe233c-1c07-47fd-a30b-9dfe4d2550aa
status: draft
---

# SPEC-004: GitHub CLI logged in with the user's GitHub login

Status: draft
Date: 2026-10-11
Owner: Yulius task #2
Scope authorization: user request ("github cli should get logged in automatically when the
user logs in using github")

## Goal

A user who signed in to Yulius with GitHub finds `gh` already authenticated as themselves in
every shell and editor terminal of a sandbox: `gh auth status`, `gh pr list` and `gh issue
create` work with no `gh auth login`, exactly as `git push` already does (UI-016, EDT-003).

## Context

- User processes get Git credentials through `yulius git-credential`, which asks the internal
  listener with a workspace token (`ws_…`); the listener maps it to the user's own GitHub token
  and re-checks access on every use ([`App::workspace_git_credential`], ADR-12). Nothing is stored
  on disk.
- `gh` has no credential-helper hook; it reads `GH_TOKEN` or `~/.config/gh/hosts.yml`.
- User processes already have `/yulius/bin` first on `PATH` (the `sudo` shim, contained mode).
- Agents use task tokens and installation tokens; they are out of scope.

## Requirements

| ID | Requirement | Evidence |
| --- | --- | --- |
| GH-001 | In a user's shell or editor terminal, `gh` finds the user's own GitHub token without `gh auth login`; the token is obtained per invocation through the existing workspace credential route and is never written to disk or into the process environment of the shell. | `gh_shim_*` tests in identity.rs; `gh_token` tests in yulius-git |
| GH-002 | The token comes from the same route and checks as Git's: only for users that may still work in the sandbox; a refusal or an unreachable server leaves `gh` unauthenticated (its own message), never with a stale token. | credential tests |
| GH-003 | A `GH_TOKEN` or `GITHUB_TOKEN` the user set themselves wins; the shim does not override it. | shim test |
| GH-004 | When `gh` is not installed the shim says so and exits 127; it never recurses into itself. | shim test |
| GH-005 | Works in contained and uncontained (development) user processes. | env/PATH tests |

## Design

- `yulius gh-token` (hidden): prints a GitHub token for github.com via `credential_get`
  (`YULIUS_URL` + `YULIUS_TASK_TOKEN`, 5 s bound), exits 1/2 like the git helper, prints nothing on failure.
- `GH_SHIM`: POSIX `sh` script `gh`, written next to the `sudo` shim with the absolute path of the
  Yulius executable. It finds the real `gh` as the first `gh` on `PATH` outside its own directory,
  fetches the token (unless `GH_TOKEN`/`GITHUB_TOKEN` is set) and `exec`s `gh` with `GH_TOKEN`
  set for that process only. `GH_PROMPT_DISABLED` is not forced.
- Contained: the shim directory is already bound at `/yulius/bin` and first on `PATH`.
  Uncontained: a directory holding only the `gh` shim is put first on `PATH` (the `sudo` shim must
  not shadow the host's `sudo`).
- A repository path is not sent: `gh` talks to the API across repositories; the token is the user's
  own and GitHub enforces its scope.

## Failure and compatibility

Server down, user disabled or access lost: `gh-token` fails, `gh` runs unauthenticated. Existing
Git helper unchanged. Existing homes need no migration (shim is rewritten when its text differs).

## Acceptance criteria

1. As a signed-in user, `gh auth status` inside a sandbox shell reports logged in (via `GH_TOKEN`).
2. With a refused credential, `gh-token` prints nothing and exits non-zero.
3. A user-set `GH_TOKEN` is passed through untouched.

## Test plan

- yulius-git: `gh_token` prints the token on 200, nothing on 403, rejects non-loopback URLs.
- yulius-app: shim text is written with the exe path quoted; uncontained PATH starts with the gh-only dir.
- Shim run with a fake `gh` that echoes `GH_TOKEN`: token injected; pre-set token preserved; missing gh → 127.

## Plan

- [ ] `yulius gh-token` prints the user's token or fails closed
- [ ] The `gh` shim exists in user processes (contained and uncontained) and logs `gh` in
- [ ] Docs, ADR note and changes log updated; every gate green

## Decisions

- Shim + per-call token over `hosts.yml`: nothing on disk, tokens stay fresh (user tokens expire),
  access is re-checked at each use. Rejected: writing `hosts.yml` (stale, secret at rest); setting
  `GH_TOKEN` in the shell environment (leaks to every child process and goes stale).

## Evidence

Pending.
