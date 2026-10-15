---
yulius-task: 11930dd3-70c1-4b2b-8411-8a797b83103d
status: done
---

# SPEC-056: open GitHub issues triaged; the web UI keeps the person's keyboard, scroll and text

Status: implemented
Date: 2026-10-13
Owner: Yulius task #47
Scope authorization: user request: "Fix all open actionable issues from github without creating
new issues and close them".

## Goal

Every open, actionable GitHub issue of the repository is either closed as already fixed, fixed by
this task, or handed to a chained Yulius task that fixes and closes it. No new GitHub issue is
created. This task's own change fixes the web UI defects in which the page takes the person's
keyboard, scroll position, or typed text: #325, #323, #310, #288, #316, #324 and #259.

## Context

The triage covered 52 open issues on 2026-10-13.

**Closed as already fixed on `dev`, each with a comment naming the change:**
- #242: SPEC-050 IDN-101, PR #238.
- #253, #301 and #308: SPEC-050 TRM-101, PR #319.
- #280 and #282: SPEC-053, PR #314. The VM measurement stays in #321.
- #44: a tracking issue whose work was all delivered.

**Not actionable here, left open:**
- #5, #6, #7, #11 and #14: checkpoints that need live Codex or Claude accounts, a real GitHub
  App, or real Chromium and openvscode-server on the VM.
- #15: the progress tracker.
- #321: needs a person to promote `dev` to `main` and update the VM.

