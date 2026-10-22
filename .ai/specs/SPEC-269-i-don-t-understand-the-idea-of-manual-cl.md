---
yulius-task: e7691505-2d0c-4385-bb8f-c282747c398d
status: in-progress
---

# SPEC-269: Yulius cleans up after itself, and the hygiene card says what it did

Status: in progress
Date: 2026-10-22
Owner: Yulius task #255
Scope authorization: task #255 ("I don't understand the idea of manual cleaning, + after cleaning
i don't see any interaction from the interface. I want at least better ux (the same way how we
improved deployment, check the history or sessions) or just better cleaning system. Think how to
improve it", a screenshot of the CI tab's "Repository hygiene" card listing two GitHub branches of
merged pull requests, two local branches and three worktrees of archived tasks). Amends SPEC-001
DLV-007. Decision: ADR-0199. Follows SPEC-189 (a confirmed deploy answers at once).

## Goal

Removing what no task needs any more is Yulius's job, not a person's: the 6-hourly hygiene round
removes every safe item by itself. The card says what it is for and when Yulius cleans next; a
person's "Clean up" (do it now) shows that it is running and then what was removed and what was
kept and why, on the card itself, until the next cleanup.

## Context

Read in the code on `dev` (21c26d8b):

- `App::start_hygiene` (`crates/yulius-app/src/ci/hygiene.rs`) scans every sandbox 10 minutes
  after start and then every 6 hours, and only **lists** stale items. Only a person (CI tab) or
  an agent (`repo_cleanup`) removes them. The items listed as `safe` lose nothing by design
  (GitHub keeps a pull request's commits; a local branch is on GitHub; a worktree is removed only
  when clean), so the list asks a person to make a decision that has only one answer. Branches of
  pull requests merged on GitHub (not through Yulius) and worktrees of archived tasks pile up.
- `pages/ci.js` `cleanup()`: after the confirmation the card does not change while
  `POST …/hygiene/cleanup` runs (a fresh scan with a GitHub fetch, each deletion, a refetch and a
  second scan: seconds); then a toast says only counts ("3 items removed. 2 items kept.") and the
  list is redrawn. What was removed, what was kept and **why** is in the answer (`removed`,
  `skipped[].reason`) but never shown. When everything was kept (for example a worktree with
  uncommitted changes, or GitHub refusing a deletion), the list looks exactly as before the click.
- The card's footer "Agents clean up with repo_cleanup; merged pull requests lose their branch
  right after the merge." explains the agents' tool, not the card.
- SPEC-189 fixed the same shape for deploys: state shown at once, elapsed seconds, the result in
  the same place.

## Requirements

- HSC-001. The background hygiene round removes every safe item of each sandbox as `yulius`,
  through the same path as a person's Clean up (fresh scan, the same safety checks, each removal
  audited `hygiene.cleanup`). Unsafe items (`idle`: unmerged work with no pull request) are never
  removed automatically; they stay listed.
- HSC-002. The hygiene report carries `last_cleanup` (when, by whom: `Yulius` or the person's
  login or the agent, what was removed and what was kept with its reason) and `next_cleanup_at`
  (when the automatic round runs next; null when it is not scheduled). An automatic round that
  removed and kept nothing does not replace the last cleanup. Both are kept in memory, per
  sandbox, across scans (the audit log keeps the history).
- HSC-003. The card says what it is for: "Branches and worktrees no task needs any more. Yulius
  removes the safe ones by itself (next <relative time>); Clean up removes them now." An unsafe
  item reads "Kept" and says it is unmerged work Yulius never deletes by itself.
- HSC-004. Confirming Clean up turns the card busy at once, before the server answers: both
  buttons disabled, Clean up reading "Cleaning up…", and a status live region "Removing <n>
  items…" with the seconds so far. Check now reads "Checking…" while it scans.
- HSC-005. When the answer comes, the card shows the cleanup's result from `last_cleanup`:
  "Cleaned up <relative> by <who>: <n> removed, <m> kept", the removed items, and each kept item
  with its reason. A refusal or a network error is said in the card ("Couldn't clean up: …") and
  the buttons come back. No toast repeats what the card says.
- HSC-006. Nothing else changes: what is stale, what is safe, `force`, the bounds, the REST and
  MCP requests and their answers (two optional fields added to the report).

## Plan

- [x] Read the product context and related work, then write and validate the spec
- [ ] Yulius removes the safe leftovers by itself on its 6-hourly round
- [ ] The hygiene report carries the last cleanup and the next automatic one
- [ ] The card explains itself and shows a cleanup running and its result
- [ ] Independent review in a fresh session, findings fixed test-first
- [ ] Every gate green on the branch merged with current dev

## Acceptance criteria

- AC1 (HSC-001): an automatic round on a sandbox with a merged branch, a closed pull request's
  branch and recent unmerged work removes the first two from GitHub, keeps the third, and audits
  each removal as `yulius`.
- AC2 (HSC-002): after a person's cleanup the report (`GET …/hygiene`, also after another scan)
  has `last_cleanup` with their login, the removed and the kept items; an automatic round with
  nothing to do leaves it as it was; `next_cleanup_at` is set once the round is scheduled.
- AC3 (HSC-003..005): in mock `studio`, the card names the automatic cleanup; with the answer held,
  confirming Clean up shows "Cleaning up…" and "Removing 3 items…" at once; after the answer the
  card says "3 removed, 1 kept" by mkarp, lists the three removed and the kept branch with its
  reason, with no WCAG violation; a failing cleanup says "Couldn't clean up" in the card.

## Test plan

- Integration (`crates/yulius/tests/local_ci.rs`): `the_hygiene_round_removes_safe_items_itself`
  (AC1, AC2's automatic part), `a_cleanup_is_kept_on_the_report` (AC2).
- e2e (`e2e/tests/ci.e2e.ts`): the existing hygiene test rewritten for the card's result, plus
  "a cleanup shows at once that it runs" (held answer) and "a failed cleanup says why in the card"
  (AC3). The mock's cleanup can be held (`__yuliusMock.holdHygieneCleanup`) and failed.
- Gates: `scripts/dev ci`, `scripts/dev security`, the whole e2e suite.

## Decisions

- **Clean up safe items automatically (ADR-0199)** instead of only explaining the manual button.
  Alternatives: (a) better copy and feedback only: the person still has to come and press a button
  whose only sensible answer is yes; (b) clean right after each task ends or is archived: more
  triggers to keep correct, and the 6-hourly round already exists and is bounded; (c) a per-project
  switch: nobody asked for keeping leftovers, and the round removes only what loses nothing, so
  there is no switch until someone needs one. The button stays as "do it now", for a person who
  wants the list empty before the next round.
- **The result lives on the card, not in a toast** (as SPEC-189's dialog): the card is where the
  person looks, and it survives a reload while the server runs. It is in memory only: the audit
  log is the durable history, and a restart's first round writes a new one when it removes
  anything.
- **Elapsed seconds, not a progress bar**: the server answers once, after every deletion.

## Evidence

(filled when done)
