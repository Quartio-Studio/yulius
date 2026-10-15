---
yulius-task: 04e5e2b7-e9a3-4738-8b4c-1acbcb3a5a01
status: done
---

# SPEC-161: The Pipeline (stages) is a tab of the Dashboard, not a sidebar entry

Status: implemented
Date: 2026-10-15
Owner: Yulius task #152
Scope authorization: task #152, a screenshot of the sidebar's "Pipeline (stages)" entry with "i dont
like it here, move it somewhere else, maybe as part of dashboard".

## Goal

The sidebar no longer lists **Pipeline (stages)**. The Dashboard page gets the tab strip the CI/CD
and Secrets pages use, with **Overview** (the Dashboard as it is) and **Pipeline (stages)** (the
Pipeline page as it is, with its filters, range, export and timelines). Every address and link that
opened the Pipeline keeps working and lands on that tab; the sidebar's Dashboard is current there.

## Context

- `crates/yulius-server/web/assets/layout.js` `NAV`: Tasks, Git, CI/CD, Pipeline (stages), Secrets,
  Dashboard (SPEC-029, SPEC-137, SPEC-151); the rest is under More.
- `pages/cicd.js` (SPEC-151, ADR-0125): a page titled CI/CD whose "Section" radiogroup
  (`tl-tabs ftabs`, `tl-tab` radios, arrow keys, roving focus) mounts one section at a time with
  `mount(ctx, sandbox, route, host, actions) → teardown`; the section is kept in `?section=cd` with
  `replaceQuietly`, and the router maps the old `/cd` to it.
- `pages/dashboard.js` `mountDashboardPage`: header actions (range label, 7/30/90 days radiogroup,
  Export, Refresh), `?days=`, a live refresh every 30 s, and a **Pipeline** card (SPEC-137 STG-404)
  whose "Open Pipeline" link goes to `/s/<slug>/pipeline[?days=N]`.
- `pages/pipeline.js` `mountPipelinePage`: its own header actions (range, Export, Refresh), filters
  and `?days=&subject=&environment=&job=&trend=&open=` written with `replaceQuietly` against
  `/s/<slug>/pipeline`, a refresh every 5 minutes; `pipelineHref(sandbox, subject)` is the
  `?open=` link used by CI and CD run dialogs ("In Pipeline") and a task's overview.
- `router.js` `SANDBOX_PAGES` holds `pipeline`; `app.js` maps it to `mountPipelinePage`. No Rust
  code or agent text links to `/pipeline`.
- e2e: `pipeline.e2e.ts`, `navigation.e2e.ts`, `responsive.e2e.ts`, `quality.e2e.ts` open
  `/s/studio/pipeline`; the Dashboard is covered by `pipeline.e2e.ts` (its Pipeline card),
  `places.e2e.ts` (its range in the address), `times.e2e.ts` and `subscriptions.e2e.ts`.
- ADR-0112 places the Pipeline page "beside CI and CD"; ADR-0125 kept it its own entry because
  task #145 named CI and CD only.

## Requirements

| ID | Requirement | Verification |
| --- | --- | --- |
| DPL-001 | The sandbox navigation lists Tasks, Git, CI/CD, Secrets and Dashboard (then More); no "Pipeline (stages)" entry. | e2e `navigation.e2e.ts` |
| DPL-002 | The Dashboard page keeps its heading **Dashboard**; under it a "Section" radiogroup with **Overview** and **Pipeline (stages)**, styled and keyboard-driven like the CI/CD page's tabs. Each tab shows exactly what its page showed before, with its own header actions and sub-heading; leaving a tab stops its refreshes. | e2e `pipeline.e2e.ts` (with axe), `places.e2e.ts` |
| DPL-003 | The Overview tab's address is `/s/<slug>/dashboard` (with `?days=`), the Pipeline tab's `/s/<slug>/dashboard?section=pipeline`, followed by its own state (`days`, `subject`, `environment`, `job`, `trend`, `open`); a tab change writes it quietly and a reload keeps the tab and its state. | e2e `pipeline.e2e.ts` |
| DPL-004 | Every earlier address keeps working: `/s/<slug>/pipeline` with any query opens the Pipeline tab with that state and its address becomes the canonical one; "In Pipeline" (CI and CD runs), a task's stages link and the Overview's Pipeline card open the Pipeline tab; the sidebar's Dashboard is current on both tabs. | e2e `pipeline.e2e.ts`, `navigation.e2e.ts` |
| DPL-005 | ADR-0132 records the move (amending ADR-0112's and ADR-0125's placement); SPEC-001 UI-004, the product summary, the architecture, the web README and the e2e README name the Dashboard's Pipeline tab. | review |

## Plan

- [x] Read the product context and related work, then write and validate the spec
- [x] The sidebar drops Pipeline (stages); the Dashboard shows it as a second tab
- [x] Old /pipeline links, run and task links open the Dashboard's Pipeline tab, proven by e2e
- [x] ADR, product, architecture and UI docs describe the Dashboard's Pipeline tab
- [x] Independent review in a fresh session, findings fixed test-first
- [x] Every gate green, docs and the changes log updated

## Acceptance criteria

