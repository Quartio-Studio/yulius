---
yulius-task: 7218092d-42bf-424c-b806-04c7ebb88046
status: done
---

# SPEC-151: CI and CD are one CI/CD page with two tabs, like Secrets' Team and Agent

Status: implemented
Date: 2026-10-15
Owner: Yulius task #145
Scope authorization: task #145, "make CI/CD one tab with a tab like in secrets team/agent".

## Goal

The sidebar shows one **CI/CD** entry instead of "CI (tests)" and "CD (deployment)". It opens one
page titled CI/CD whose section is chosen with the same tab strip the Secrets page uses for Team
secrets and Agent secrets: **CI (tests)** and **CD (deployment)**. Each tab shows exactly what its
page showed before, with its header action. Every address that worked keeps working.

## Context

- `crates/yulius-server/web/assets/layout.js` `NAV`: `ci` "CI (tests)" and `cd` "CD (deployment)"
  are two primary entries (SPEC-029, SPEC-134), next to Pipeline (stages) (SPEC-137).
- `pages/ci.js` `mountCiPage` and `pages/cd.js` `mountCdPage` each build a `pageFrame` (titles "CI"
  and "CD", one header action each: "Check for commits", "Set up CD with an agent"), poll their
  overview, and open a run dialog from `?run=<id>` that writes `/ci?run=` or `/cd?run=` with
  `replaceQuietly`.
- `pages/secrets.js` `mountSecretsPage`: the pattern to reuse. A `tl-tabs ftabs` radiogroup
  ("Section") of `tl-tab` radio buttons with an icon, arrow keys move between them, the choice is
  kept in `?section=agents` with `replaceQuietly`, and the shown section fills the body and the
  header's actions.
- `router.js` `matchRoute`: `/s/<slug>/<page>` for every name in `SANDBOX_PAGES`; `app.js` keys a
  page view `<page>:<sandbox>:<query>`, so a quiet address change within one page keeps the view,
  and a link to the plain address mounts it again (SPEC-113).
- Links from outside the UI: the `yulius/ci` commit status targets `/s/<slug>/ci?run=<id>`
  (`yulius-app/src/ci/mod.rs`); the agents' CD text names `/s/<slug>/cd`
  (`yulius-app/src/ci/cd.rs`); the Pipeline page and e2e use `/cd?run=<id>`.
