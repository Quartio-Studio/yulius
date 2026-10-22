---
yulius-task: 027750d0-f1ed-4a0d-8b88-bfdcad37fb80
status: in-progress
---

# SPEC-262: Triage of the Vidoc findings in the agent-secret code (#639)

Status: in-progress
Date: 2026-10-22
Owner: Yulius task #248
Scope authorization: task #248 (nightly "Work through open issues"). Fixes #639; the CI-filed
copies of three of its items are #688, #697 and #714.

## Goal

Each of the nine unconfirmed Vidoc findings tracked in #639 (four in its body, five in its
comments) is either fixed test-first or dismissed with its reason and a narrow `[[ignore]]` entry
(one per wording) in `.config/vidoc.toml`.

## Context

- The findings were reported by task #200's change scans (SPEC-213) in code that change did not
  touch. The automation's read-only verification on `dev` at 5f3e7f1f is in the task request.
- SPEC-134 (round 2, M-B): a secret agents can use (`env_files = allowed` or bound hosts) is no
  production secret (`cd_prod`), and the reverse. The app checks it on the record it read; each
  store compare-and-set (`yulius-store/src/vault.rs`) matches only its own fields.
- `vault/requests.rs` `reason()` cleans the reason an agent gives before it reaches audit lines.
- `hooks.rs` reads repository documents for the briefing outside containment (#70); `read_repo_doc`
  re-checks the opened file's real path, `repo_doc_exists` and `actions_workflows` do not.
  `files::open_inside` (#657, ADR-0174) is the shared checked open.
- Vidoc's `[[ignore]]` matches the path exactly and the title as a case-insensitive substring
  (`scripts/vidoc_scan.py` `apply_ignores`); Vidoc rewords findings between scans.

## Requirements

- VTR-001 (finding 1, real). The store refuses, in the same statement that changes it, to set
  `cd_prod` on a secret that is allowed in `.env` files or bound to hosts, to allow `.env` files on
  a production secret, and to add hosts to a production secret (a request's answer or an edit), so
  two managers answering at once can never leave a secret both production-only and agent-usable.
- VTR-002 (finding 4, real). `reason()` drops every Unicode format (Cf) character the audit could
  hide text with, and turns line and paragraph separators (U+2028, U+2029) and controls into
  spaces: U+00AD, U+061C, U+180E, U+2028/U+2029, the bidi and zero-width ranges, U+FEFF, the
  interlinear annotation characters and the tag characters never reach an audit line.
- VTR-003 (`status_line`, #688, real but minor). Control characters (C0, DEL, C1) of the
  repository name and branch never reach the coloured status line.
- VTR-004 (`repo_doc_exists` / `actions_workflows`, #697, real but minor). Both read through a
  handle opened with `files::open_inside`, so a folder swapped for a link after the path check
  cannot make the server report a host file as a repository document or count a host folder's
  workflow files.
- VTR-005 (dismissals). Finding 2 (`is_disposable` checked once), finding 3 (`patterns()` drops
  unparseable stored hosts), the `.git` exclusion of `open_folder` (#714) and task-number
  allocation are dismissed with their reasons (Decisions). Each whose Vidoc wording is on record
  gets an `[[ignore]]` entry with that wording; finding 3's wording was not kept anywhere, and no
  title is guessed for it (an over-broad one would hide a regression). The `.git` refusal and the
  parsed request are pinned by tests.

## Plan

- [x] Spec SPEC-262 with the triage of all nine findings
- [x] Production and agent use exclude each other in every store compare-and-set (finding 1)
- [x] Audit reasons drop line separators and every invisible format character (finding 4)
- [x] Status line strips control characters from repository and branch names
- [x] Document presence and workflow count read through a checked handle (hooks TOCTOU)
- [x] Dismissals pinned by tests and narrow [[ignore]] entries (findings 2, 3, .git, task numbers)
- [x] Gates, security scan, docs and changes entry
- [ ] Independent review answered and merged into dev on green

## Acceptance criteria

- A store test where each side's read record is out of date fails on `dev` (the stale CAS
  succeeds) and passes with VTR-001.
- `reasons_are_short_single_lines` with U+2028, U+2029, U+00AD, U+061C, U+180E and a tag character
  fails on `dev` and passes.
- A status-line test with ESC and C1 CSI in the branch fails on `dev` and passes.
- A test that swaps a folder for a link to a host folder between the check and the open (driven
  through the helpers' handle open) shows neither helper reports outside content.
- `.config/vidoc.toml` has one narrow entry per dismissed wording on record; the broad scan
  (`scripts/dev security --all`) reports none of the nine findings, or it is recorded as not run.

## Test plan

- `yulius-store` unit test in `vault.rs`: stale records on both sides of every pair of CAS calls.
- `requests.rs` unit test extended; `hooks.rs` unit tests for the status line text and the
  checked opens; `env_files.rs` unit test that `open_folder` refuses a `.git` folder reached by its
  real path.
- `scripts/dev ci` (the full gate) and `scripts/dev security`.

## Decisions

- Finding 1 is fixed in SQL, not with a lock in the app: the store already serialises writers on
  one thread, and a condition in each `UPDATE` holds also for paths added later.
- Finding 2 dismissed: `is_disposable` keeps a person's `.env` values out of an *accidental*
  commit; it is no boundary against the agent, which controls `.gitignore` and can un-ignore the
  file right after any write just as well as before it, so a re-check right before the write adds
  nothing. The values are still never shown to the agent (`[secret:NAME]`), and a person allowed
  the copy (ADR-0105).
- Finding 3 dismissed: `requested_hosts` is written only by `merge`/`auto_bind` with hosts that
  already parsed (`HostPattern::to_string`), so every stored entry parses; were one not to (a
  future tighter parser), allowing re-parses the whole list with `parse_hosts`, which refuses it,
  so it is never bound, and dismissing only clears it.
- `.git` exclusion (#714) dismissed: `relative_components` refuses a `.git` component and
  `files::inside` refuses any real path with one, before anything is read or written.
- Task numbers dismissed: `MAX(number) + 1` and the `INSERT` run in one transaction on the store's
  single database thread, and `UNIQUE (sandbox_id, number)` refuses a duplicate.
- Status line and the hooks TOCTOU are fixed rather than dismissed: each fix is a few lines on
  existing helpers, cheaper than reasoning about every terminal and every caller.

## Evidence

- Regression tests, each seen failing before its fix:
  `vault::tests::production_and_agent_use_exclude_each_other_from_stale_reads` (yulius-store:
  the stale production allowance succeeded on a secret with `.env` copies allowed),
  `vault::requests::tests::reasons_are_short_single_lines` (U+2028 kept),
  `hooks::tests::status_line_text_carries_no_control_characters` (ESC, BEL, C1 CSI, DEL and a
  newline kept), `hooks::tests::a_folder_swapped_for_a_link_after_resolving_is_not_followed` (the
  host file was reported as a document). Pinning the dismissals (pass as they are):
  `env_files::tests::git_folders_are_refused`, `requests::tests::stored_requests_hold_only_hosts_that_parse`.
- The store rule broke three integration tests that made a production secret bound to a host
  through `vault_set_cd_prod`; they prove the runtime never uses such a row, so they now write it
  back whole (`support::legacy_production_secret`), as a row from before SPEC-134 M-B could be.
- `scripts/dev ci` at b331fe3f: exit 0, 2409 tests run, 2409 passed, 17 skipped; clippy (Linux
  and macOS), docs, repository contracts passed.
- `scripts/dev security` (change scan) at b331fe3f: exit 0, 13 files changed, 10 sent; no finding.
- `scripts/dev security --all` at 94179773 (all fixes in): exit 1 for 2 high, 19 medium and 34 low
  findings elsewhere in the repository, none of them in `vault/env_files.rs`, `vault/requests.rs`,
  `hooks.rs`, `yulius-store/src/tasks.rs` or `yulius-store/src/vault.rs` (all of which were sent):
  none of the nine #639 findings is reported. Those others are outside #639 (CI files each once).
- `[[ignore]]` entries added for the three dismissed wordings on record (task #200's scans, #714):
  "TOCTOU on the Git disposable check before writing secret values", "open_folder does not exclude
  .git despite documented invariant", "Non-atomic task number allocation".
- **Independent review, round 1 (approve at f1610ec9; minors and nits fixed).** Minor 1 (own): an
  edit binding hosts, refused because a person allowed the secret for production meanwhile, asked
  for a reload that would fail again; `agents::update_refused` now names the production rule, test
  `vault::agents::tests::an_edit_refused_by_a_production_allowance_meanwhile_says_so` (failed
  first). Minor 2 (own): the workflow listing dropped read errors before its bound, so a folder
  whose reads keep failing could loop; the bound now comes first (a persistent read error cannot
  be produced in a test). Nits: `vault_auto_bind` also requires `cd_prod = 0` in its own statement;
  `legacy_production_secret` asserts a project secret (deleting another would drop its switches);
  `doc_present` documents that a file the server cannot open is absent; this Plan item ticked.
