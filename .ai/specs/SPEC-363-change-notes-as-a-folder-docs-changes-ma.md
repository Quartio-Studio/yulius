---
yulius-task: ba278958-27a5-41e2-b1fc-57b35ee30eed
status: done
---

# SPEC-363: Change notes as a folder, one file per pull request

Status: implemented
Date: 2026-10-31
Owner: Yulius task #346 (from the owner's quartio-v2 sandbox)
Scope authorization: task #346. Builds on SPEC-116 CHG-001 (the changes excerpt), SPEC-083
MRG-001 (union merges of the shared logs) and SPEC-001 §7.2 (the documentation format).
Decision: [ADR-0248](../../docs/decisions/0248-change-notes-may-be-a-folder-of-one-file-per-change.md).

## Goal

A project whose `[docs] changes` names a folder (one change note per pull request) gets a
briefing that says so: add one new file there, never edit other tasks' files, and its Recent
changes quote the newest notes. Projects with one changes file see no difference.

## Context

- **Symptom (2026-10-31, quartio-v2).** Many tasks merge into `dev` in parallel and every one
  added an entry at the top of `docs/changes.md`. Each merge made every other open pull request
  conflict on GitHub (its server-side merge ignores the union driver of MRG-001), and each paid a
  full CI rerun of up to an hour. quartio-v2 moved to one file per pull request in
  `docs/changes/` (its PR #488, AGENTS.md updated). Yulius' briefing still said "add a short entry
  at the top of `docs/changes.md`" and quoted that file, so tasks got two contradicting rules.
- **Where the path is used.** `DocsLayout::changes` (a `String`, parsed in
  `spec_format/config.rs`, which strips one trailing `/` today, so `"docs/changes/"` read as the
  file `docs/changes`) is read by:
  the briefing's discipline rule 6, "Read before you build" and "Recent changes"
  (`yulius-domain/src/briefing.rs`), filled by `RepoDocs::read` in `yulius-app/src/hooks.rs`
  through `changes_excerpt`; the documentation check `validate_project` (missing changes file)
  through `ProjectFiles::from_files` (scaffolds) and `onboarding::validate_checkout`
  (checkouts); and `integration_merges::union_lines`, which keeps `merge=union` for the log.
  Scaffold templates always write `docs/changes.md` (single-file default), unchanged.
- **Decisions already have both shapes** (`DecisionsLocation::{Directory, File}`), chosen by the
  path's `.md` ending. Changes keep a `String`: the folder is marked by a trailing `/`.

## Requirements

- **CHN-001 Folder mode.** `[docs] changes` names a folder when it ends with `/`
  (`"docs/changes/"`), or when the path is an existing directory of the checkout. The layout then
  holds the path with one trailing `/` (`DocsLayout::changes_folder`). Any other path is the single
  file, as today.
- **CHN-002 The rule.** In folder mode, discipline rule 6 says: add one new file to the folder for
  your change, named `YYYY-MM-DD-<task number>-<short-slug>.md` with this task's number filled in,
  unless the repository's AGENTS.md or the newest files there use another naming (then follow
  it), and never edit other tasks' files. "Read before you build" lists the folder as one file per
  delivered change.
- **CHN-003 Recent changes.** In folder mode, Recent changes quotes the newest notes, newest first,
  each under its file name: the folder's `.md` files except `README.md` and `index.md`, ordered by
  the first ISO date in the name, then by the name, newest first; at most 12 notes, each cut to
  1,200 bytes, and the whole within the 4,000 bytes of the single-file excerpt, under the same
  budget rules. A folder with no notes reads as no delivered changes yet.
