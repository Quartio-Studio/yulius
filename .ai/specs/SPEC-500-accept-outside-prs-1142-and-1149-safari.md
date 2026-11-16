---
yulius-task: d68a94dd-e1e4-4cb7-a8b9-aedb8bfd7dd9
status: in-progress
---

# SPEC-500: A dialog shows its whole body in Safari; the README links yulius.dev

Status: in-progress
Date: 2026-11-16
Owner: Yulius task #482, bringing two accepted outside pull requests onto current `dev`:
#1142 by studenciak00owl (its spec, SPEC-448, is this spec's first part, renumbered because the
task's spec carries the number Yulius gave it) and #1149 by fajfer (Damian Fajfer).
Scope authorization: MrFishPL accepted both pull requests (task #482's prompt, 2026-11-16). The
contributor's own report for #1142, verbatim (2026-11-10): "Popraw te okienko, bo defaultowo zbyt
krótkie się wyświetla i nie widać reszty opcji" ("fix this dialog: by default it shows too short
and the rest of the options cannot be seen"), seen on Add subscription in Safari.

Each part is its own pull request into `dev`: first the dialog fix (#1142), then the README
links (#1149).

## Goal

1. Every dialog opened with `openModal` shows its whole body in Safari, as it does in Chrome, and
   scrolls its body only when the window is too short for it (#1142).
2. The README sends a reader to the Yulius website at https://www.yulius.dev/, not to
   `juliusz.dev` (#1149).

## Context

- `.modal` is a column flex container whose height is `auto` with `max-height: calc(100vh - 48px)`;
  `.modal-body` had `flex: 1; min-height: 0`, so its flex basis is `0%`. On `dev` at `c7348d044`
  (2026-11-16) that rule is unchanged since the contributor's base `91a0cec5`.
- Chrome resolves that `0%` basis like `content` because the dialog's height is indefinite. WebKit
  resolves it to `0`, and `min-height: 0` lets the body stay there: the body is 32 px tall (its
  padding) and everything else scrolls inside it.
- The contributor measured it in WebKit 26.0 (Playwright 1.56.1) on the mock (`?mock=1`),
  1440 × 900, body height shown / content height, on `dev` `91a0cec5`:

  | Dialog | WebKit before | WebKit after | Chrome before and after |
  |---|---|---|---|
  | Add a Claude subscription | 32 / 150 | 150 / 150 | 150 / 150 |
  | New playground | 32 / 653 | 653 / 653 | 653 / 653 |
  | Import repository | 32 / 418 | 418 / 418 | 418 / 418 |

  This task measures again on current `dev` (Evidence).
- The full-height sign-in terminal dialog (`modal-full`) has a definite height; it is unchanged
  by the fix.
- Found while bringing it onto `dev`: the task search (`spotlight.js`, the Tasks page's "Search
  tasks" and `/`) is a second auto-height dialog with the same rule, `.spot-list { flex: 1;
  min-height: 0 }` inside `.spot` (`max-height`, no height). It existed at the contributor's base
  too, but it is not an `openModal` dialog, so #1142 did not reach it. In WebKit 26.0 its results
  list is 16 px tall (its padding) whatever it holds. A scan of `styles.css` for column flex
  containers with a `max-height` and no height (`.menu`, `.modal`, `.agent-requests`, `.lab`,
  `.spot`, `.needs-you-parts`, `.upload-errors`) finds no other child with a `0%` basis.
- The collapse is WebKit 26.0's (Playwright 1.56.1, Safari 26.0). The WebKit 26.6 of Playwright
  1.63.0, which this repository's e2e uses, already shows both bodies whole with the old rule.
  Only these two WebKit versions were measured, nothing older than 26.0; the fix gives the same
  layout in both and in Chromium.
- The README links `juliusz.dev` on line 12 (the "Just found Yulius" note) and names it in the
  "From curious to first task" diagram on line 47; no other file of the repository names
  `juliusz.dev` (`grep -rn juliusz.dev`, 2026-11-16). #1149 changed line 12 only.

## Requirements

- DLG-001. `.modal-body` takes its content's height as its flex basis (`flex: 1 1 auto`), so a
  dialog is as tall as its content up to its `max-height` in every engine.
- DLG-002. A body taller than the space left still shrinks (`min-height: 0`) and scrolls, and a
  `modal-full` dialog's body still fills its fixed height.
- DLG-003. The task search's results list takes its content's height as its flex basis
  (`flex: 1 1 auto`), so the search dialog shows its results up to its `max-height` and scrolls
  the rest, in every engine.
- DOC-001. The README links and names the Yulius website as `yulius.dev`
  (https://www.yulius.dev/) wherever it named `juliusz.dev`: the note on line 12 and the diagram.

## Plan

- [x] Safari fix: a dialog's body takes its content height (contributor's commit on current dev)
- [x] Task spec SPEC-500 from the contributor's SPEC-448, changes entry
- [x] e2e guard passes with the fix and fails without it
- [x] The task search shows its results in Safari (same rule, found on current dev)
- [x] Measured in real WebKit and Chromium before and after
- [x] Independent review of the Safari fix
- [ ] README links yulius.dev on line 12 and in the diagram (contributor's commit plus line 47)
- [ ] Independent review of the README links

## Acceptance criteria

- In WebKit, Add subscription, Add sandbox and Import repository show their whole body; at a
  short window the body scrolls instead of overflowing.
- In Chromium nothing changes (same measurements before and after).
- The `modal-full` sign-in dialog keeps its height and its terminal's.
- In WebKit 26.0 the task search shows its results (up to its `max-height`), as in Chromium.
- The e2e guard passes: the Add subscription dialog's body has flex basis `auto` and shows all of
  its content; it fails with `dev`'s rule.
- `grep -rn juliusz.dev` finds nothing in the repository; both README places read `yulius.dev`
  and the line-12 link goes to https://www.yulius.dev/.

## Test plan

- e2e (`subscriptions.e2e.ts`): "a dialog shows its whole body, also in Safari". The suite runs
  in Chromium only, where the old rule did not clip, so the guard checks the computed flex basis
  (the rule WebKit needs) besides the shown height. Run with the fix (passes) and with `dev`'s
  rule (fails). The same for the task search (`task-graph.e2e.ts`, "the task search shows its
  results, also in Safari"): it waits for the tasks, then checks basis `auto` and that the first
  task row lies wholly inside the list's box. The dialog guard also checks that the body still
  shrinks (`flex-shrink: 1`, DLG-002). Each guard is run 40 times in a row.
- Real engines: a Playwright script (kept out of the repository) opens each dialog and the task
  search on the mock in WebKit 26.0 (the `playwright:v1.56.1-noble` image's server) and Chromium with `dev`'s stylesheet and with the branch's, at 1440 × 900 and at a short
  window, and prints shown / content heights.
- README: `grep`, and the rendered diagram's text read back.

## Decisions

- The contributor's commit is cherry-picked as it is (clean on current `dev`), so they stay its
  author; this task's own commits carry `Co-authored-by` lines for them.
- Their SPEC-448 becomes this task's SPEC-500 (Yulius ties a task to the spec it numbered), and
  its changes entry moves to today's date at the top of `docs/changes.md`.
- The task search gets the same one-line fix and its own guard in the same pull request: same
  cause, same engine, same outcome the contributor asked for ("a dialog shows its whole body"),
  found by measuring every auto-height dialog in WebKit 26.0 rather than only `openModal`'s.
- Two pull requests, one per contribution, as the acceptance asked; the dialog fix goes first
  because it carries the spec.

## Evidence

Part 1, the dialog fix (#1142), on `dev` `c7348d044` with the branch's commits (the contributor's
`1283b202f` kept as theirs):

- Real engines, the script `target/evidence/measure-dialogs.mjs` (not committed): WebKit 26.0
  from the `mcr.microsoft.com/playwright:v1.56.1-noble` image's `run-server`, WebKit 26.6 and
  Chromium 153 from the e2e suite's playwright-core 1.63.0, on the mock, body shown / content
  height in px, `dev`'s stylesheet (before) and the branch's (after):

  | Dialog, 1440 × 900 | WebKit 26.0 before | WebKit 26.0 after | Chromium 153 and WebKit 26.6, before and after |
  |---|---|---|---|
  | Add a Claude subscription | 32 / 150 | 150 / 150 | 150 / 150 |
  | Add sandbox (playground) | 32 / 653 | 653 / 653 | 653 / 653 |
  | Import repository | 32 / 418 | 418 / 418 | 418 / 418 |
  | Search tasks, results list | 16 / 705 | 466 / 705 (dialog at its 560 px limit) | 466 / 705 |
  | Sign in (`modal-full`): dialog, body, terminal | 852, 744, 664 | 852, 744, 664 | 852, 744, 664 |

  At 1440 × 380 in WebKit 26.0: Add sandbox and Import repository 32 → 224 shown (the body
  scrolls, dialog 332 = the window less 48 px), the search 16 → 190 (dialog 284 = its limit),
  Add subscription 32 → 150; Chromium is 224, 224, 190 and 150 before and after; the sign-in dialog
  is 332 tall with a 320 px terminal in both engines, before and after.
- e2e, the two guards (`npx e2e run tests/<file>.e2e.ts --grep 'also in Safari' --repeat-each
  40`): 40 of 40 passed for each, at a host load average of about 28. The search guard fails with
  `dev`'s `.spot-list { flex: 1 }` (`expected "0%" to be "auto"`); the dialog guard fails with
  `dev`'s `.modal-body { flex: 1 }` (`expected "0%" to be "auto"`) and with `flex: none`
  (`expected "0" to be "1"`).
- Review round 1 (Fable 5.2) found the first version of the search guard flaky (36 of 120 runs
  failed): it typed before the tasks loaded, and it compared the row's offset from the dialog
  with the list's height. Both are fixed as above. It also found that a search typed in while the
  tasks load stays empty until the next key, in code this change does not touch. Reproduced
  (`target/evidence/search-race.mjs`: 0 task rows 2 s after the tasks arrive, 12 after one more
  key) and filed as #1363.
- Review round 2 (Fable 5.2) approved `3208b5630`. Both guards passed 40 of 40 again, with
  default workers and with `--workers 1`. In WebKit 26.0 the new geometry check fails with
  `dev`'s stylesheet (the first row's bottom is 238 against the list's 200) and passes with the
  branch's.
- Full e2e suite (`npx e2e run`), before that test fix: 1064 passed, 1 skipped (87 files, 5 min).
- `python3 scripts/check_repository.py`: "Repository contracts passed".
- `yulius security-scan`: exit 2, not run: Vidoc refused the instance's key (HTTP 401). Only CSS
  and e2e tests changed.

## Known limits

- The e2e suite has no WebKit target, so a later change back to a `0%` basis is caught by the
  computed-style check, not by a Safari run.
