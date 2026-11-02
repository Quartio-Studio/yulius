---
yulius-task: fd3c7632-5851-4508-b863-eeb4f01b1959
status: in-progress
---

# SPEC-393: Triage of the Vidoc findings on supervisor input and preflight (#818 #752 #692 #691 #693)

Status: in-progress
Date: 2026-11-02
Owner: Yulius task #375
Scope authorization: task #375 (a person asked for each of the five issues to end closed, as a
false positive or as solved, without creating new issues).

## Goal

Each of the five Vidoc findings in `crates/yulius-app/src/supervisor.rs` filed as #818, #752,
#692, #691 and #693 is checked against `dev` (f416b400) and either fixed test-first or dismissed
with its reason, the guard that stops it and the test that pins that guard. A dismissed wording
gets one narrow `[[ignore]]` entry in `.config/vidoc.toml`, so Yulius does not reopen the issue on
the next scan; dead code a finding points at is removed instead.

## Context

- Vidoc files each finding once as an issue and reopens it when it reports the finding again after
  it was closed; a false positive needs an `[[ignore]]` entry (path exact, title a
  case-insensitive substring, SPEC-217, SPEC-273). Vidoc rewords findings between scans, so each
  entry holds the wording on record and stays narrow (SPEC-262 VTR-005): no `function` or `line`,
  which would accept any later finding in `send_input` or `preflight_start`.
- A task's first message with attached images is typed into Claude Code's TUI as bracketed pastes
  (SPEC-030, SPEC-124): `Request::parse(&task.prompt).opening()` (`supervisor.rs:1315-1318`, the
  only producer of `Paste` values outside tests) gives the pastes, `opening_pastes`
  (`supervisor.rs:755-771`) frames each as `ESC [200~ … ESC [201~` and `type_opening`
  (`supervisor.rs:1880`) writes them with `send_input` (`supervisor.rs:3563`).
- Subscription accounts are one instance-wide pool (docs/product.md "Subscription pool"); an
  account row has no owner (`model.rs:526-551`).

## Requirements