- The sidebar of `/s/studio/t/12` lists Tasks, Git, CI/CD, Secrets, Dashboard and More.
- `/s/studio/dashboard` shows the heading Dashboard, the Overview tab checked, the key numbers and
  the Pipeline card; choosing **Pipeline (stages)** (click or arrow key) shows the Delivery, trend
  and Stages cards with the range, Export and Refresh of the Pipeline, and the address is
  `/s/studio/dashboard?section=pipeline`; a reload keeps it.
- `/s/studio/pipeline?days=7&subject=cd_run&environment=prod` shows the Pipeline tab with those
  filters at `/s/studio/dashboard?section=pipeline&days=7&subject=cd_run&environment=prod`, and the
  sidebar's Dashboard is current.
- "In Pipeline" in a CI run's dialog opens `/s/studio/dashboard?section=pipeline&open=ci_run:…`.

## Test plan

- e2e (mock), written first and failing before the change: `navigation.e2e.ts` expects no Pipeline
  entry; `pipeline.e2e.ts` opens the Pipeline through the Dashboard's tab (click, arrow key, reload,
  address, header actions, axe), opens the old `/pipeline` addresses as aliases and expects the
  canonical addresses everywhere it wrote `/pipeline`; the Overview's Pipeline card opens the tab.
  `responsive.e2e.ts` and `quality.e2e.ts` open the canonical address.
- Review round 1: `consistency.e2e.ts` opens `/s/studio/constructor` (a name every object has) and
  expects Page not found; `pipeline.e2e.ts` counts the pages' live refreshes across tab changes and
  leaving the page (it fails when a section's teardown keeps its interval), opens the card with a
  7-day range and "In Pipeline" from a deploy's dialog.
- `scripts/dev check`, `scripts/dev lint`, `scripts/dev test`, `scripts/dev security`, the e2e suite.

## Decisions

- **A tab of the Dashboard, not a More entry** (ADR-0132): the request names the Dashboard, the
  Dashboard already carries the Pipeline's overview card, and both are read-only analytics of the
  sandbox with a date range and an export. Under More it would be hidden by default and still
  stand apart from the overview that points at it. A Dashboard section of cards (no tab) would put
  two date ranges, two exports and the Pipeline's filters on one page.
- **The CI/CD page's tab strip, shared**: `cicd.js`'s strip and section switching become
  `sectionedPage` in `pages/common.js`, used by CI/CD and the Dashboard, so the two pages behave
  the same and nothing is copied. Secrets keeps its own strip (its sections are not separate
  pages and it has no alias); folding it in is not asked.
- **`?section=pipeline` on `/dashboard`, `/pipeline` an alias**: as `/cd` is for CI/CD (ADR-0125);
  the Overview keeps the plain address the sidebar and older links use.
- **A tab change starts the other tab fresh** (its default range and filters), as CI/CD does; each
  tab's own state stays in the address while it is shown, and the place memory (SPEC-113) returns
  to the tab and state left.
- **Aliases are own names only** (review round 1): `SECTION_ALIASES` is read with `Object.hasOwn`,
  so `/s/<slug>/constructor` and the like stay Page not found.
- **The Overview's Pipeline card keeps its link** ("Open Pipeline" to the Pipeline tab with the
  same range): a link to another address of the page mounts it again, as every page's links do.

## Evidence

Run on head b44b97d3 (branch current with `origin/dev` 9883a45f), 2026-10-15:

| Command | Result |
| --- | --- |
| `npx e2e run tests/pipeline.e2e.ts tests/navigation.e2e.ts` before the change | 7 failed as expected: no Section radiogroup, heading "Pipeline", a sixth sidebar entry, `/pipeline` addresses |
| same plus `cd`, `ci`, `places`, `responsive`, `quality`, `loading-shapes`, `times`, `subscriptions` after it | pass (49 + 66 tests) |
| review round 1 tests before their fix | `consistency.e2e.ts` failed (`/s/studio/constructor` showed Sandboxes); the refresh test failed with the Pipeline teardown removed on purpose ("expected 1 to be 0") |
| `cd e2e && npx e2e run` (host load ~26) | pass: 468 passed, 1 skipped |
| `scripts/dev check` | pass |
| `scripts/dev lint` | pass |
| `scripts/dev deps` | pass: cargo-deny and cargo-shear found no issues |
| `scripts/dev test` | pass on b9accd0a: 1814 passed, 18 skipped. On b44b97d3 (no Rust changed since): 1813 passed, 1 failed, `task_done::a_switch_waits_for_a_background_agent_launched_in_the_turn` timed out at 60 s with the task Running and both runs killed (load ~26), the open #527 |
| `python3 scripts/check_repository.py` | pass (after `--sync-brand` moved `pipeline.js` in index.html's preloads) |
| `scripts/dev security` | not run (exit 2): Vidoc answers HTTP 401 "Invalid or expired token" for `VIDOC_API_KEY` (#470, ADR-0104) |

Independent review round 1 (fresh session, `om-code-review`, head b9accd0a): approved with four
minors and two nits. Fixed test-first: an inherited name (`constructor`) was taken for a page alias
and broke navigation; the refreshes the tab and the page left stop were untested; the card link
with a 7-day range and "In Pipeline" from a deploy were untested; this spec cited a test file that
does not exist. Accepted: the `/pipeline` link to the state already shown (Decisions, ADR-0132).
Round 2 reviews the head that carries this Evidence; the gates of the merged head run as Yulius CI
on the pull request.
