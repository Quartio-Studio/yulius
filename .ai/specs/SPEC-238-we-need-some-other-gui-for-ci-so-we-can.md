---
yulius-task: 4e385ec2-bed3-48ce-8f81-13bdf362c6b5
status: draft
---

# SPEC-238: CI and CD runs open as a page view, under a live "Now" summary

Status: draft
Date: 2026-10-19
Owner: Yulius task #224
Scope authorization: the person's request (task #224): "we need some other GUI for CI so we can go
back easily after this is closed and we see some dynamic summary all the time", with a screenshot
of the Deploy dialog of a running dev deploy on the CI/CD page's CD tab.

## Goal

A run of CI or CD is a place in the CI/CD page, not a dialog over it. A person who leaves a run
gets back to it in one click (the page's "Now" summary, the browser's Back, the sidebar's CI/CD
link), and while anything runs the page says what runs, which stage it is in and for how long,
updating by itself, on both tabs.

## Context

- **Today (SPEC-151, SPEC-189, SPEC-137).** A CI run (`pages/ci.js`) and a deploy (`pages/cd.js`)
  open in a modal dialog over the CI/CD page: state, stages (the waterfall), the jobs' tabs and the
  log. The address names the run (`?run=<id>`, `?section=cd&run=<id>`) only while the dialog is
  open: closing it writes the plain address back (`replaceQuietly`). So once closed, the run is
  gone from the address, from Back and from the sidebar's remembered place (SPEC-113), and the
  only way back is to find its card in the Runs list, which on the CD tab sits under both
  environment cards.
- **The screenshot.** A running dev deploy: its dialog covers the page; behind it the Runs list
  shows the run as "Deploying · Started 1 min ago", a static line that says nothing of its stage.
- **What exists to reuse.** `loadWaterfall` (stages.js) draws a run's stages from
  `/stages/timeline?subject=ci_run:<id>|cd_run:<id>`; the overviews `/ci` and `/cd` list runs with
  their state, start and (CI) jobs; both tabs already poll every 3 s while something runs.
- **Pages update in place or remount.** `app.js` remounts a sandbox page when its address's query
  changes, unless the page has `update`; a `replaceQuietly` keeps the page mounted.

## Requirements

| ID | Requirement | Evidence |
| --- | --- | --- |
| CRV-001 | Opening a CI run (its card, its commit status link `?run=<id>`) shows it **inside the CI tab**, in place of the tab's list: a region named "CI run" with an "All runs" link back, its live summary, stages, job tabs and log. No dialog. The address is `/s/<slug>/ci?run=<id>`, pushed to history, so Back returns to the list and Forward to the run, and the sidebar's CI/CD link (SPEC-113 places) returns to the run. Opening a run puts the keyboard on its region (announced by name) when the control that opened it went; back in the list, it goes to that run's card. The header's "Check for commits" works in the view too ("Looked for new commits."). | `ci.e2e.ts` |
| CRV-002 | A deploy opens the same way on the CD tab (`?section=cd&run=<id>`), a region named "Deploy". A confirmed deploy (SPEC-189) shows its "Starting" state in that region at once and turns into the run in place; "All runs" while it starts shows the list again with the keyboard on the CD tab, and the deploy goes on with its toast and Open (CDS-004 kept). A refusal is said in the region (CDS-003 kept). Leaving the tab meanwhile still opens and names nothing (CCD-003). | `cd.e2e.ts` |
| CRV-003 | A run's view starts with a **live summary** that stays at the top while its log scrolls (sticky): state, what (label / environment, commit), how long ("1 min 5 s so far", ticking every second while it runs; "took 2 min" and "started 2 hours ago" once ended), the stages running now (each unfinished stage with no unfinished stage under it, waits left out, at most two named: "Now: engine › build, package (gateway) and 2 more", "Now: build") and, for CI, the jobs line ("3 jobs · 1 passed · 2 running"). Its facts are drawn before the stages load, so it does not grow under them; it updates with each refresh. | `ci.e2e.ts`, `cd.e2e.ts` |
| CRV-004 | Above the tabs, the CI/CD page shows a **"Now" summary** on both tabs: each CI run and deploy that is queued or running (state, label, commit, jobs line or environment, time so far ticking), each a link to its run view; with nothing running, one quiet line with the latest CI run's and the latest deploy's outcome and age. It refreshes every 3 s while something runs, every 20 s otherwise, not while the page is hidden (`pollPaused`), and stops when the page goes. A failed read of one overview keeps what was read of it before; a page mounted again draws the last overviews read at once, so the tabs do not move. Each link's name is its visible text (WCAG 2.5.3); the link of the run in view adds no history entry. | `cd.e2e.ts` |
| CRV-005 | Nothing else changes: the confirmations (typed slug, `confirm_ms`, the commit bound), Run again, Cancel, the hygiene card, the waterfall, "In Pipeline", the server and its API. No WCAG violations; no horizontal scroll at 390 px. | e2e, gate |

