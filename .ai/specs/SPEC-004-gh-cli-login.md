---
yulius-task: b5fe233c-1c07-47fd-a30b-9dfe4d2550aa
status: done
---

# SPEC-004: GitHub CLI logged in with the user's GitHub login

Status: implemented
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
- Agents asked the same route with their task token and got an installation token. Since
  F 2026-10-11 (SPEC-001 GIT-003) the route answers a task token with the task creator's own
  token, and agents get the shim too (GH-006).

## Requirements

| ID | Requirement | Evidence |
| --- | --- | --- |
| GH-001 | In a user's shell or editor terminal, `gh` finds the user's own GitHub token without `gh auth login`; the token is obtained per invocation through the existing workspace credential route and is never written to disk or into the process environment of the shell. | `gh_shim_*` tests in identity.rs; `gh_token` tests in yulius-git |
| GH-002 | The token comes from the same route and checks as Git's: only for users that may still work in the sandbox; a refusal or an unreachable server leaves `gh` unauthenticated (its own message), never with a stale token. | credential tests |
| GH-003 | A `GH_TOKEN` or `GITHUB_TOKEN` the user set themselves wins; the shim does not override it. | shim test |
| GH-004 | When `gh` is not installed the shim says so and exits 127; it never recurses into itself. | shim test |
| GH-005 | Works in contained and uncontained (development) user processes. | env/PATH tests |
| GH-006 | In an agent run, `gh` is logged in as the task's creator: the same shim first on the run's `PATH`, whose `yulius gh-token` asks with the run's task token; without a usable creator token, the App's token for the sandbox's repository (SPEC-001 GIT-003). GH-002..004 apply unchanged. | `identity::agent_runs_get_the_gh_shim_first_on_path`; `agent_hooks.rs::the_agents_git_acts_as_the_task_creator` (path-less request); `supervision.rs` (shim written for a run) |

## Design

- `yulius gh-token` (hidden): prints a GitHub token for github.com via `credential_get`
  (`YULIUS_URL` + `YULIUS_TASK_TOKEN`, 5 s bound), exits 1/2 like the git helper, prints nothing on failure.
- `GH_SHIM`: POSIX `sh` script `gh`, written next to the `sudo` shim with the absolute path of the
  Yulius executable. It finds the real `gh` as the first `gh` on `PATH` that is not a copy of the shim (recognised by its header line, so symlinked or doubled shim directories never loop),
  fetches the token (unless `GH_TOKEN`/`GITHUB_TOKEN` is set) and `exec`s `gh` with `GH_TOKEN`
  set for that process only. `GH_PROMPT_DISABLED` is not forced.
- Contained: the shim directory is already bound at `/yulius/bin` and first on `PATH`.
  Uncontained: a directory holding only the `gh` shim is put first on `PATH` (the `sudo` shim must
  not shadow the host's `sudo`).
- Agent runs (GH-006): the directory holding only the `gh` shim is bound read-only at
  `/yulius/bin` in a contained run (nothing else of `runtime/` enters it) or used by its host path
  in an uncontained one, and put first on the run's final `PATH`.
- `gh` hands `GH_TOKEN` to its own children (aliases, extensions); that is inherent and the token
  is the user's own, scoped by their GitHub access.
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
- yulius-app: shim run with an exe path containing a space and a quote, an empty `PATH` entry, two shim directories and a symlinked one.
- Not covered by an automated test: the uncontained `PATH` wiring in `user_space` (a one-line prefix, checked by review) and a real `gh` binary (not installed in CI).
- Shim run with a fake `gh` that echoes `GH_TOKEN`: token injected; pre-set token preserved; missing gh → 127.

## Plan

- [x] `yulius gh-token` prints the user's token or fails closed
- [x] The `gh` shim exists in user processes (contained and uncontained) and logs `gh` in
- [x] Docs, ADR note and changes log updated; every gate green

## Decisions

- Shim + per-call token over `hosts.yml`: nothing on disk, tokens stay fresh (user tokens expire),
  access is re-checked at each use. Rejected: writing `hosts.yml` (stale, secret at rest); setting
  `GH_TOKEN` in the shell environment (leaks to every child process and goes stale).

## Evidence

- GH-006 (F 2026-10-11, #188): `cargo nextest run -p yulius-app -E 'test(agent_runs_get_the_gh_shim)'`
  and `-p yulius --test agent_hooks --test supervision`: passed. Not run: a real `gh` in an agent run.

- `cargo test -p yulius-git --test credential_helper`: 8 passed (2 new `gh_token_*`).
- `cargo test -p yulius-app --lib identity`: 9 passed (6 new `gh_shim_*`).
- `scripts/dev lint` (fmt + clippy): clean.
- `cargo nextest run --workspace --all-features --profile ci`: 1091 of 1093 passed; the
  failures are `wiring::system::tests::reads_real_machine_metrics` ("disk total", this sandbox
  exposes no disk totals; unrelated) and `supervision::a_session_change_inside_claude_is_adopted_into_the_task_lineage`
  (failed once under full-suite load, passes alone; unrelated).
- Independent review (fresh session): approve; minor findings on `PATH` globbing, empty entries
  and recursion through differently spelled shim directories fixed in the shim with a test.
- Not run: a real sign-in with GitHub and a real `gh` (no GitHub App or `gh` here).
