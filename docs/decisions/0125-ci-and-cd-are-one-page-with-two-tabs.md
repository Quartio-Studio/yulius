# 0125. CI and CD are one CI/CD page with two tabs

## Status

Accepted. Specified in [SPEC-151](../../.ai/specs/SPEC-151-make-ci-cd-one-tab-with-a-tab-like-in-se.md);
amends [0025](0025-sandbox-navigation-more-group.md) and SPEC-134, which gave CI (tests) and CD
(deployment) an entry each in the sandbox navigation. Its Pipeline alternative is amended by
[0132](0132-the-pipeline-is-a-tab-of-the-dashboard.md): the Pipeline is a tab of the Dashboard.

## Context

The sidebar listed "CI (tests)" and "CD (deployment)" as two pages next to each other, each with
its own heading. A person asked to "make CI/CD one tab with a tab like in secrets team/agent": the
Secrets page shows Team secrets and Agent secrets as two sections of one page, picked with a tab
strip and kept in `?section=agents`. Links into the two pages already exist outside the UI: the
`yulius/ci` commit status targets `/s/<slug>/ci?run=<id>` and agents' CD text names
`/s/<slug>/cd`.

## Decision

The sidebar has one entry, **CI/CD**, at `/s/<slug>/ci`. Its page (`pages/cicd.js`) is titled CI/CD
and shows the Secrets page's tab strip (a "Section" radiogroup, arrow keys, roving focus) with
**CI (tests)** and **CD (deployment)**. The CI tab is the plain address; the CD tab is
`?section=cd`, written quietly on a change, so a reload and the sidebar's place memory (SPEC-113)
keep the tab. `ci.js` and `cd.js` mount their section into the page with its header action and
return a teardown, as Secrets' sections do; the shown section's refreshes and run dialog stop when
the other tab is chosen. The router maps `/s/<slug>/cd` (with any `?run=`) to the CD tab, and the
page writes the canonical address. A CD run's dialog is `?section=cd&run=<id>`.

## Alternatives

- **Two routes that each draw the tab strip and navigate to each other**: every tab change
  remounts the page and loses the keyboard focus on the tabs.
- **A new `/cicd` path with `/ci` and `/cd` as aliases**: every link, test and the commit status
  change for no benefit to the person.
- **Fold the Pipeline (stages) in as a third tab**: not asked, and the Pipeline also measures
  tasks (SPEC-137), so it stays its own entry.

## Consequences

The sidebar is one entry shorter. CI's address and its commit-status links are unchanged; `/cd`
keeps working as an alias but is no longer the address a person sees. The CI/CD page's sub-heading
follows the shown tab.
