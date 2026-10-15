---
yulius-task: 19ed6b36-b342-4870-8933-99af8f2ed570
status: in-progress
---

# SPEC-172: A folded subscription shows its 5-hour and weekly usage at a glance

Status: in-progress
Date: 2026-10-15
Owner: Yulius task #162
Scope authorization: task #162: "[screenshot of the Subscriptions list: one bar and one percent
per subscription, each row folded with a chevron] in every subscribtion view we want both session
and weekly usage visible instantly, without a need to expand".

## Goal

Every list of subscriptions shows, for each subscription and without unfolding it, both its
5-hour window ("session" in Claude's own words) and its weekly usage: a short name, a bar and the
percent used of each.

## Context

- One component draws a subscription everywhere: `accountUsage` in
  `crates/yulius-server/web/assets/layout.js` (ROT-009, SPEC-001). It has two forms:
  - **folded** (`foldable: true`): a `<details>` whose summary is the avatar, the name, the state
    and a "glance", then a chevron; the body holds one row per usage window with its reset time.
    Used by the Status panel's Subscriptions section and the Dashboard's Subscriptions card (the
    screenshot, from the Dashboard).
  - **card** (`foldable: false`): the Subscriptions page (`pages/admin.js`), which already shows
    every window's row open, so it needs no change.
- The glance shows only the 5-hour window (`usage.primary`): one bar and one percent. The weekly
  window (`usage.secondary`) appears only after unfolding. That is the defect the request names.
- The 5-hour window is deliberately not called "session" in Yulius: a session is a Claude Code run
  (#147, `limitName`). The folded row keeps that vocabulary.
- The Status panel is at most 440 px wide (`.status-pop`), the whole width of a phone below that,
  so the row's name, plan badge and state badge share it with the glance.

## Requirements

- **GLN-001.** A folded subscription's summary shows two usage lines, the 5-hour window first and
  then the week: each a short name ("5 h", "Week"), a bar and the percent used, rounded, in the
  tone of its value (ok, warn, danger). An unknown value reads "—" with a hatched bar, never 0 %.
- **GLN-002.** Each bar is a `meter` named by its window ("5-hour window", "This week") with its
  value as text, so a screen reader hears both without unfolding.
- **GLN-003.** Unfolded, the glance hides as before: the body's rows say the same with reset times.
- **GLN-004.** Where the row is narrow (the list under 360 px wide: the Status panel on a phone),
  the glance keeps both names and percents and drops the bars; nothing overflows the row.
  Where the list is under 480 px (the Status panel), a folded row's state badge shows its icon
  and keeps its words for screen readers and on hover. The subscription's name keeps at least
  64 px (or all of itself) and its full text on hover.
- **GLN-005.** The Subscriptions page (card form) is unchanged.

## Plan

- [x] Read the product context and related work, then write and validate the spec
- [ ] A folded subscription shows its 5-hour and weekly usage side by side
- [ ] Both usage figures stay readable at phone width
- [ ] Independent review in a fresh session, findings fixed test-first
- [ ] Every gate green, docs and the changes log updated

## Acceptance criteria

- On the Dashboard and in the Status panel, every folded subscription shows "5 h" and "Week" with
  their percents and two named meters, without being opened (mock data: Claude #1 reads 42 % and
  19 %).
- A subscription without a weekly reading shows "—" for the week (mock: Claude spare).
- Opening a subscription hides the glance and shows the full rows, as before.
- At 360 px wide the Status panel's rows show both percents, no bar in the glance, and no row
  wider than the list.
- axe reports no violations on the Dashboard.

## Test plan

- e2e (`e2e/tests/subscriptions.e2e.ts`): the Dashboard's folded rows show both windows' names,
  percents and meters; unknown weekly reads "—"; opening hides the glance.
- e2e: the Status panel at phone width (360 px) shows both percents per subscription with the
  glance's bars hidden and no horizontal overflow.
- The existing subscription, consistency and loading-shape e2e tests keep passing.

## Decisions

- **Two stacked lines in the glance, not two side-by-side bars.** Side by side needs two
  label-bar-percent groups in one line (about 300 px), which leaves the name no room in the 440 px
  Status panel; stacked, the glance stays about 150 px wide and fits the two-line height the name
  and email already take.
- **"5 h" and "Week", not "Session".** Yulius names windows by their length because a session is a
  Claude Code run (#147); the meters' accessible names use the full names of the unfolded rows.
- **Bars dropped below 360 px rather than a second layout.** The percents carry the information;
  a container query on the list keeps one markup for every width.
- **The 5-hour window and the overall week only.** Per-model weekly caps (Opus, Sonnet) stay in the
  unfolded rows: the request names session and weekly usage, and a third line would not fit the
  row's two-line height. The glance's short names are fixed, as its title was before: Claude
  reports these two windows with these lengths.
- **The state badge gives way before the name** (review round 1): beside the wider glance, a badge
  such as "Limit reached" left "Claude #5" 55 of 68 px in the 406 px Status panel and 31 px at
  360 px. Its icon carries the tone; its words stay for screen readers and on hover.
- No ADR: this is a presentation detail of ROT-009 with no lasting architectural choice.