- VSI-001 (#752, dismissed). No paste can hold the end delimiter: it starts with ESC, a control
  character, and none reaches a paste. `Request::parse` adds every text piece through `push_text`
  (`request.rs:274-284`), which keeps no control character but tab and newline
  (`without_controls`, `request.rs:287-292`), and takes an attachment path only through
  `valid_path` (`request.rs:338-342`), which refuses any path holding a control character (such a
  line stays text, cleaned). `Paste::Text` is rendered from those parts and mentions of those
  paths (`request.rs:178-215`), `Paste::Image` is such a path. Pinned by `request.rs`
  `a_typed_first_message_carries_no_control_character` (a prompt with `\u{1b}[201~` in its text and
  ESC in an image path: no ESC in any paste, the bad path not attached) and
  `control_characters_never_reach_the_first_message`.
- VSI-002 (#818, duplicate of #752, dismissed). Its premise is wrong: each paste is a bracketed
  paste (`supervisor.rs:756`), so a newline in it is text in the prompt box, not a submit (Enter
  is written separately, `supervisor.rs:1915`). Control bytes are removed as in VSI-001,
  whoever wrote the text (the prompt also folds in queued messages of other people, so the guard
  is the parsing, not who wrote it).
- VSI-003 (#692, dismissed). `send_input` is the terminal's keyboard: a viewer's keystrokes,
  arrows, Esc and Ctrl+C must reach the TUI unchanged, so sanitizing there would break the
  terminal. Its only route, `terminal_input` (`terminal.rs:176-216`), refuses a viewer without
  write access (`may_write`, `terminal.rs:183`, re-checked when access changes, SPEC-118) and a
  frame over 64 KiB (`terminal.rs:186`). Every other caller types text Yulius cleaned or wrote:
  task messages (`clean_message`, `task_message.rs:42-52`, `message_paste`
  `task_message.rs:67-71`), the opening (VSI-001), the typed `/goal` (`one_line`,
  `request.rs:356-364`, via `typed_goal_condition` `request.rs:588`), the compaction line
  (constant text and a task number, `context_full.rs` `after_compaction`), the `/compact` line
  (`TaskFocus::compact_command`, task title, goal, branch and steps through `clean`, which drops
  control characters; typed by `type_line`, `context_full.rs:539`), the outage wake
  (constant, `outage.rs:259`) and lone Enters. Pinned by `task_ownership.rs`
  `a_non_owner_watches_the_terminal_read_only`,
  `a_hand_over_refuses_the_previous_owners_next_keystroke` and `task_message.rs`
  `escape_sequences_and_control_characters_never_reach_the_session`.
- VSI-004 (#691, dismissed). There is no other user's account to reach: accounts are one pool with
  no owner (`model.rs:526-551`), and placement may put any task on any of them
  (`account_views`, `supervisor.rs:896-914`). The caller is authorized before: `switch_task`
  (`POST /api/tasks/{tid}/switch`) loads the task with `Action::Work` (`lifecycle.rs:519`), and
  only its owner may write to a task (R3). A requested account must be in the pool, of the task's
  provider, healthy and with capacity (`rotation.rs` `manual`, `rotation.rs:331-361`), else the
  switch is refused. `preflight_start` opens the credential only to check it decrypts and drops it
  (`supervisor.rs:3478-3479`): nothing is returned to the caller. Pinned by `rotation.rs`
  `manual_account_choice_rules`, `task_ownership.rs` `only_the_owner_writes_to_a_task` and
  `http_auth.rs` `invisible_sandboxes_answer_like_missing_ones_and_read_users_cannot_change_tasks`.
- VSI-005 (#693, dismissed; dead branch removed). `transcript_path`'s `stored` argument is always
  `None`: its one call (`supervisor.rs:3894`) passes `None`, so `PathBuf::from(path)`
  (`supervisor.rs:2170`) never runs. The only stored `transcript_ref` Yulius writes is its own
  `data_dir/transcripts/<task id>/<session id>.jsonl` (`session_docs.rs:983-1002`), with the
  session id refused unless safe (`is_safe_session_id`, `imports.rs:376`). The argument and the
  branch go, and the function returns the native path: no behaviour changes.

## Plan

- [x] Spec with the triage of all five findings
- [x] Narrow [[ignore]] entries for the dismissed wordings (#818 #752 #692 #691)
- [x] transcript_path loses its dead stored-path branch (#693)
- [x] Gates, changes entry and evidence
- [x] Independent review answered
- [ ] Dismissed issues commented and closed (#818 #752 #692 #691 #693)

## Acceptance criteria

- `.config/vidoc.toml` has one `[[ignore]]` entry per dismissed wording in `supervisor.rs` (four:
  the exact titles of #818, #752, #692 and #691), each with its reason; none uses `function` or
  `line`.
- `transcript_path` takes no stored path; the only product change is that removal, and the
  watcher's transcript is the same path as before.
- The tests named in VSI-001..004 pass on this branch.
- Each issue carries a comment with the reason and file:line evidence on `dev` and is closed as not
  planned (#818 also labelled `duplicate`, "Duplicate of #752"); no issue is created.

## Test plan

- `cargo nextest run -p yulius-domain -E 'test(a_typed_first_message_carries_no_control_character)
  | test(control_characters_never_reach_the_first_message) | test(manual_account_choice_rules)'`.
- `cargo nextest run -p yulius -E 'binary_id(yulius::main) & (test(/^task_ownership::/) |
  test(/^http_auth::/))'` and the `task_message` unit tests.
- The supervision tests that watch a run's transcript (`supervision::`, `run_metrics::`) still
  pass after the `transcript_path` change.
- `scripts/vidoc_scan.py`'s config parser accepts `.config/vidoc.toml`.
- `scripts/dev ci`: the full gate.

## Decisions

- All five findings are false positives on `dev`; nothing is fixed. Each guard is already pinned by
  a unit or integration test, so no new test is added (a test that cannot fail without a change
  would prove nothing new).
- #818 is a duplicate of #752: the same paste path and the same guard; #752 was filed first.
- #693's dead branch is removed rather than ignored: unreachable code that trusts a stored path
  invites a future caller to pass one, and dead code goes (AGENTS.md, dead code). This changes no
  behaviour, so it needs no new test; the transcript-watching tests cover the path.
- `send_input` keeps writing raw bytes: it is the terminal's keyboard, and its callers own the
  cleaning (VSI-003). A sanitizing `send_input` would drop the viewer's Esc, arrows and Ctrl+C.

## Evidence

- `python3 scripts/bootstrap.py` then `scripts/dev ci` at af5a37c3 (spec, ignores, the
  `transcript_path` change, changes entry): exit 0; nextest "2960 tests run: 2960 passed (3 slow),
  16 skipped". Among them every test cited above passed:
  `request::tests::a_typed_first_message_carries_no_control_character`,
  `request::tests::control_characters_never_reach_the_first_message`,
  `rotation::tests::manual_account_choice_rules`,
  `task_message::tests::escape_sequences_and_control_characters_never_reach_the_session`,
  `task_ownership::a_non_owner_watches_the_terminal_read_only`,
  `task_ownership::a_hand_over_refuses_the_previous_owners_next_keystroke`,
  `task_ownership::only_the_owner_writes_to_a_task`,
  `http_auth::invisible_sandboxes_answer_like_missing_ones_and_read_users_cannot_change_tasks`;
  the transcript-watching suites (`supervision::`, `run_metrics::`) passed with the simplified
  `transcript_path`.
- `scripts/vidoc_scan.py` `parse_config` accepts `.config/vidoc.toml` (79 entries); each of the
  four issue titles matches exactly one new entry at `crates/yulius-app/src/supervisor.rs` as
  `apply_ignores` matches (case-insensitive substring).
- `yulius security-scan`: "Security scan disabled: Vidoc is turned off in Yulius" (exit 0):
  disabled, nothing scanned, so the ignores are not proven against a live scan.
- Independent review (fresh read-only session, `om-code-review`, range f416b400..85e8dc6d):
  APPROVE, no blocker or major; it agreed all five are false positives and that removing #693's
  dead branch beats an ignore. Answered: VSI-003 now lists the `/compact` line (finding 1),
  VSI-002 no longer claims the prompt's author may type into the terminal (finding 2). Finding 3
  (pre-existing): `Refusal::ProviderMismatch` has no test and cannot happen while `Provider` has
  one variant; the ignore's reason does not rest on it.
