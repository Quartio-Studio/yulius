---
yulius-task: 8a13aa7f-873c-4084-8847-278adfb05f62
status: done
---

# SPEC-419: The CD page's kept releases fit their card and offer one action each

Status: done
Date: 2026-11-04
Owner: Yulius task #401
Scope authorization: task #401, "UI is overwhleming and broken in CD section", with a screenshot of
the Dev card of a two-part environment (engine, infra) whose releases are long content ids.

## Goal

Long release ids no longer break the CD page's environment cards, and the rollback section is
calm to read. Each kept release shows one action, so the same rollback is not offered twice.

## Context

- SPEC-415 (task #397) added the rollback section to every CD environment card
  (`crates/yulius-server/web/assets/pages/cd.js`, `rollbackSection`). It shows each part, then a
  "Roll back to previous (<release id>)" button, then each kept release with its full id in
  monospace and a "Roll back to this" button on every release except the current one.
- Symptom (screenshot, quartio-v2): the release ids are 77-character ids
  (`0e36f03a3bba-67f8…adfb1c`). The "Roll back to previous (…)" button holds the whole id. A
  `.btn` does not wrap and is not shrinkable, so the button runs past the Dev card and under the
  Production card next to it. Each release row prints the full id on a line of its own. With two
  releases, the previous one gets a "Roll back to previous" button and a "Roll back to this"
  button, which both do the same thing.
- Root cause: the section was built and tested only with short ids (`v41`, the mock's). Nothing
  bounds the width of an id in a button or a row, and the one-click default was added as a second
  control instead of as the label of the previous release's own button. The e2e test
  `the CD page rolls back to an earlier release in one click` uses `v40`-`v42`, so it could not
  see the overflow.
- Task #400 (in parallel) changes rollback configuration and `cd_status` wording, not this
  rendering.

## Requirements

- R1. No part of an environment card overflows it, whatever the length of a release id, at
  desktop and phone widths. No button text holds a release id longer than 16 characters: a longer
  id is shortened to its first 12 characters and "…". The full id is still in the row's tooltip,
  in its accessible name and in the confirmation message.
- R2. Each kept release row shows its id on one line (cut with an ellipsis when it is too long),
  its commit, time and who below it, and at most one action on the right:
  - the current release: "Deployed now";
  - the previous release (RBK-013): the one-click default, "Roll back to previous";
  - a blocked release: why it is blocked;
  - any other release: "Roll back to this".
  There is no separate "Roll back to previous" button above the list.
- R3. A part shows its 3 newest kept releases. Older ones sit behind a "N older releases"
  disclosure, closed by default.
- R4. The confirmation dialog's confirm button uses the shortened id. Toasts use the shortened id
  too. The message still names the full id.
- Unchanged: who may roll back, what the server runs, barriers and the drill (SPEC-415).

## Plan

- [x] Long release ids keep every CD card within its bounds, with one action per release (e2e first)
- [x] Older kept releases fold behind a disclosure (e2e first)
- [x] Docs, changes entry and independent review answered
- [x] Gates green and delivered to dev

## Acceptance criteria

- With 77-character release ids in two parts, at 1440 px and 390 px wide, every descendant of
  each `.cd-env` card lies within the card's box, and the page does not scroll sideways.
- The previous release's row holds the only "Roll back to previous" button. Its accessible name
  is `Roll back to previous (<id>) of <where>`. The current row holds no button.
- A part with 5 kept releases shows 3 rows. "2 older releases" opens the other 2.
- The existing rollback e2e flow (one click, barrier, production typed slug) still passes, with
  its button names updated to the new layout.

## Test plan

- e2e `cd-releases.e2e.ts` (new): a test gives the mock's dev environment two parts with long ids and checks
  the bounds of every element (desktop and phone), the one action per row and the axe rules. It
  fails before the fix because the button ends past the card.
- e2e `cd-releases.e2e.ts`: the fold of older releases (5 kept releases: 3 shown, disclosure opens 2 more; walked back, the release deployed now and the previous one stay shown).
- `cd.e2e.ts`: the existing SPEC-415 rollback test, adjusted where the header button became a row button, now also rolls forward with "Roll back to this".

## Decisions

- One action per row instead of a header button plus row buttons. The previous release's own
  button reads "Roll back to previous". This keeps RBK-040's one-click default without offering
  the same action twice. It is a small UI choice inside SPEC-415, so it needs no ADR.
- Shorten ids in text (12 characters and "…") instead of relying only on CSS ellipsis inside
  buttons. A button label must stay readable, and the accessible name keeps the full id
  (WCAG 2.5.3: the name starts with the visible text).
- Fold after 3 rows. Rollbacks almost always go to the previous release, and up to 20 releases
  are kept per part, so the full list stays one click away.

## Evidence

- Reproduced: `e2e/tests/cd-releases.e2e.ts` failed at 4baf4be2, before the fix. Its bounds check
  found `button Roll back to previous (0731fe929a36-ca82…` outside both parts' cards. The fold
  test failed at its own commit, before the fold existed.
- `cd e2e && E2E_TELEMETRY_DISABLED=1 npx e2e run tests/cd.e2e.ts tests/cd-releases.e2e.ts`:
  27 passed at the final head, with axe finding no violations, including with the fold open.
- `scripts/dev ci` at fbb2a7f9 (round-1 answers): exit 0, nextest 3099 run, 3099 passed, 16
  skipped. The only later change is a line moved within `e2e/tests/cd.e2e.ts` (review round 2).
- `yulius security-scan`: "Security scan disabled" (Vidoc is off in Yulius). Disabled, not passed.
- Visual check: a Playwright screenshot of the Dev card at 1440 px and 390 px, with two parts
  holding 77-character ids. Each release fits on one line with one action, and the older ones
  are folded.
- Independent review in a fresh session (`om-code-review`):
  - Round 1 approved, with minor findings. All were answered: roll-forward coverage, the toast's
    double ellipsis, part names on the folds, and the spec and docs wording.
  - Round 2 asked to move the mock's `cdRunMs = 0` line before the v41 rollback, so the
    roll-forward step no longer waits for the 3 s poll. Done.
  - The pre-existing "Roll back to this" label on newer releases is filed as #1061.
- Not tested here: the full e2e suite (Yulius CI runs it as the `e2e` job) and a live
  quartio-v2 page. The ids come from its screenshot, replayed in the mock.
