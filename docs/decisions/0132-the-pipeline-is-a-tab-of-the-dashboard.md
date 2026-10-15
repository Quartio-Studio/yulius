# 0132. The Pipeline (stages) is a tab of the Dashboard

## Status

Accepted. Specified in [SPEC-161](../../.ai/specs/SPEC-161-i-dont-like-it-here-move-it-somewhere-el.md);
task #152. Amends [0112](0112-stages-are-spans-written-with-the-state-they-measure.md), which put
the Pipeline page in the sidebar beside CI and CD, and [0125](0125-ci-and-cd-are-one-page-with-two-tabs.md),
which kept it its own entry.

## Context

The sidebar listed Tasks, Git, CI/CD, Pipeline (stages), Secrets and Dashboard. A person pointed at
the "Pipeline (stages)" entry: "i dont like it here, move it somewhere else, maybe as part of
dashboard". The Dashboard already shows the Pipeline's overview card (deployments a week, lead
time, change failure rate, a CI run's p50) with an "Open Pipeline" link; both pages are read-only
analytics of the sandbox with a 7/30/90-day range, an export and a refresh. The CI/CD page
(ADR-0125) already shows two former pages as tabs of one, with the old address as an alias.

## Decision

The sidebar drops the Pipeline (stages) entry. The Dashboard page keeps its heading and gets the
CI/CD page's tab strip (a "Section" radiogroup, arrow keys, roving focus) with **Overview** (the
Dashboard as it was) and **Pipeline (stages)** (the Pipeline page as it was, with its own range,
export, refresh and filters). Overview is `/s/<slug>/dashboard` (with `?days=`); the Pipeline tab
is `?section=pipeline`, followed by its own state (`days`, `subject`, `environment`, `job`, `trend`,
`open`). The router maps `/s/<slug>/pipeline` with any query to that tab, as it maps `/cd`, and the
page writes the canonical address. "In Pipeline" links (CI and CD runs, a task's overview) and the
Overview's Pipeline card open the tab.

The tab strip and section switching move from `pages/cicd.js` to `sectionedPage` in
`pages/common.js`, used by CI/CD and the Dashboard; each section mounts with its header actions
and returns its teardown.

## Alternatives

- **Under More**: hidden by default, and still a page apart from the overview card that points at
  it; the request suggests the Dashboard.
- **The Pipeline's cards appended to the Dashboard (no tab)**: two date ranges, two exports and
  the Pipeline's filters on one page, and the Dashboard's 30-second refresh would reread every stage
  of the range.
- **A third tab of CI/CD**: the Pipeline also measures tasks (SPEC-137), and the request names the
  Dashboard.
- **A copy of the tab strip in `dashboard.js`**: two copies of the same keyboard and address logic
  to keep in step; the shared helper costs nothing.

## Consequences

The sidebar is one entry shorter: Tasks, Git, CI/CD, Secrets, Dashboard, then More. Every link to
the Pipeline keeps working; `/pipeline` is no longer the address a person sees. Switching tabs
starts the other tab with its defaults; each tab's state stays in the address while it is shown. A
link to `/pipeline` with exactly the state already shown keeps that address until a reload, as
ADR-0125 accepted for `/cd`. Secrets keeps its own tab strip.
