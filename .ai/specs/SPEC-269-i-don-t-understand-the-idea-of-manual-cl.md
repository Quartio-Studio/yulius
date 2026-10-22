---
yulius-task: e7691505-2d0c-4385-bb8f-c282747c398d
status: done
---

# SPEC-269: Yulius cleans up after itself, and the hygiene card says what it did

Status: done
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
- HSC-007. Every check holds at the deletion (review round 1): a task's worktree is in use unless
  its task rests (closed, stopped, failed), read again right before the removal; it goes only when
  Git ignores nothing in it but rebuildable caches (SPEC-019's rule); a merged or closed pull
  request's branch is safe only while its tip is the pull request's head, and is deleted only while
  its tip on GitHub is still the commit the scan judged (a failed fetch or a push since keeps it);
  a local copy is judged again against GitHub after the remote deletions and a fresh fetch (kept
  when the fetch fails); a task that cannot be read keeps its worktree; a snapshot import's
  checkout (`worktrees/import-…`) is the import's; one cleanup or requested scan of a sandbox runs
  at a time.
- HSC-006. Nothing else changes: what is stale, what is safe, `force`, the bounds, the REST and
  MCP requests and their answers (two optional fields added to the report).

## Plan

- [x] Read the product context and related work, then write and validate the spec
- [x] Yulius removes the safe leftovers by itself on its 6-hourly round
- [x] The hygiene report carries the last cleanup and the next automatic one
- [x] The card explains itself and shows a cleanup running and its result
- [x] Review round 1 findings fixed test-first
- [x] Independent review in a fresh session, findings fixed test-first
- [x] Review round 3 approves the exact head
- [x] Every gate green on the branch merged with current dev

## Acceptance criteria

- AC1 (HSC-001): an automatic round on a sandbox with a merged branch, a closed pull request's
  branch and recent unmerged work removes the first two from GitHub, keeps the third, and audits
  each removal as `yulius`.
- AC2 (HSC-002): after a person's cleanup the report (`GET …/hygiene`, also after another scan)
  has `last_cleanup` with their login, the removed and the kept items; an automatic round with
  nothing to do leaves it as it was; `next_cleanup_at` is set once the round is scheduled.
- AC3 (HSC-003..005): in mock `studio`, the card names the automatic cleanup; with the answer held,
  confirming Clean up shows "Cleaning up…" and "Removing 3 items…" with the seconds so far at
  once; after the answer the card says "Cleaned up just now by mkarp: 3 removed." and lists the
  three removed, the unmerged branch stays listed as "Kept", the result is there after a reload,
  with no WCAG violation; a failing cleanup says "Couldn't clean up: …" in the card.

## Test plan

- Integration (`crates/yulius/tests/local_ci.rs`): `the_hygiene_round_removes_safe_items_itself`
  (AC1, AC2's automatic part), `a_cleanup_is_kept_on_the_report` (AC2),
  `a_cleanup_never_loses_commits_that_only_a_branch_held` (HSC-007: commits pushed after a pull
  request closed; a local copy whose GitHub branch the same cleanup deleted),
  `a_failed_fetch_never_deletes_what_moved_on_github` (HSC-007: a ref lock fails the fetch; the
  branch that moved on GitHub and a local copy stay, an unmoved merged branch goes). `disk.rs`:
  `the_hygiene_round_keeps_paused_work_and_files_nobody_can_rebuild` (HSC-007: a paused merged
  task, an archived worktree with an ignored `.env`, an archived one with caches only) and
  `hygiene_keeps_an_ended_worktree_that_still_holds_ignored_files` (now with an `.env`: caches
  alone no longer keep an ended task's worktree). Domain: `remote_branches_are_stale_only_without_open_work`.
- e2e (`e2e/tests/ci.e2e.ts`): "repository hygiene says Yulius cleans up itself and that
  unmerged work stays", "a cleanup shows at once that it runs, then what it removed and kept"
  (held answer) and "a failed cleanup says why in the card and can be tried again" (AC3), replacing
  the old count-toast test. The mock's cleanup can be held (`__yuliusMock.holdHygieneCleanup`) and
  failed (`failHygieneCleanup`). Review round 1 added: the seconds outside the live region, the
  keyboard kept on the card, the toast check, the retry after a failure, and "a cleanup still
  running after the person left the tab shows as running when they come back". Round 2 added "a
  cleanup that fails after the person left the tab says why when they come back" and "a cleanup
  round that runs late reads as shortly, not in the past".
  `loading-shapes.e2e.ts` follows the card's new intro line.
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
- **Review round 1** (fresh session, head 876eca4e, request changes; all own code): a paused
  (`WaitingCapacity`) task's worktree could go (major, fixed test-first: in use unless resting,
  read again before removal); archived, merged or deleted tasks' worktrees lost ignored files that
  are no caches (major, fixed test-first: SPEC-019's rebuildable-cache rule for every task
  worktree; an ended task's worktree holding only caches now goes, as the disk upkeep already
  removes it); the scan-to-removal race (closed by the same re-read); a closed pull request's
  branch with later commits and its local copy could both go (fixed test-first: tip on the pull
  request, local copies judged again after a fetch); the live region read the seconds every second
  and `aria-busy` sat on its ancestor, focus fell to the page (fixed: seconds beside it, no
  `aria-busy`, focus on the card); the busy state was lost on a remount and an older read could
  draw over the answer, and two cleanups could overlap (fixed: per-sandbox state in the page, one
  cleanup per sandbox on the server); docs (operations.md, ADR-0199's context, the README order,
  `superadmin` in api.md) and nits (an explicit automatic flag instead of the `yulius` actor,
  "task #12", the confirmation's words) fixed. The concurrent-cleanup lock has no test that fails
  without it (the race is not reproducible through the API without a pause point); it is a
  per-sandbox `tokio::sync::Mutex` held for the whole cleanup.
- **Review round 2** (fresh session, head 34ee351a, request changes; all own code): with the
  scan's fetch failed, the round judged GitHub branches by stale refs and could delete commits
  pushed since (major, fixed test-first: each deletion first compares GitHub's tip with the
  clone's `origin/<branch>` and keeps a branch that moved); a store error let a worktree go and
  the task was read before the ignored-files walk (fixed: read last, an error keeps it); snapshot
  imports' checkouts counted as deleted tasks' (fixed: `IMPORT_WORKTREE_PREFIX` names them the
  import's); a failed action's message was lost on a remount (fixed test-first: kept per sandbox
  in the page); Check now and `repo_hygiene refresh` scanned outside the lock (fixed: they take
  it); a late round read "next 5 min ago" (fixed test-first: "shortly"); `has_ignored_files` had
  no caller left (removed). Not covered by a test that fails without it: the lock and the
  re-read's timing (no pause point in the cleanup), as in round 1.

## Evidence

- Red first:
  - The automatic round only listed items: `the_hygiene_round_removes_safe_items_itself` failed
    with every branch still on origin.
  - `a_cleanup_is_kept_on_the_report` failed (no `last_cleanup`).
  - `npx e2e run tests/ci.e2e.ts` with the old `pages/ci.js`: 3 failed.
  - Review round 1's tests: `the_hygiene_round_keeps_paused_work_and_files_nobody_can_rebuild`
    failed on the paused task's worktree, and again with the cache check disabled.
    `a_cleanup_never_loses_commits_that_only_a_branch_held` failed with `tip_on_pull` ignored
    (feat/later listed as `pull_closed`), and again with the local re-check disabled (the local
    copy deleted).
  - Review round 2's tests: `a_failed_fetch_never_deletes_what_moved_on_github` failed
    (feat/closed deleted on GitHub with its new commit). The two new e2e tests failed against the
    previous page.
  - The cd flake ("the Now summary…"): reproduced with a 22 s probe delay before the phone-width
    check (fails without `ciRunMs`, passes with it).
- Gates at 60e464a1 (dev merged):
  - `scripts/dev ci`: exit 0 (nextest 2451 passed, 17 skipped).
  - `npx e2e run`: 676 passed, 1 skipped (`agent.e2e.ts`, needs a real model), 1 failed:
    `secrets.e2e.ts` "a failed copy shows no check…" (axe target-size on a Close button). That is
    the open flaky issue #640, not this change: it ran while clippy loaded the machine, and passed
    3 of 3 runs on its own. `ci`, `cd` and `loading-shapes` passed.
  - The earlier full run at 876eca4e had 5 terminal and times poll timeouts under the reviewer's
    builds; the same files passed 25 of 25 on an idle machine.
- `scripts/dev security` at 60e464a1: exit 0, no open critical, high or medium finding in the
  change. Three low findings in changed lines:
  - The worktree path is built from the item name in `remove_task_worktree`. Not fixed: only names
    the fresh scan produced reach it, which are Git's own paths relative to the sandbox, and the
    Git adapter's removal refuses any path outside `worktrees/` (`owned`).
  - The hygiene lock map's poisoning gave a fresh lock. Fixed: the poisoned map is used as is.
  - The snapshot import's `exists()` check before `add_worktree`. Pre-existing; this change only
    named its prefix. Filed as #824.
  - Findings outside the change are filed by Yulius CI.
- Seen in the mock UI (dark theme, 1280 px):
  - The card names Yulius's own cleanup and its next round.
  - After Clean up the card says "Cleaned up just now by mkarp: 3 removed." with the three items
    and "Still listed:" for the kept unmerged branch.
- Gate at f62c00fc: one failure outside this change.
  `local_ci::a_run_s_jobs_record_their_stages_and_test_counts` (from #490) read "real durations:
  1540 ms, 480 ms": stage bounds are stamped when the markers are read, so under the gate's load
  the boundary between two 1 s stages moved. Its floor is now "each ≥ 100 ms, together ≥ 1500 ms",
  which still rejects zero or fake durations.
- Not testable here: a real 6-hourly round on the instance against GitHub. The round is driven in
  tests through `hygiene_round`, with real Git over local remotes and the GitHub fake.
- Independent review in fresh sessions:
  - Round 1: request changes at 876eca4e.
  - Round 2: request changes at 34ee351a.
  - Both answered above. Round 3 reviews the head that delivers.
