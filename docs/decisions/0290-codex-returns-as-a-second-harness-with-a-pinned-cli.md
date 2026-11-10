# 0290. Codex returns as a second harness, on a Codex CLI Yulius pins and provides

## Status

Proposed, for the founders to decide. Specified in
[SPEC-451](../../.ai/specs/SPEC-451-codex-returns-as-a-second-harness.md). Merging it accepts it;
it then supersedes the Codex part of the founder decision `F 2026-10-08` (SPEC-001: "prune Codex
support and switching providers; we use only Claude; if Codex gets better, we'll support it") and,
for Codex accounts, the "Claude Code only" of [ADR-0005](0005-claude-only-accounts.md), keeping its
per-sandbox homes. Provider switching and session translation stay removed (SPEC-002 ADR-06).
Works with [ADR-0288](0288-a-person-moves-a-task-between-subscriptions-and-removing-one-moves-its-tasks.md):
a Codex task moves only between Codex subscriptions.

## Context

- Codex was removed on 2026-10-08 (commit `9f88975a`): the adapter, Codex homes with `auth.json`
  write-back, its login and usage probe, provider switching, the transcript IR and translation, its
  UI and tests. The reason was the product choice "we use only Claude", with the door left open:
  "if Codex gets better, we'll support it". The adapter then targeted Codex CLI 0.160.1, and three
  risks waited for a live check that never came: refresh-token rotation across homes (R-02), hook
  configuration and trust (R-03), releases without `--no-daemon` (R-12).
- Codex CLI 0.167.0 was checked against what Yulius needs: its help texts, its `rust-v0.167.0`
  source (`codex-rs/{protocol,hooks,login,rollout,config,core}`) and the real CLI on the files
  Yulius generates (it accepted the `-c` overrides, `config.toml` and `hooks.json`; `codex …
  mcp list` listed the servers without a parse error; `codex login status` read an API-key
  `auth.json`). It has `--no-daemon`, hooks for SessionStart, UserPromptSubmit, Stop and PreToolUse
  trusted with `--dangerously-bypass-hook-trust`, device-code sign-in, `codex resume <id>`,
  per-response token usage and `rate_limits` in the rollout, and a turn error
  `usage_limit_exceeded` at a usage limit. Whether Codex's models are now good enough is the
  founders' judgement; this decision makes the choice real and tested.
- More than 700 commits landed on `dev` since the removal; reverting it conflicts in 74 files.
- Sandboxes copy the host system, which normally has no Codex CLI, so a Codex task would fail with
  `codex: not found` unless someone installed it inside that sandbox. Claude Code does not have
  this problem: Yulius binds the host's npm package and keeps a current copy in its data directory
  ([ADR-0266](0266-yulius-keeps-claude-code-current-in-its-data-directory.md)). Codex's argv,
  config keys, hook and rollout formats change between releases.
- A ChatGPT device-code sign-in needs a ChatGPT setting some owners do not want to depend on; a
  sign-in made on a laptop (`CODEX_HOME=~/codex-yulius codex login`) writes the same `auth.json`.

## Decision

- **Harness.** Codex is `Harness::Codex` with `Provider::OpenAi`, re-implemented on today's code
  with the removed code as reference and checked against Codex CLI 0.167.0. A task keeps its harness
  for life. Rotation, a person's move and a removal of a subscription stay within the task's
  provider (SPEC-447); there is no transcript translation and no cross-provider switch.
- **Homes.** Each (sandbox, account) has a Codex home `<sandbox>/codex/<account>` whose `sessions`
  links to the sandbox's shared `<sandbox>/codex/sessions`, so a resumed session finds its rollout
  on any Codex account. Yulius writes the home's `auth.json`, `config.toml` (file credential store,
  update check off, the worktree trusted) and `hooks.json` before each run through handles opened
  without following links. The home's `auth.json` is the agent's to write, so it goes back into
  the vault only as a genuine refresh: the same ChatGPT account and `id_token` subject, a newer
  `last_refresh` not dated ahead of the server's clock, never for an API-key account. A newer copy
  another run of the account left is kept at the next launch, and the file is removed once no run
  of the account is left in the sandbox. The agent's shell does not inherit `CODEX_HOME`, so a
  `codex` it runs does not get Yulius's hooks.
- **Run.** `codex --no-daemon -C <worktree> -c …` with Yulius's MCP as the `yulius mcp` stdio
  bridge; a sandbox MCP server's renamed variables pass through a `sh -c` wrapper, since Codex
  hands a server only variables of their own names. Yulius takes the session id from the
  SessionStart hook, else from the newest rollout of the worktree. A usage limit is the turn's
  `usage_limit_exceeded`; the windows come from `token_count.rate_limits` and from
  `chatgpt.com/backend-api/wham/usage`. A resume after a limit asks Codex to continue, which it
  does not do by itself. Codex takes `/goal` only typed: a Codex goal task gets its whole request
  and definition of done as its first message and is settled by its turns.
