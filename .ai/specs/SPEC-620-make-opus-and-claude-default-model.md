---
yulius-task: 5632227f-3279-48cb-9c18-3804eab24fc3
status: done
---

# SPEC-620 — Claude Code on Opus 5.5 is the default for new tasks

ADR: [0457](../../docs/decisions/0457-new-tasks-default-to-claude-code-on-opus-5-5.md)

## Goal

A task that a person starts without choosing an agent runs Claude Code on Opus 5.5 at high
effort: from the composer in a browser with no saved choice, from `yulius task create`, from
either Superagent, from an approved cross-sandbox request and from a Telegram message. The
person's request of
2026-11-22 (task #604): "Make opus and claude default model".

## Context

- [SPEC-583](SPEC-583-cli-nad-superagents-should-spawn-tasks-i.md)
  ([ADR-0422](../../docs/decisions/0422-cli-and-superagent-task-creation-defaults-to-codex.md))
  made `task_create` choose Codex when a caller without a task (the CLI through remote MCP, the
  sandbox Superagent, the Global Superagent, an approved cross-sandbox request) omits both
  `harness` and `model` (`crates/yulius-app/src/manage.rs`, `tool_task_create`).
- [SPEC-587](SPEC-587-codex-should-be-default-for-example-bart.md)
  ([ADR-0426](../../docs/decisions/0426-the-new-task-composer-defaults-to-codex.md)) made
  `modelPreferences()` (`crates/yulius-server/web/assets/models.js`), shared by the composer and
  Settings, answer `codex` (Codex default) when the browser has no saved model, an unreadable
  one or one no longer offered.
- The Claude defaults already are Opus 5.5 at high effort: `DEFAULT_CLAUDE_MODEL`
  (`claude-opus-5-5`, SPEC-493) and `DEFAULT_CLAUDE_EFFORT` (`high`, SPEC-462) in
  `crates/yulius-domain/src/harness.rs`. A Claude task without a model or effort runs them, and
  the first Anthropic model of `models.js` is `claude-opus-5-5`, which `ui_contract` keeps equal
  to `DEFAULT_CLAUDE_MODEL`.
- Out of scope: the Random draw itself (SPEC-520), which stays an explicit choice; the HTTP API's
  default for an absent `harness` and `model` (`auto`: the composer's Random sends no fields, so
  that default is Random's wire form); automations, which default to `auto` (SPEC-520 RHM-005);
  a running task's follow-ups, which keep their parent's agent (RHM-004); an agent's
  `sandbox_handoff` without a harness (`handoffs.rs`) and Yulius's own bootstrap upgrade task
  (`bootstrap_upgrade.rs`), which no person starts and which still draw.

## Requirements

- **CDM-001** `task_create` called with neither `harness` nor `model` by a caller without a task
  (the CLI, the sandbox Superagent, the Global Superagent, an approved cross-sandbox request),
  and a task a person starts with a Telegram message (SPEC-512),
  creates a Claude Code task with `harness_choice: "chosen"` and no model or effort override, so
  it runs `DEFAULT_CLAUDE_MODEL` (Opus 5.5) at `DEFAULT_CLAUDE_EFFORT` (high). Starting it needs
  a usable Claude subscription; without one the ordinary refusal names Claude.
- **CDM-002** A named harness, a model that implies one, an explicit effort and an explicit
  `auto` (Random) are honoured as before; a running task's follow-up keeps its parent's agent;
  automations and the HTTP API's absent harness keep their `auto` defaults.
- **CDM-003** In a browser with no saved model, an unreadable one or one no longer offered, the
  composer and Settings show Opus 5.5, and a task started untouched is a chosen Claude Code task
  on `claude-opus-5-5` at the inherited high effort. A saved model (Codex default included) and a
  saved Random choice (JSON null) are kept across visits. With no usable subscription the
  untouched composer offers Connect Claude.
- **CDM-004** The `task_create` and `sandbox_task_create` tool descriptions, the browser mock and
  the product, architecture, API and remote-control documents describe Claude Code on Opus 5.5
  as the default.

## Plan

- [x] Write and validate SPEC-620 and its ADR
- [x] CLI, Superagent and cross-sandbox tasks default to Claude Code on Opus 5.5
- [x] The composer and Settings start on Opus 5.5
- [x] Product, architecture, API and remote-control docs say Claude Code is the default
- [x] Independent review in a fresh session, findings fixed test-first
- [x] Every gate green, docs and the changes log updated

## Acceptance criteria

- **AC-1** (CDM-001) With Claude, Codex and Antigravity subscriptions all usable, the Superagent's
  `task_create` without `harness` or `model` records `claude` / `chosen`, and with only Codex and
  Antigravity subscriptions it is refused naming Claude and creates nothing; the CLI's
  `yulius task create`, with Codex usable too, creates a task whose details show
  `harness: "claude"`, `harness_choice: "chosen"`, `model: "claude-opus-5-5"`,
  `model_default: true`, `effort: "high"`, `effort_default: true`. The Global Superagent, an
  approved cross-sandbox request and a Telegram message (with Codex usable too) record
  `claude` / `chosen`.
- **AC-2** (CDM-002) On the same callers, `model: "gemini-4-neon"` creates an Antigravity task,
  an explicit effort is kept, `harness: "auto"` draws (`random`), and a running Antigravity task's
  follow-up stays Antigravity.
- **AC-3** (CDM-003) In Chromium with empty storage, Settings and the composer show Opus 5.5; the
  started task's request carries `model: "claude-opus-5-5"` and no `harness` or `effort`, and the
  task is `claude` / `chosen` on `claude-opus-5-5` (`model_default: false`, as the browser names
  it) at effort `high`. An unrecognised and an unreadable saved model
  recover to Opus 5.5; a saved Random and a saved Codex default are kept. With no account the
  composer links Connect Claude and a refused start offers Connect Claude.
- **AC-4** (CDM-004) The tool descriptors' unit tests read Claude Code as the default and no
  longer Codex; `scripts/dev quick` passes.

## Test plan

- Rust, red first: `crates/yulius/tests/harness_draw.rs` (Superagent default, explicit choices
  and the refusal without a Claude account), `remote_mcp.rs` (the real CLI against remote MCP,
  default details with Codex usable too), `telegram.rs` (a message's task),
  `cockpit.rs`, `global_superagent.rs`, `cross_sandbox.rs`, `cross_sandbox_live.rs` (defaults
  recorded as Claude Code, the Codex fixture no longer needed), and the `mcp.rs` descriptor unit
  tests. Run each touched binary's tests in full (`yulius::main` filtered by file,
  `yulius-app`'s lib tests).
- Browser: `e2e/tests/model-choices.e2e.ts` (untouched default, recovery, empty-account
  connection, saved Random and Codex kept), `superagent-requests.e2e.ts` (approved request
  defaults), and the composer tests SPEC-587 gave `&codex=1` (drafts, filedrop, inline images,
  intake, new task, prepared, start change, supertask), now starting on the mock's Claude account.
- `scripts/dev quick`; Yulius CI runs the gate.

## Decisions

- **Reverse both Codex defaults, keep Random explicit
  ([ADR-0457](../../docs/decisions/0457-new-tasks-default-to-claude-code-on-opus-5-5.md),
  superseding ADR-0422 and ADR-0426).** The request names the default model; the places that
  default to Codex today are the CLI/Superagent rule and the browser's untouched choice. Changing
  the HTTP API's absent-harness default would turn the composer's Random (which sends no fields)
  into Claude, and changing automations would rewrite a contract the request does not mention.
- **The CLI and Superagents leave model and effort unset.** The existing launch defaults supply
  Opus 5.5 at high, and the task reports them as inherited (`model_default`), as SPEC-583 did for
  Codex.
- **The browser's untouched choice is the Opus 5.5 item** (`PROVIDERS[0].models[0]`, which
  `ui_contract` keeps equal to `DEFAULT_CLAUDE_MODEL`), sent as `model: "claude-opus-5-5"`. A
  harness-only "Claude default" item would list the same model twice and break the menu's
  one-to-one list with `OFFERED_CLAUDE_MODELS`. Saved preferences are read, never rewritten, so a
  person who chose Codex default or Random keeps it.
- **A Telegram message names Claude Code** (found in review: it drew, so a person's task with no
  chooser could land on Codex or Antigravity). It names the harness rather than a model, so the
  task inherits Opus 5.5 at high like the CLI's. Handoffs and the bootstrap upgrade task are not a
  person's start and keep their rules.
- **The browser mock reports Claude Code's real defaults** (Opus 5.5 at high instead of a stale
  `claude-opus-4` / `default`) for a task that names no model, and `ui_contract` pins it.
- **`url` 2.5.9 in the same pull request.** RUSTSEC-2026-0592 (`url` 2.5.8) failed `deps` on
  this pull request's first CI run and on every other until the lock moves; no parallel pull
  request bumped it. Its own commit; 2.5.9 changes only `blob:` origins, and Yulius's three
  `origin()` callers handle http(s) URLs.

## Evidence

- **Red first.** Before the code change: `scripts/cargo nextest run -p yulius-app --lib -E
  'test(/mcp::tests::(a_task_asked_for_elsewhere|task_create_names_the_default)/)'` 0 passed,
  2 failed; the six changed `yulius::main` tests (`harness_draw`, `remote_mcp`, `cockpit`,
  `global_superagent`, `cross_sandbox`, `cross_sandbox_live`) 0 passed, 6 failed, each on Codex
  (`("codex", "chosen")`, `Codex`, or "Connect a Codex account first"); six browser tests in
  `model-choices`, `antigravity` and `claude-connect` failed on the old `models.js` default;
  `telegram::with_intake_a_message_starts_a_task_and_a_photo_is_its_attachment` failed with
  `("claude", "random")` against `("claude", "chosen")`.
- **Hand mutations.** Putting `manage.rs`'s default back to `Harness::Codex` failed
  `harness_draw::a_superagent_default_without_a_claude_account_is_refused_naming_claude`; putting
  `mock.js`'s approved-request default back to `'codex'` failed `superagent-requests.e2e.ts`'s
  "approved tasks inherit the Claude Code defaults" (`"harness":"codex"`). Both restored.
- **Green.** `yulius-app` lib tests 701/701. `yulius::main` files `harness_draw`, `remote_mcp`,
  `cockpit`, `global_superagent`, `cross_sandbox`, `cross_sandbox_live`, `manage_tools`, `codex`
  261/261; `telegram` 37/37; `ui_contract` 12/12. Full e2e (`cd e2e && E2E_TELEMETRY_DISABLED=1
  npx e2e run`): 1325 passed, 1 skipped (`agent.e2e.ts`, needs a model credential);
  `model-choices.e2e.ts` 9/9 after the review fixes. `scripts/dev quick` passed;
  `scripts/dev deps` failed on RUSTSEC-2026-0592 before the `url` bump and passed after it.
- **Security scan.** `yulius security-scan`: "Security scan disabled: Vidoc is turned off in
  Yulius"; nothing was scanned (disabled, not passed).
- **CI.** Run `c9adaf24` on `07e2dbcfd` failed in `deps` only (the advisory); run `f48663dd` on
  `ba0e6b009` passed quality, lint, macos, docs, deps and file-contracts; the test shards and e2e
  run on the merged revision.
- **Independent review** (`yulius-reviewer`, Fable 5.2) of `origin/dev...ba0e6b009`:
  "Verdict: approve", no blocker or major. Answered: the minor on Telegram intake (now Claude Code,
  test-first) and on handoffs and the bootstrap upgrade (listed out of scope); the minor on AC-1's
  CLI refusal (AC-1 and the Test plan now name the Superagent refusal test, which exercises the
  same handler; the CLI test asserts the default with Codex usable too); the minor on
  `model_default` (pinned `false` in `model-choices.e2e.ts`); the nit on `ui_contract`'s joined
  assertion (split). Left as they are: the mock's `high` for a Haiku task without an effort
  (pre-existing in effect: the mock's detail view showed `high` before), and the documented
  manual `&codex=1` mock parameter.
- **Not tested live.** No real Anthropic, OpenAI or Telegram account was used; subscriptions,
  the Telegram bot and the agent are synthetic fixtures, fakes and the browser mock.
