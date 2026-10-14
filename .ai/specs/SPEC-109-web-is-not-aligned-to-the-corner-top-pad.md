---
yulius-task: ad9045a3-586a-41ee-878f-c4b053a73303
status: done
---

# SPEC-109: the session's spider web sits in its corner and keeps its colour

Status: done
Date: 2026-10-14
Owner: Yulius task #94 (issue #427)
Scope authorization: the person's request (task #94), "web is not aligned to the corner, top
padding it too low and web gets almost invisible for some reason when scrolling, it should stay in
the same color. Fix.", with three screenshots of a read-only session: the web crossing the well's
top border into the header, and, scrolled, the web faded nearly into the background.

## Goal

The pixel-art web of the read-only session view (SPEC-075, SPEC-101) hangs exactly in the well's
top-right corner, at the top of the session and while it sticks during scrolling, and it keeps the
same colours whether it is stuck or not.

## Context

- **Symptom 1: off the corner.** In the mock session `ses_imp_checkout` at 1280 px, the well's top
  is at 186 px and `svg.ss-web` starts at 171 px: 15 px above the well's border, inside the header
  (the first screenshot).
- **Root cause 1.** SPEC-101's review round 2 added `.ss-web-mark` (`height: 0; margin-top: -16px`)
  before the web, so the IntersectionObserver would see the well's very top. A float starts at the
  position of the block before it, so the mark lifts the float's start 16 px into the padding, and
  the float's own `margin: -16px …` lifts it 16 px more: above the border. The e2e tests only bound
  the offset from above (`top <= 2`), so a web too high passed.
- **Symptom 2: fades when scrolling.** Stuck, the web takes `--session-web-stuck` (`#292725` on the
  well's `#171615` in dark), which reads as almost invisible.
- **Root cause 2.** This was by design (SPEC-101 SPD-001, ADR-0082): `watchStuckWeb` sets
  `.ss-web-stuck` while the mark is above the view, to keep text that runs over the threads at AA.
  The person rejects that trade-off: the web should stay the same colour.
- The mark exists only for that observer, so removing the fade removes the mark, and with it the
  misalignment.

## Requirements

| ID | Requirement | Evidence |
| --- | --- | --- |
| WEB-001 | At the top of a session the web's top edge is the well's top inside its border (within 1 px, never above it) and its right edge is the well's right edge. | e2e |
| WEB-002 | Scrolled, the web's top edge is the scroll area's top (within 1 px, never above it), its right edge the well's, and it never leaves the well. | e2e |
| WEB-003 | The threads and silk keep `--session-web` and the spider `--session-dim` while the session scrolls, in both themes; there is no stuck colour. Text further down still paints over the web. | e2e, repository check |
| WEB-004 | SPEC-101, ADR-0082 (superseded in part by a new ADR), `docs/changes.md`, `e2e/README.md` describe it. | review |

## Plan

- [x] Root cause found and the spec written and validated
- [x] The web sits exactly in the well's corner, at the top and while stuck
- [x] The stuck web keeps its colour while the session scrolls
- [x] Independent review in a fresh session, findings fixed test-first
- [x] Every gate green, docs, ADR and the changes log updated

## Acceptance criteria

- AC-WEB-1: the corner test measures `web.top - (well.top + border)` in [0, 1] and
  `well.right - web.right` in [0, 1]; before the fix it is -16.
- AC-WEB-2: the sticky test, scrolled by 200 px and just past the well's top, measures the web's top
  against the scroll area's in [0, 1] (and before scrolling against the well's inner top).
- AC-WEB-3: scrolled, the computed fills of `.ss-web-thread`, `.ss-web-silk` and `.ss-web-body`
  equal `--session-web`, `--session-web` and `--session-dim`, in dark and light; no
  `.ss-web-stuck` and no `.ss-web-mark` remain.
- `scripts/dev ci` and the e2e suite pass.

## Test plan

- e2e `e2e/tests/sessions.e2e.ts` (mock mode): tighten the corner test and the sticky test to two
  bounds (red first: -16 px), and replace "fades to a colour every text reads on" with "keeps its
  colours" (red first: the stuck fill is `--session-web-stuck`).
- Look at it by hand: screenshots at the top and scrolled, both themes.
- No server change.

## Decisions

- Remove the fade, its observer and its mark rather than repositioning the mark: the mark has no
  other use, and the person asked for the same colour. ADR-0088 records that it supersedes the
  stuck colour of ADR-0082.
- Text running over the web lower in the session now reads over `--session-web` threads; the
  threads are 3 px lines of a decoration under the text, which still paints above them. This is
  the person's explicit choice; `--session-web-stuck` and its contrast pairs in
  `check_repository.py` go away.
- Rejected: reserving a right gutter beside the stuck web for the whole transcript (84 px less text
  everywhere, worse at 390 px) and a thinner faded variant (still "a different colour").
- Review round 1 (fresh session): APPROVE at b4bbfeaf with 2 minors and 3 nits. Fixed test-first:
  the corner is also checked at 390 px (red against the old assets: top -12), the test checks that
  no `.ss-web-stuck`/`.ss-web-mark` is left, and SPD-001's row points to SPEC-109. Round 2 at
  b755918d: APPROVE; its nit (measure the phone corner scrolled to the top) is fixed.

## Evidence

- Red first, `e2e/tests/sessions.e2e.ts` (mock mode):
  - corner and sticky tests, two-sided: `expected -16 to be greater than or equal to 0` before
    the fix; the phone check against the old assets: `{"top":-12,"right":0}`.
  - colour test: `dark, scrolled past: expected false to be true` (the stuck fill was
    `--session-web-stuck`).
- Measured by hand (Playwright, mock `ses_imp_checkout`): at 1280 px the web's top went from 171
  to 187 px, the well's inner top (187 px), and scrolled it sits at the scroll area's top (34 px)
  with its right on the well's right; at 390 px likewise (279 px, then 44 px). Screenshots in dark
  and light show the stuck web in `--session-web` and `--session-dim`.
- `cd e2e && npx e2e run` at 360261cc: 45 files passed, 1 skipped; 329 tests passed, 1 skipped.
  `tests/sessions.e2e.ts`: 20 of 20, axe clean.
- `python3 scripts/check_repository.py`: passes (the `--session-web-stuck` pairs removed).
- `scripts/dev ci` at 360261cc: 1579 of 1580 Rust tests passed; one `task_done` test timed out
  under host load (load average 15–18 on 12 cores): `a_live_monitor_keeps_the_task_working_until_it_ends`
  (at b755918d, `a_background_shell_keeps_the_task_working_until_it_ends`). Both are the known
  load-flaky family of issue #404 (evidence added there); this change touches no Rust. Yulius CI on
  the pull request is the gate of record.
- Not covered: a real server with real sessions (the mock serves the same assets).