- e2e: `ci.e2e.ts`, `cd.e2e.ts`, `navigation.e2e.ts` (the sidebar's entries), `pipeline.e2e.ts`,
  `responsive.e2e.ts`, `quality.e2e.ts`, `loading-shapes.e2e.ts`, `places.e2e.ts`.

## Requirements

| ID | Requirement | Verification |
| --- | --- | --- |
| CCD-001 | The sandbox navigation has one entry, **CI/CD**, between Git and Pipeline (stages), in place of "CI (tests)" and "CD (deployment)"; it links to `/s/<slug>/ci` and is the current page on either tab. | e2e `navigation.e2e.ts` |
| CCD-002 | The page's heading is **CI/CD**; under it a "Section" radiogroup with **CI (tests)** and **CD (deployment)**, styled and keyboard-driven like the Secrets page's tabs (click, arrow keys, roving focus). The shown section's content and header action are exactly the former page's. | e2e `ci.e2e.ts`, `cd.e2e.ts` (with axe) |
| CCD-003 | The CI tab's address is `/s/<slug>/ci`, the CD tab's `/s/<slug>/ci?section=cd`, written quietly on a tab change; a reload keeps the tab, and leaving a tab stops its refreshes and closes its run dialog. | e2e `cd.e2e.ts` |
| CCD-004 | Every earlier address keeps working: `/s/<slug>/cd` (and `/cd?run=<id>`) opens the CD tab (with that run), and its address becomes the canonical one; `/s/<slug>/ci?run=<id>` opens the CI run as before; a CD run's dialog writes `?section=cd&run=<id>` and closing it goes back to `?section=cd`. | e2e `cd.e2e.ts`, `pipeline.e2e.ts`, `ci.e2e.ts` |
| CCD-005 | ADR-0125 records the merge (amending the separate entries of SPEC-029/ADR-0025 and SPEC-134); SPEC-001 UI-004, the product summary, the architecture and the web README name the CI/CD page. | review |

## Plan

- [x] Read the product context and related work, then write and validate the spec
- [x] One CI/CD sidebar entry opens a page with CI (tests) and CD (deployment) tabs
- [x] Old /cd links and run deep links open the right tab, proven by e2e
- [x] ADR, product, architecture and UI docs describe the CI/CD page
- [x] Independent review in a fresh session, findings fixed test-first
- [x] Every gate green, docs and the changes log updated
- [x] Review round 2 approves the final head

## Acceptance criteria

- The sidebar of `/s/studio/t/12` lists Tasks, Git, CI/CD, Pipeline (stages), Secrets, Dashboard
  and More.
- `/s/studio/ci` shows the heading CI/CD, the CI (tests) tab checked, the CI configuration, runs
  and repository hygiene, and "Check for commits" in the header.
- Clicking CD (deployment) (or pressing an arrow key on the tab) shows the Dev and Production
  environments and "Set up CD with an agent", the address is `/s/studio/ci?section=cd`, and a
  reload keeps it.
- `/s/studio/cd?run=cd_s2` opens the CD tab with the deploy dialog; the sidebar's CI/CD is current.

## Test plan

- e2e (mock), written first and failing before the change: `navigation.e2e.ts` expects the one
  CI/CD entry; `ci.e2e.ts` expects the CI/CD heading and the checked CI tab; `cd.e2e.ts` gets a
  tab test (click, arrow key, reload, address, header action, axe) and opens `/cd` and
  `/cd?run=` as the old links; the existing CD tests expect the CI/CD heading and the canonical
  run address. Every other test that names the CI or CD headings follows.
- Review round 1: `cd.e2e.ts` "a tab left behind draws nothing when its answers come late" holds
  the CD overview (`hold=`) and a production deploy (`__yuliusMock.holdCdDeploy`), switches to CI,
  then releases them; `tests/cd.rs` asserts the agents' CD text names `/ci?section=cd`.
- `scripts/dev check`, `scripts/dev lint`, `scripts/dev test`, `scripts/dev security`, the e2e suite.

## Decisions

- **One page, sections in the query** (ADR-0125), as Secrets does (`?section=agents`): the tab
  changes in place with no remount, the place memory (SPEC-113) returns to the tab left, and the
  CI tab keeps the address the `yulius/ci` commit status links to. Alternatives: keep two routes
  that each draw the tab strip and navigate between them (a remount per tab and focus lost), or a
  new `/cicd` path with `/ci` and `/cd` as aliases (every link and test changes for nothing).
- **`/cd` stays an alias, not a 404**: agents' messages and older links name it; the router maps it
  to the CI/CD page with `section=cd` and the page writes the canonical address.
- **Pipeline (stages) stays its own entry**: the request names CI and CD only, and the Pipeline
  spans tasks too (SPEC-137).
- **A link to `/cd` while the CD tab is shown keeps that address** (review round 1, nit): both
  addresses are the same view, so the page is not mounted again and does not rewrite it; the tab
  shown is right and a reload canonicalises it. Not worth an `update` hook in `app.js`.
- **Sections are mounted as parts of one page**: `ci.js` and `cd.js` export a section mount that
  takes the body and the header's actions and returns its teardown, like Secrets' `mountTeam` and
  `mountAgents`; the page module owns the tab strip.

## Evidence

Run on head ab82e7e1 (after merging `origin/dev` 070abcdd), 2026-10-15:

| Command | Result |
| --- | --- |
| `npx e2e run tests/cd.e2e.ts tests/ci.e2e.ts tests/navigation.e2e.ts` before the change | 6 failed as expected: heading "CI"/"CD", no Section radiogroup, two sidebar entries, `/cd` not canonical |
| same files plus `pipeline`, `places`, `secrets` after the change | 55 passed |
| `cd.e2e.ts` "a tab left behind…" before the review fix | failed: the torn-down CD section's "Deploy" dialog covered the CI tab; passes after it (8/8 in the file) |
| `cargo nextest … test(/^cd::agents_deploy_dev/)` | pass (asserts `/ci?section=cd.`) |
| `scripts/dev check` | pass |
| `scripts/dev lint` | pass |
| `scripts/dev test` | pass: 1789 passed, 16 skipped |
| `scripts/dev deps` | pass: cargo-deny and cargo-shear found no issues |
| `python3 scripts/check_repository.py` | pass |
| `cd e2e && npx e2e run` (host load ~75–78) | 444 passed, 1 skipped, 5 failed by timeout in files this change does not touch (editor presence, ownership ×1, start-change, supertask, terminal); rerun alone, 29 of 31 passed, and `ownership.e2e.ts` then passed 4/4 at load ~62 on this branch and on `origin/dev`. Filed as #506 (terminal: #394) |
| `scripts/dev security` | not run (exit 2): Vidoc answers HTTP 401 "Invalid or expired token" for `VIDOC_API_KEY` (#470, ADR-0104) |

Independent review round 1 (fresh session, `om-code-review`, head 769049a6): approved with three
minors and five nits. Fixed: a deploy answered after the person left the CD tab opened its dialog
over CI and wrote `?section=cd&run=` there (regression test first); the late-answer coverage of
CCD-003; ADR-0025 and the decision index mark the amendment; the agents' CD text names the
canonical address; the index order; the architecture line wrap; this spec's status. Accepted: the
`/cd` link nit (Decisions).