## Plan

- [ ] Read the product context and related work, then write and validate the spec
- [ ] A CI run opens as a view in the CI tab at its own address, with All runs
- [ ] A deploy opens as the same view on the CD tab, its Starting state included
- [ ] A run view's live summary stays on top: time ticking, the stage running now
- [ ] The Now summary above the tabs shows live CI and CD runs and opens them
- [ ] ADR-0180, architecture, product, e2e README and changes updated
- [ ] Independent review in a fresh session, findings fixed test-first
- [ ] Every gate green, docs and the changes log updated

## Acceptance criteria

- With the mock's running dev deploy, the CD tab shows the "Now" summary with "Deploying" and the
  time moving; its link opens the deploy as a region at `?section=cd&run=…`; Back returns to the
  list; Forward and the sidebar's CI/CD link return to the deploy.
- A CI run opened from its card is the "CI run" region with the failing job selected; "All runs"
  shows the list; no `dialog` element opens for a run.
- A running run's summary says the stage running now and a time that grows between two reads.
- A confirmed production deploy shows "Starting" in the Deploy region, then becomes the run there.
- The CI and CD e2e suites, the pipeline, places and loading-shapes suites pass; axe finds nothing.

## Test plan

- e2e (`ci.e2e.ts`, `cd.e2e.ts`): the run view, its address and Back/Forward, "All runs", the
  sticky summary (its time grows, the stage now), the Now summary on both tabs and its link, the
  quiet line when nothing runs, no run dialog; SPEC-189's starting flow in the region.
- Existing suites that opened run dialogs (`pipeline`, `places`, `loading-shapes`) find the region
  instead; their checks (waterfall, address not rewritten when leaving, loading shape) stay.
- `scripts/dev ci` (the gate) and `scripts/dev security`.

## Decisions

- **A view inside the page, at its own address (ADR-0180)**, rather than a modal: a modal hides
  the page and drops its address when closed, which is the reported problem. A separate route
  (`/s/<slug>/ci/runs/<id>`) was weighed: the `?run=` addresses already exist (commit status links,
  toasts, `/cd?run=`), so keeping them and changing what they show breaks no link.
- **Open by navigation (push), not in place.** The page remounts on the new address (app.js's
  rule); the run and the list are both history entries, so Back/Forward and SPEC-113's places work
  without new routing code. Only the starting deploy turns into its run in place
  (`replaceQuietly`, as SPEC-189), because its view exists before its id does.
- **The Now summary lives on the CI/CD page, not in the app's top bar.** It needs both overviews
  polled every 3 s; doing that on every page of every sandbox costs requests for a view the person
  did not open. A global indicator stays a possible follow-up.
- **The stage running now comes from the timeline the view already loads** (`loadWaterfall` returns
  it), so no new API.

## Evidence

(filled when done)