**Handed to chained tasks, one concern each.** Three lanes run one task at a time, so parallel
Rust builds do not push the sandbox over its disk quota again (#240):

| Task | Issues |
| --- | --- |
| #49 | #317, #272 |
| #50 → #53 → #56 → #59 → #62 | #240 #256 #258 #245 → #248 #249 #255 #251 → #243 #320 #322 #294 #269 → #223 → #170 |
| #51 → #54 → #57 → #60 → #63 | #213 → #284 #285 #290 → #276 #277 #278 → #305 #264 #265 #271 → #303 |
| #52 → #55 → #58 → #61 → #64 | #295 #291 → #169 → #275 #293 → #252 #266 → #250 |

**This task's own defects** live in the web UI's plain ES modules under
`crates/yulius-server/web/assets/`:

- **#325:** `typingElsewhere` (`terminal-core.js`) does not count Monaco's EditContext `div` as a
  text field. On Chromium, a task terminal that shows late takes the keys typed into the Editor.
- **#323:** a focus asked for before a terminal showed lands when it shows, even after a later
  terminal (a Shell, the Agent) asked for the keyboard and got it.
- **#310:** `notes.js` `show()` always focuses the notepad, also when `panels.js` shows it again for
  a new page (`{ context: true }`).
- **#288:** `openMenu`'s `place()` (`dom.js`) runs again on a height-only resize. For a filtered
  menu it calls `applyFilter()`, which sets `list.scrollTop = 0`.
- **#316:** `loadDetail` (`taskview.js`) applies every task-detail response when it lands, so an
  older read can overwrite a newer one.
- **#324:** the queued messages card hides as soon as the task starts, hiding the typed text and
  the refusal.
- **#259:** the composer's `destroy()` revokes the thumbnail object URLs but not the ones it made
  for files opened in a tab.

The e2e suite runs these against the mock (`?mock=1`). It missed the defects because no test
covered:
- Chromium's EditContext;
- two focus requests where the later terminal shows first;
- Notes shown again on a new page;
- a resize of a scrolled, filtered menu;
- out-of-order detail reads;
- a start while text sits in the queue box;
- URL revocation.

## Requirements

| ID | Requirement | Evidence |
| --- | --- | --- |
| WUI-101 | A terminal that shows late leaves the keyboard in an element that takes text: a textarea, a text input, a contenteditable element, an element with an EditContext, or anything inside a Monaco editor (#325). | `editor.e2e.ts` |
| WUI-102 | A focus request records its order across all terminals of the page. A request made before the terminal showed lands only if no later request was made, or if the keyboard is on nothing (the page body). The terminal asked for last keeps the keyboard (#323). | `queued-messages.e2e.ts` |
| WUI-103 | Notes shown again because the page or task changed (`{ context: true }`) does not take the keyboard. Opened by the person, it focuses its notepad (#310). | `notes.e2e.ts` |
| WUI-103a | The Shell shown again because the page, task or sandbox changed (`{ context: true }`) refits and does not take the keyboard, also while it loads its terminals; opened by the person (the rail), it focuses its terminal (SPEC-170 TKS-002, #439). | `terminals.e2e.ts` "the Shell shown again for another task …" |
| WUI-104 | Placing a menu again keeps its list's scroll position. Only editing the filter scrolls the list back to its top (#288). | `menus.e2e.ts` |
| WUI-105 | The task view never applies a detail answer older than one it already applied, and only the latest read's answer sets the task's summary (its status) or reports a failure (#316). If the newest read fails, an older answer's details still count. | `queued-messages.e2e.ts` |
| WUI-106 | A queued messages card that holds unsent text or a refusal stays when the task starts (`starting` or running). It says the task started, keeps the text selectable, disables Queue, and can be dismissed (#324, SPEC-052 QMS-006). A task that ends or is archived instead takes the card away. A task that waits again before it ran gets the plain box back, without the old refusal. | `queued-messages.e2e.ts` |
| WUI-107 | Leaving the composer revokes every object URL it created: thumbnails and files opened in a tab (#259). | `filedrop.e2e.ts` |

## Plan

- [x] Open issues triaged: fixed ones closed, the rest handed to chained tasks
- [x] A terminal that shows late leaves the keyboard with the Editor and a later terminal (#325, #323)
- [x] Notes and filtered menus keep the person's place (#310, #288)
- [x] The task view applies only its latest detail and keeps an interrupted queued message (#316, #324)
- [x] Leaving the composer revokes every object URL it made (#259)
- [x] Independent review approves the head, findings fixed test-first
- [x] Gates green, spec evidence and changes log written, issues closed

## Acceptance criteria

- Each requirement has an e2e test that fails on `dev` (81bf3aa) and passes with the fix.
- The focus suites stay green: `terminal`, `terminals`, `agent-panel`, `queued-messages`,
  `editor` and `notes`. So do `menus` and `drafts`, and the full e2e suite.
- `scripts/dev ci` passes, Yulius CI is green on the head, and an independent review approved it.
- Every issue this task fixes is closed with a comment, and every handed-off issue is named in a
  chained task's request.

## Test plan

- Run each new e2e case on the unfixed code (it must fail), then with the fix:
  `npx e2e run tests/<file>.e2e.ts`.
- Run the full e2e suite: `cd e2e && npx e2e run`.
- Run `scripts/dev ci` for the Rust and repository gates. The change touches only web assets, so
  this guards the embedded assets and the repository checks.

## Decisions

- **The work is split into chained tasks, not one pull request.** About 40 actionable issues
  across the supervisor, delivery, CI, vault, MCP and web UI would be far over the about 400 lines
  per pull request, and no reviewer could judge them together. The project's rules ask for bigger
  work to be split into chained tasks. Three sequential lanes keep at most three Rust builds
  beside this task, because parallel builds are what pushed the sandbox over its quota (#240,
  #256, #258).
- **No new GitHub issue is created**, as the request says. Follow-ups go into comments on the
  existing issues, and the chained tasks are told the same.
- **This task keeps the web UI group.** It needs only the mock and the e2e suite, so it adds no
  Rust build to the disk while the lanes start.
- **#324 keeps the card instead of moving the text to a notice.** The card already holds the text
  and the reason in place, as QMS-006 says. Keeping it, with Queue disabled and a Dismiss button,
  is the smallest change that loses nothing.
- **#323 uses a page-wide request order, not the element that asked.** Remembering the element
  that asked for the keyboard broke the Shell panel, which rebuilds its tab strip after asking
  (SPEC-052 review). A counter shared by every terminal of the page does not depend on the DOM.
- **Two narrow focus gaps from the review stay as they are**; #323's acceptance criteria are met
  without them.
  - The lazy terminal wrapper (`terminal.js`) passes a focus asked for before the terminal core
    loaded only when the core arrives, in the order the views were created. This matters only on
    the first load of the core, which is preloaded after sign-in.
  - A click straight into another terminal is not a focus request, so a task terminal asked for
    earlier can still take the keyboard from it when it shows. This was the behaviour before too.
  - Closing either gap means changing focus rules shared by every terminal, beyond this fix.
- **The summary is not merged from a superseded read** (review round 3).
  - If the latest detail read fails, the newest successful older answer still updates the details
    (the step list, the request).
  - It does not update the summary in the header (status, step counts). The status stays current
    through task events. The step counts wait for the next successful read, and the failed read
    has warned the person.
  - Merging only the fields that task events do not own would close that gap, but the rule would be
    more complex than this defect deserves.
- **The third review round is the last full one.** After round 3, only a short confirmation pass
  covered the documentation-only correction above, as the rules allow: decide and record why after
  three rounds.
- No ADR: nothing lasting about the architecture is decided.

## Evidence

- **Regression tests fail before their fix.** Each test was committed before its fix and run
  against the unfixed assets. They failed on their own assertion:
  - #325: `editor.e2e.ts`, the Editor value after the release;
  - #323: `queued-messages.e2e.ts:146`, the Shell no longer focused;
  - #310: `notes.e2e.ts`, the notepad focused;
  - #288: `menus.e2e.ts`, `expected 0 to be 120`;
  - #316: `queued-messages.e2e.ts:174`, "Starting" again after the held read;
  - #324: "The task has started" missing;
  - #259: `filedrop.e2e.ts`, the URL not revoked.
- **Review rounds.**
  - Round 1's new tests failed before their fixes:
    - the older read is not applied when the newer one fails;
    - a cancelled task keeps the card;
    - a stale refusal stays after a wait.
  - Round 2's new test failed before its fix: `queued-messages.e2e.ts:297`, "Starting" again after
    a failed newer read.
  - All of them pass with their fixes.
- **The reviewer re-checked round 1's tests.** In a separate checkout with the old
  `terminal-core.js`, `notes.js`, `dom.js` and `taskview.js`, all 8 new tests failed.
- **Gates at 81369b8:**
  - `cd e2e && npx e2e run`: exit 0, 235 passed, 1 skipped. The skipped one is the agent test,
    which needs a model key.
  - `scripts/dev ci`: exit 0, nextest 1266 passed, 16 skipped (the opt-in live tests).
  - The earlier run at b852dec was also green: e2e 231 passed, `scripts/dev ci` exit 0.
- **Independent reviews** (fresh read-only sessions, skill `om-code-review`):
  - Round 1, b852dec: APPROVE. Minors 1, 2, 3 and 6 were fixed test-first; 4 and 5 are recorded
    under Decisions.
  - Round 2, 165d35d: APPROVE, with two minors and a comment nit, all fixed.
    - An older answer no longer restores the status. Fixed test-first: `queued-messages.e2e.ts:297`.
    - A superseded read that fails no longer warns. This one has no automated test: it needs two
      reads in flight at once, and the mock holds only one. The round-1 test
      `an older read is still applied when the newer one fails` checks that the latest read's
      failure still warns.
  - Round 3, e48cdaf: APPROVE. Its minor about step counts is recorded under Decisions; its
    wording findings are fixed in this section. A confirmation pass on this documentation-only
    change approved the final head.
- **Issues.**
  - Closed with comments: #242, #253, #301, #308, #280, #282 and #44.
  - #325, #323, #310, #288, #316, #324 and #259 are closed after the merge.
  - Handed to the chained tasks listed in Context, which close their own.
  - Left open (not actionable here): #5, #6, #7, #11, #14, #15 and #321.
- **Not tested here:** Safari and Firefox, which have no EditContext; the rule there is the
  textarea case, already covered.