- **Sign-in.** `codex login --device-auth` in the server-owned terminal, an OpenAI API key, or the
  imported `auth.json` of a full ChatGPT sign-in made elsewhere (`auth_mode` `chatgpt`,
  `id_token`, `access_token`, `refresh_token`, `account_id`; at creation or by
  `POST /api/admin/accounts/{id}/import`), each sealed as the `auth.json` Codex reads, never
  returned, logged or audited.
- **CLI.** Yulius installs one pinned release, `0.167.0`, into `<data>/codex-cli/<version>-<platform>`:
  the platform tarball from npm's registry, taken only when its sha512 equals the one in the source
  (`yulius_domain::agent_cli::codex`), only the regular files of its `vendor/<triple>/` tree,
  bounded, unpacked under a temporary name and renamed into place. No `npm`, no package script,
  nothing executed on the host. The install runs on the first need (at start when Codex accounts
  exist, at a sign-in, after an account is added by key or import, else at the first Codex run),
  never under the launch or sign-in locks, one at a time; a failure answers for a minute before the
  next try; earlier pins go once no process runs them. Runs and sign-ins start `<dir>/bin/codex` by
  its absolute path, and a contained run binds `<dir>` read-only at the same path. Without a
  provided release, a host that has its own `codex` uses it; without one the start fails with the
  reason.

## Alternatives

- **Keep `F 2026-10-08`** (do nothing): Yulius stays Claude-only; the fork keeps its own copy and
  drifts from `dev` (SPEC-447, merged a day after the fork's version, already needed changes to
  fit Codex).
- **Revert `9f88975a`**: 74 conflicted files against code that changed under them, and an adapter
  written for Codex 0.160.1.
- **Bring back translation between harnesses**: a task could move to whichever provider has
  capacity, at the cost of lossy transcripts and the IR's upkeep; not asked for.
- **One Codex home per sandbox**: two tasks on different accounts in one sandbox would overwrite
  each other's `auth.json`.
- **Codex `exec --json` instead of the TUI**: no native terminal session to watch and type into,
  unlike Claude Code's (HRN-001).
- **Install with `npm i -g` in each sandbox**: needs Node and root in the sandbox, writes the
  agent's system layer, runs package scripts, repeats per sandbox.
- **Bind the host's npm install, as for Claude Code**: needs a person to install Codex on the host
  and keep it at the checked release.
- **Follow npm's `latest`** or **trust the registry's `integrity`**: a release the harness was not
  checked against would reach every Codex run, or the registry's answer would decide what runs.
- **Import through `relogin` with a body**: `relogin` starts a terminal sign-in and answers with
  its id; a separate endpoint keeps both contracts plain.

## Consequences

- Codex runs get no in-sandbox redaction of tool output: Codex's PostToolUse hook cannot replace a
  result, and it has no shell prefix like `CLAUDE_CODE_SHELL_PREFIX`. Agent secrets still reach it
  only as placeholders, but a value copied into the worktree can reach the model.
- Yulius's edit gate (SPEC-426) does not see Codex's `apply_patch`; the shell guard does see its
  `Bash` tool.
- Claude-only features stay Claude-only for Codex tasks: goal tracking, typed openings with pasted
  images, the transcript step-back, the status line, sub-agent tails, session import and search of
  Codex sessions, and #1138's detection of a refused subscription (Codex's wording for it is not
  known; a refused Codex sign-in shows as a failed run).
- Two runs of one account in different sandboxes each refresh its tokens; Codex rotates refresh
  tokens, so one copy can go stale until written back (R-02, unverified live). An imported
  `auth.json` from a laptop that keeps using the same `CODEX_HOME` shares one refresh token with
  Yulius; the UI asks for a separate `CODEX_HOME`.
- About 460 MB in the data directory per installed release, and a 174 MB download on the first
  Codex use of an instance. Codex releases reach runs only through a pin bump in this repository.
  Other architectures than Linux x86-64 and arm64 get no provided CLI and use the host's `codex`;
  the fallback looks on the host's `PATH`, so a `codex` added to the host after the sandbox base
  was built is not seen until the base is rebuilt.
- Should a first Codex start meet the install still running, the queue and capacity loops wait for
  it. An import landing between a cancelled sign-in's last check and its write can be overwritten
  by that sign-in (one store call); a relogin whose start still waits for the CLI is not yet a
  running sign-in, so an import then does not cancel it.
- Not verified without a live Codex account: a real device sign-in, a real rollout and hook
  payloads of the TUI, token refresh and write-back, `wham/usage`, the usage-limit error, an
  imported laptop sign-in used by a run, and the provided CLI in a namespace-isolated sandbox of a
  real instance.
