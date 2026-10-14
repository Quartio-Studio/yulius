---
yulius-task: 47613da5-0c55-4c3d-8d5e-eba53dc65369
status: done
---

# SPEC-090: the session view folds long injected context as the TUI hides it

Status: implemented
Date: 2026-10-14
Owner: Yulius task #75
Scope authorization: task request: "Fix #347: Session view: hide isMeta context (skill bodies,
injected notes) as Claude Code's TUI does", following the issue's acceptance criteria.

## Goal

A person reading a session on the Sessions page sees what the TUI showed: the injected context
that Claude Code records as `isMeta` (a skill's whole body, long hook context) no longer fills the
page. It takes one faint line, `⎿ Context added (6.2 KiB)`, that opens on demand. Short notes that
people act on, such as the Stop hook's goal line and its feedback, stay readable as written.

## Context

- SPEC-075 (ADR-0059) prints a session as Claude Code's TUI. The server turns every `isMeta`
  transcript text into a `meta` block (`crates/yulius-app/src/transcripts.rs` `entry_of` /
  `text_block`, cut to 16 Ki characters with no mark that it was cut). The page (`metaLines` in
  `crates/yulius-server/web/assets/pages/sessions.js`) reads Claude Code's wrappers in it and shows
  any other text as a faint `note`, in full. Issue #347 is the follow-up SPEC-075 filed.
- Claude Code's TUI does not print `isMeta` messages. Some of them still say something a person
  needs: the Stop hook's "A session-scoped Stop hook is now active with condition: …" and its
  "Stop hook feedback:" are what the TUI shows as the goal and the hook's verdict.
- Every non-wrapper `isMeta` text in this sandbox's transcripts, on 2026-10-14 (1,170 texts):

  | Note | Count | Largest | Lines |
  | --- | --- | --- | --- |
  | "Stop hook feedback: …" (goal not met, Yulius notices) | about 1,000 | 1,068 B | 4 |
  | "A session-scoped Stop hook is now active with condition: …" | 102 | 713 B | 1 |
  | "Goal check-in: «…»" | 28 | 759 B | 3 |
  | "[Image: …]", "Continue from where you left off.", short task notes | about 25 | 217 B | 1 |
  | "Base directory for this skill: …" (a skill's body) | 1 | 6,332 B | 92 |

  The notes people act on are at most 4 lines and about 1 KB; a skill body is tens of lines and
  several KB. Imported sessions may carry other hook context of either kind.
- Agents read the same transcript through `session_read`, and meta texts are not in the search
  index (ADR-0059). Neither changes here: the page alone decides how a note is printed.

## Requirements

| ID | Requirement | Verification |
| --- | --- | --- |
| MTF-001 | A meta note (text `metaLines` gives as `note`) longer than 8 lines or 2,000 characters (UTF-16 units) is folded: one faint line `⎿ Context added (<size>)`, the UTF-8 size of the text the server sent in the app's binary units, that a click or the keyboard opens to that text. | e2e `sessions.e2e.ts` (unit `foldsNote`, rendered page) |
| MTF-002 | A shorter note is shown as before, faint and as written; the Stop hook's goal line and feedback stay readable. | e2e `sessions.e2e.ts` |
| MTF-003 | An opened long note scrolls within a bounded height and is a labelled region a keyboard reaches, like a long tool result; the page has no WCAG violation. | e2e `sessions.e2e.ts` with axe |
| MTF-004 | The transcript API and `session_read` are unchanged. | No server change; `transcripts.rs` tests unchanged |

## Plan

- [x] Spec, ADR and plan for folding long meta notes
- [x] Long meta notes fold to one line in the session view, with e2e tests
- [x] Docs, changes log and gates green
- [x] Independent review and its fixes

## Acceptance criteria

- A session with a skill body shows `⎿ Context added (… KiB)` where the body was, and none of the
  body's text until the line is opened; opening it shows the body.
- The Stop hook's goal line in the same session is shown in full.
- `sessions.e2e.ts` covers a long note and a short one, and axe finds no violation with the long
  note folded and opened.
- No Rust change: `session_read` output is unchanged.

## Test plan

- Unit, in the page (as the existing `metaLines` test): `foldsNote` is false for the Stop hook's
  line, a 4-line feedback note and exactly 8 lines or 2,000 characters; true for 9 lines and for
  2,001 characters on one line.
- Rendered page: the mock's task session #4 gains a skill body as recorded (`isMeta`, "Base
  directory for this skill: …", 40 lines). The test checks the folded line and its size, that the
  body is hidden, that a keyboard opens it, that the Stop hook line stays visible, and axe in both
  states. Written first; it fails on today's page, which prints the body.
- Gates: `scripts/dev ci` and the full e2e suite.

## Decisions

- Fold, not hide (ADR-0071). Hiding would match the TUI exactly, but a person reviewing a session
  could not tell that a skill or hook shaped the next turn; one faint line costs little and keeps
  it reachable. A folded line also needs no list of known notes.
- The threshold is size, not kind: more than 8 lines or 2,000 characters. Every note people act on
  in the measured transcripts is at most 4 lines and 1,068 B, about half the bound; a skill body is
  far beyond it. Matching kinds by their text ("Base directory for this skill:") would miss
  hook context the CLI or other tools add later. A long note of a kind people act on folds too, and
  is one click away.
- The page decides, as for wrappers (ADR-0059): no server or API change, so agents' reads and the
  search index stay as they are.

## Evidence

- Fail first: with the mock's skill body and the new tests in place, `npx e2e run
  tests/sessions.e2e.ts` failed 2 of 15 (`foldsNote` undefined; no `details.ss-note`). With the
  change: 15 of 15 passed.
- The opened note's bound is proven: with `.ss-note-text` dropped from the `max-height` rule the
  rendered test fails (1 of 15), and passes again with it.
- `scripts/dev ci` (after `python3 scripts/bootstrap.py`): exit 0; nextest 1,502 tests passed,
  16 skipped; repository contracts, lint, docs passed.
- Full e2e suite, `E2E_TELEMETRY_DISABLED=1 npx e2e run`: 41 files passed, 1 skipped; 280 tests
  passed, 1 skipped; axe checks included.
- `python3 scripts/check_repository.py` on the final tree: passed.
- No Rust file changed: the transcript API and `session_read` output are as before (MTF-004).
- Seen in a headless Chromium on the mock: `⎿ Context added (1.6 KiB)` under the last result,
  the Stop hook line in full above it; opened, the body indented under it.
- Independent review (fresh session, `om-code-review`): APPROVE at d7858286 with minors, all
  fixed in 603f9f7a: the opened note's scroll, bound and focus ring are tested; the size is
  checked exactly in UTF-8 bytes (the mock has non-ASCII text); the CSS joins the neighbouring
  rules; the wording says the text the server sent. The server's silent cut of meta texts over
  16 Ki characters predates this change and is follow-up #380.