- **CHN-004 Other readers.** The union merge of MRG-001 skips a changes folder (each task adds its
  own file, nothing to union). The documentation check counts an existing folder as present and
  reports a missing one as missing; a scaffold listing has the folder when it has a file in it.
  A folder is read only as a real directory inside the repository (no links out, as for
  decisions, #70).
- **CHN-005 Single file unchanged.** A project with a changes file gets the same briefing, union
  line and documentation check as before.

## Plan

- [x] Spec and ADR-0248 for the changes folder
- [x] `[docs] changes` parses a folder path with a trailing slash
- [x] Briefing gives the folder rule and quotes the newest notes
- [x] Checkouts resolve an existing changes directory and read its newest notes
- [x] Union merges and the documentation check handle the folder
- [x] Product summary, config reference and changes updated

## Acceptance criteria

- A briefing for a project with `changes = "docs/changes/"` contains the folder rule with the
  task's number and no "entry at the top"; its Recent changes quote the newest notes first, the
  `README.md` left out.
- `changes = "docs/changes"` naming an existing directory gives the same.
- The single-file briefing text is byte-for-byte the one before (existing tests unchanged).
- `union_lines` gives no line for a changes folder; `validate_checkout` reports no problem for an
  existing folder and "is missing" for an absent one.

## Test plan

- Unit (`spec_format::config`): a trailing `/` parses and renders back; `"docs/changes//"` is
  refused.
- Unit (`briefing`): note ordering (dated names, undated names, README left out); the excerpt's
  bounds; the folder rule and Recent changes in a built briefing; the single-file rule unchanged.
- Unit (`hooks`): `RepoDocs::read` on a checkout with a folder (slash and no slash), a folder
  linked out of the repository read as nothing.
- Unit (`integration_merges`, `onboarding`, `project`): no union line for a folder; an existing
  folder passes the documentation check, a missing one is reported, a scaffold listing with a note
  passes.

## Decisions

- Folder mode is marked by a trailing `/` in the layout, not a new enum: every reader that needs a
  file keeps working (reading a folder as a file fails cleanly), and only the readers that care
  ask `changes_folder()`. An existing directory without the slash is resolved once where the
  checkout is known (`DocsLayout::resolve_changes_folder`). See ADR-0248 for the alternatives.
- The naming rule fills in the task's number and leaves the date and slug to the agent, since the
  delivery date is not known at the briefing.
- Notes of the same day sort by name, so `…-99-x.md` comes before `…-488-y.md` (review nit 5):
  kept as specified ("then by the name"); a numeric task key would assume one naming.
- The union-line check reads `symlink_metadata` of the joined path (review nit 7): it only
  decides whether a union line is kept, reads nothing, so a link in a parent is harmless there.

## Evidence

- `scripts/dev ci` at 40287d1f: exit 0; nextest 2917 tests run, 2917 passed, 16 skipped; fmt,
  clippy, deps, macOS check and `cargo doc -D warnings` passed. The first run (619437aa) failed
  only at `cargo doc` (a public doc linked the private `MAX_CHANGE_NOTE_BYTES`), fixed.
- New tests, each seen failing without its change (mutation runs): config
  `a_changes_path_with_a_trailing_slash_names_a_folder`; briefing
  `a_changes_folder_gets_the_one_file_per_change_rule`; hooks
  `repo_docs_read_a_changes_folder_as_its_newest_notes` (failed before RepoDocs read folders);
  project `a_changes_folder_is_present_with_a_note`; integration merges
  `a_changes_folder_has_no_union_line` (did not compile before `checkout_layout`). Also
  `change_notes_are_ordered_newest_first`, `the_change_notes_excerpt_is_bounded`,
  `an_existing_changes_directory_names_a_folder`, `checkout_validation_accepts_a_changes_folder`
  and end to end `agent_hooks::a_changes_folder_briefs_one_new_file_per_change` (SessionStart
  briefing for `changes = "docs/changes/"`: folder rule with the task number, newest note first,
  README left out). Every existing single-file briefing test passes unchanged (CHN-005).
- Independent review (fresh session, diff `origin/dev...619437aa`): APPROVE; low findings
  answered: spec closed out (1), Context sentence corrected (2), the 10,000-entry listing bound
  documented (3), the ordering assertion made strict (4), a comment on the presence-only
  `Some("")` (6); nits 5 and 7 kept, see Decisions.
- `yulius security-scan`: exit 2, not run — Vidoc is turned off in Yulius for this project or
  the instance; nothing was sent. Not a pass.
- Yulius CI on PR #993 at 0ed8ecea failed `xtask::cli security_is_listed_and_needs_a_vidoc_key`
  (also on PR #992): CI jobs now get `YULIUS_VIDOC=off`, and the test only removed the key, so the
  scan exited 0 as disabled. Reproduced with `YULIUS_VIDOC=off cargo test -p xtask --test cli`
  (failed), fixed by removing `YULIUS_VIDOC` from the child's environment (passes with and
  without the variable); issue #994. PR #992 merged the same fix into `dev` meanwhile, so the
  merge of `dev` keeps its version and this change no longer touches the test.
- A later full gate (4cc89884, `YULIUS_VIDOC=off`) failed once at
  `local_ci::running_ci_jobs_weigh_on_their_sandbox_until_they_end` (git fetch "bad object
  worktrees/build@1/HEAD"): code this change does not touch (SPEC-362); 8 of 8 isolated reruns
  passed; filed as flaky, issue #995.
- Not tested: a real quartio-v2 checkout (no access from this sandbox); the behaviour is covered
  by the fixture folders above.
