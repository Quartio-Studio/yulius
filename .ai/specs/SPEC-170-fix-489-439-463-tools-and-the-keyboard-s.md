---
yulius-task: 6fb5a4c8-565f-476f-85a8-d6dc9d381a19
status: done
---

# SPEC-170: Tools and the keyboard stay where the person left them; presence leaves arrive last

Status: done
Date: 2026-10-15
Owner: Yulius task #135, issues #489, #439, #463 (part of task #118, SPEC-138)
Scope authorization: task #135: "Fix GitHub issues #489, #439 and #463 of Quartio-Studio/yulius
[…] All live in the web UI's tools panel".

## Goal

Three defects of the tools panel and the presence loop it hosts are gone:

- A sandbox left while its page still loads comes back with no tool open (#489, SPEC-113 KST-005).
- The Shell shown again for a new task or sandbox leaves the keyboard on the page; opened by the
  person it takes it (#439, SPEC-056, as the Superagent and Notes do).
- A presence leave sent with `keepalive` as the tab hides can no longer leave a stale caret drawn
  when it overtakes an update still on its way (#463, SPEC-112 NOT-018/NOT-020, SPEC-127).

## Context

- **#489.** `panels.setContext(sandbox, task)` records the tool of the sandbox it leaves
  (`toolOf`, KST-005). `app.js` `handleRoute` calls it only at the end of a route, after awaits
  (`ensureTasks`, `findTaskByNumber`); a route superseded meanwhile returns early. A sandbox left
  while its route was loading therefore never became the panel host's context and has no record:
  on return the host takes the "not visited yet" branch and keeps the other sandbox's tool in view.
  Reproduced by the issue (task #116, `places.e2e.ts`); the test there now waits for the page.
- **#439.** `createShellView().show()` ignores its `opts`; `setContext` shows the current tool
  again with `{ context: true }` for a new task or sandbox, so the Shell's `setTimeout(… focus())`
  takes the keyboard, and its later focus request wins over the task view's (terminal-core
  `focusRequests`, SPEC-056 WUI-102). `agent.js` and `notes.js` honour `opts.context`. A Shell
  view loading its list for the first time also focused when `shown`, regardless of who showed it.
- **#463.** `presence.js` keeps one update on its way at a time, but `send({ going: true })` (the
  page hides or goes) does not wait. The request with `keepalive` can reach the server before the
  earlier one, so receivers see the earlier selection last. Nothing on the wire orders a tab's
  updates. Notes and the Editor share this loop (SPEC-127 EDT-010).

## Requirements

- **TKS-001 (#489).** A sandbox whose route started (the sandbox known) but which was left before
  the route finished is recorded as having no tool open, unless it already has a record (a visit
  earlier in this tab, whose tool the person did not change). A sandbox not visited yet keeps the
  tool in view, as KST-005 says. Several sandboxes passed through while loading are all recorded.
- **TKS-002 (#439).** The Shell shown with `{ context: true }` (a new task or sandbox, a sandbox's
  tool coming back) refits its terminal and leaves the keyboard where it is, also when it loads its
  terminals for the first time then. Opened by the person (rail, link, shortcut) it takes the
  keyboard as before.
- **TKS-003 (#463).** Every presence update a tab sends (notepad and file) carries `seq`, a number
  that grows with each update the tab sends (from 1, per page load). The server accepts `seq`
  (an unsigned integer, default 0) in `PUT …/notes/presence` and `PUT …/files/presence` and relays
  it in `note.presence` and `file.presence`. A receiver ignores an update whose `seq` is not above
  the newest it has seen from that person's tab (`user_id` + `client`), also after that tab left;
  `seq` 0 (absent) is never ignored. What a receiver remembers of tabs that went quiet is dropped
  after the 45 s expiry, so it stays bounded.

## Plan

- [x] Spec, scope and issue checks
- [x] A sandbox left while loading comes back without a tool (#489)
- [x] The Shell shown for a new page keeps the keyboard on the page (#439)
- [x] Presence updates carry a per-tab seq on the server (#463)
- [x] Receivers drop older presence updates from a tab (#463)
- [x] Docs, ADR and changes updated
- [x] Gates green and independent review passed

## Acceptance criteria

- `places.e2e.ts`: studio's task route held loading (`hold=/api/sandboxes/sb_studio/tasks`), the
  person switches to product and opens the Superagent; back in studio, no tool is open. Fails
  without TKS-001.
- `terminals.e2e.ts`: with the Shell open beside task #12, opening task #11 from the sidebar leaves
  the keyboard outside the Shell's terminal; closing and reopening the Shell from the rail puts it
  in. Fails without TKS-002.
- `terminals.e2e.ts`: with the Shell open in studio, product's Git page (product not visited yet)
  builds product's Shell, which loads its terminal without taking the keyboard. Fails without the
  first-load part of TKS-002 (review round 1).
- `notes.e2e.ts`: a teammate's update with a lower `seq` delivered after their `null` draws
  nobody; an update without `seq` is still drawn; the page's own updates carry growing `seq`.
  Fails without TKS-003.
- Rust: `seq` is accepted, defaults to 0 and is relayed for notes and files.

## Test plan

- e2e (mock server): the three cases above, run red before the fix and green after; then the
  touched files (`places`, `terminals`, `notes`, `editor`, `agent-panel`) and the full suite.
- Rust: contract tests for the `seq` default; integration tests `notes::presence_is_relayed_to_readers`
  and `files::presence_is_relayed_to_writers_only` assert the relayed `seq`.
- Gates: `scripts/dev check`, `scripts/dev lint`, `scripts/dev test`, `scripts/dev security`.

## Decisions

- **#489: record on leaving, not on arriving.** The panel host learns at the route's start which
  sandbox is coming (`arriving`) and records it as tool-less when another context replaces it
  before it arrived. Recording at the start instead would break KST-005's "a sandbox not visited
  yet keeps the tool in view" (a first visit would close the panel). Calling `setContext` early
  with no task would re-show the tool and tell the editor of a task change twice per route.
  An existing record is kept: the person did not change that sandbox's tool while passing through.
- **#463: a per-tab sequence number, not ordering the leave.** A page going away cannot wait for
  the answer to the update on its way (it may never come), so the sender cannot order its requests.
  Receivers ordering by `seq` is the issue's alternative and costs one integer. Timestamps were
  rejected (clocks of one tab are monotonic but `Date.now()` is not). See
  [ADR-0138](../../docs/decisions/0138-presence-updates-carry-a-per-tab-sequence-number.md).

## Evidence

- **Red before each fix** (the new test run against the unfixed code):
  - `places.e2e.ts` "a sandbox left while its page loads …": `toBeHidden` failed, the Superagent
    panel was visible in studio.
  - `terminals.e2e.ts` "the Shell shown again for another task …": the keyboard was in the Shell's
    terminal (`expected true to be false`).
  - `terminals.e2e.ts` "the Shell of a sandbox first visited …" (review round 1): the same, with
    `focus: shown` restored in `load`. On the composer page it passed without the fix (the prompt
    keeps the keyboard, `typingElsewhere`), so the test uses product's Git page.
  - `notes.e2e.ts` "a teammate's update arriving after their leave …": first no `seq` was sent;
    with only the sender numbered, alice stayed drawn (`expected ["alice","bob"] to equal ["bob"]`).
  - Rust: `notes::presence_is_relayed_to_readers` and `files::presence_is_relayed_to_writers_only`
    did not compile without `seq` on the events.
- **Green after:** `places.e2e.ts` 21/21, `terminals.e2e.ts` 7/7, `queued-messages`, `rail`,
  `notes` and `editor` e2e files passed. The full e2e suite passed: 489 passed, 1 skipped
  (`npx e2e run`, 25 min). `cargo nextest` for the presence tests 6/6, `yulius-contracts` 46/46.
- **Gate:** `scripts/dev ci` exit 0: 1847 tests passed, 18 skipped, plus doc and lint stages. A
  first run, made while the e2e suite loaded the machine, failed one test outside this change,
  `yulius-git` `ci_checkouts_created_at_once_all_succeed` (`git worktree list` read a registration
  a concurrent `worktree add` was still writing: "failed to read .git/worktrees/j1.2/commondir").
  It passed 15 of 15 reruns under load. This task may not create issues, so it is reported in the
  task summary rather than fixed here.
- **Security:** `scripts/dev security` exited 2, "Vidoc rejected the key (HTTP 401)" (#470). It is
  unrun, not passed (ADR-0104). The CI `security` job reports it as skipped.
- **Review:** an independent review in a fresh session approved 558b0518 with no blocker or major
  findings. Round 1 addressed its three minor findings: the first-load test, this Evidence section
  and merging `dev`. Its nits were left as they are: one shared `focusWanted` flag (no caller
  shows twice in one tick), "Try again" after a failed load, and the by-design TKS-001 choice.
