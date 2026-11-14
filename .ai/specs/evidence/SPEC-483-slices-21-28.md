# SPEC-483: evidence of slices 21 to 28

Moved here, word for word, from the Evidence section of
[SPEC-483](../SPEC-483-deep-security-and-quality-audit-fix-ever.md) when that file neared its
size bound. Paths like `target/audit/sliceN/` name ignored scratch files of the task's
worktree.

### Slice 21 (2026-11-14)

- **Tests failing before the fix** (`target/audit/slice21/before.log`): with a person's
  keystroke from before the refusal the terminal never said that the session was left to
  them (the compaction was typed), and no message could be tried between `/compact` and
  its Enter (nothing marked that moment).
- **Tests failing without their fix** (`target/audit/slice21/mutate.py`, `mutations.log`, 4
  cases, each failed its test): a draft from before the refusal not keeping `/compact`
  out; `/compact` not holding the typing mark; the goal's typist not holding it; a
  message not asking for it.
- **Gate.** `scripts/dev ci` at `3d4c7877d`: exit 0;
  `4337 tests run: 4337 passed (1 slow), 17 skipped` (`target/audit/slice21/gate.log`);
  doctor, repository check, lint, the macOS compile, cargo-deny, cargo-shear and rustdoc
  passed.
- **Web UI.** Nothing under `web/` or `e2e/` changed; the e2e suite was not run.
- **Security scan.** Vidoc is switched off for this project in Yulius: disabled, not passed.
- **Not tested.** A real Claude Code prompt box with a real draft (the test records a
  keystroke; the fake agent has no box); the line that wakes a compacted session with a
  draft in the box (the same function and rule as `/compact`); a typing mark that stays
  set for more than 5 s (Yulius's line is then typed all the same).
- **Review, round 1** (a fresh session, read-only, head `114041dfa`): request changes. Two
  major findings in this slice's code. The gate counted any key since the last prompt
  sent from the box, so "a person who once answered an AskUserQuestion dialog or pressed
  Ctrl-O to read the transcript makes every later full context stop the task", with one
  line in the terminal and no flag: the compaction is held now, flagged, and typed when
  the hold ends (TYP-001 as it reads above; the paragraphs before this one describe the
  first version). And "The only test of the new gate runs the fake agent without hooks",
  where a prompt never empties the box: the tests type through the task's terminal with
  an agent whose prompts reach the hooks, and the usual case (a person's own prompt is
  refused) is shown to compact (AC-118, AC-120, AC-121). Two minor: nothing was asked
  again after the wait for the typing mark (asked again with the mark held, AC-123); the
  restart that waits for the transcript's replay had no test (AC-122). One from before:
  between the goal's tries the mark was free while the line could sit in the box (the
  goal keeps it until it is taken, AC-119). Nits: the documents say which rule each
  typist asks and which typists hold the mark; the comments say what they cover.
- **Tests failing before those fixes** (`target/audit/slice21/before2.log`): with the hooks
  on, the person's own refused prompt was compacted already (the first version was right
  there, and untested), and the two tests of the hold failed for want of a flag.
- **Tests failing without their fix, after the review** (`mutations2.log`, 14 cases, each
  failed its test): a possible draft not holding the compaction; its person not told;
  the flag staying when they type; the hold lasting for good; the flag of a hold that is
  over staying; the wake-up typed behind a draft; a key before the typing mark not asked
  for again, for `/compact` and for the goal; `/compact`, a message and the goal each
  without the mark; the goal giving the mark up after its Enter; a goal kept out at its
  first try saying nothing; the restart of a reattached run not waiting for the replay.
- **The suites after it:** `2771 tests run: 2771 passed (1 slow), 17 skipped`
  (`target/audit/slice21/suite2.log`), before the last three tests were added; the gate
  ran after them.
- **Gate after it.** `scripts/dev ci` at `3cc050727`: exit 0;
  `4344 tests run: 4344 passed (1 slow), 17 skipped` (`target/audit/slice21/gate2.log`).
- **Not tested, after it.** A real Claude Code prompt box (keys are typed through the
  task's terminal into a fake agent that has none); what Claude Code does with `/compact`
  typed behind a real draft once the hold is over; the ten minutes themselves (the hold
  is 3 s in the test); a typing mark that stays set for more than 5 s.
- **Review, round 2** (the same session, head `99aed011e`): request changes; the findings
  of round 1 but the first were fixed, and the hold that answered the first brought new
  ones. One major: after the hold the command goes out behind real text as a prompt and
  is refused, so the flag's and the notice's promise of a compaction was untrue where it
  mattered (the texts say what happens now, and that refusal fails the compaction at
  once: AC-121). Three minor: "The hold's flag outlives the hold" on every way out but
  two, and a restart of Yulius took a held compaction for one that gave up (the flag ends
  with the hold, and the new server takes the hold up again: AC-125); the wake-up was
  kept out behind a draft without a word, after the terminal had said that the task goes
  on (the terminal says that the session was not told to carry on; the first line no
  longer promises: AC-124); the test of the hold read its notice in a race and did not
  assert one it waited for (both asserted through one helper; the time the tests assume
  is named). A nit: the hold's end is logged once. Found on the way: the key the hold was
  for kept the wake-up out after the compaction it had waited for, since nothing empties
  the box for a command; Yulius's own Enter does now.
- **Tests failing without their fix, after round 2** (`mutations3.log`, 19 cases, each
  failed its test): the fourteen above as they are now, and: a key from before Yulius's
  own Enter still counting as a draft; a command refused behind a draft waiting for the
  compaction's time; the episode not told that the command went behind a draft; a hold
  cut by a restart counted as given up; a wake-up kept out saying nothing.
- **Gate after it.** `scripts/dev ci` at `13f445e3c`: exit 0;
  `4346 tests run: 4346 passed (1 slow), 17 skipped` (`target/audit/slice21/gate3.log`).
- **Not tested, after it.** A real Claude Code (what it does with a command typed behind
  real text is taken from how it reads slash commands, at the start of the input only);
  the hold's ten minutes; a run that ends, or a terminal that never goes quiet, during a
  hold (the flag's end there is the same line as when its person types).
- **Review, round 3** (the same session, head `f687673ac`): request changes. Every finding
  of round 2 fixed; one minor, "a regression of the R2-2 fix": the flag of a hold was
  ended before the command only when the hold had run out, not when the box was emptied
  without a key. It ends whenever the hold's flag is up now (AC-126). The reviewer's own
  test for it (the text sent by an Enter nobody typed) could not fail: that prompt starts
  a turn, which ends every flag. The test empties the box by a command's prompt hook
  instead, which starts none, and the mutation fails it. Nits: the comment and TYP-001
  say that Claude Code may run no prompt hook for a command (2.1.292 ran none for
  `/compact`); the comment names the Enter a TUI may drop.
- **Tests failing without their fix, after round 3** (`mutations4.log`, 20 cases, each
  failed its test): the nineteen above, and a hold whose box was emptied without a key
  keeping its flag while Yulius compacts.
- **Gate after it.** `scripts/dev ci` at `26c79dcba`: exit 0;
  `4347 tests run: 4347 passed (1 slow), 17 skipped` (`target/audit/slice21/gate4.log`).
- **Review, fourth and fifth look** (the same session): approve at `b6f1b8d77`
  ("Nothing of severity minor or above in own code remains"), and at `ee6bc62aa` after the
  evidence of slices 1 to 20 moved to its own file (Yulius refused a spec above 256 KiB).
- **Delivery.** Pull request #1276 merged into `dev` at `6c8663268` (the approved head
  with `dev` merged into it by Yulius for its merge train), whose own Yulius CI run
  passed.

### Slice 22 (2026-11-14)

- **Tests failing before the fix** (the five new tests and the changed `installed` test,
  run against a stub that answered
  nothing for the helper's service): a silent helper whose service still ran read
  `lost`; `FileMoveHelper` asked nothing of systemd and could not name the service; a
  mark of bytes that are not text left the task `stopped (server lost)`; a second close
  moved the close time from 200 to 300; a version-57 database stayed at 57.
- **Tests failing without their fix** (`target/audit/slice22/mutate.py`,
  `mutations-r1.log`, 12 cases, each failed its test): a silent report lost whatever
  systemd says; systemd's silence taken for an ended service; a silent report never
  lost; systemd asked of every report; a lost move that cannot be continued; an
  `activating` oneshot taken for ended; a `failed` one for unknown; the path unit asked
  instead of the service; a report that ended between the two reads read lost; a mark
  that is not text taken for no mark; a continued task that keeps its close time; a
  repeated close that moves it.
- **Gate.** `scripts/dev ci` at `3f22df324`: exit 0; `4369 tests run: 4369 passed (1
  slow), 17 skipped` (`target/audit/slice22/gate.log`); doctor, repository check, lint,
  the macOS compile, cargo-deny, cargo-shear and rustdoc passed.
- **Review, round 1** (a fresh session, head `3f22df324`): approve, with four nits in own
  code, answered in `0011416f5`: a test's doc comment had moved above the new test (back
  in place); "the report is read before systemd is asked", so a helper that writes its
  last report and ends between the two reads read lost for one read (read again once
  systemd says ended; the test's new case fails without it); a line of
  `docs/operations.md` over the wrap and its logs paragraph still saying "after a
  minute" (both said as MOV-001 says). The fourth, "the create-time branch … has no
  test", is left: no caller creates a task chained or prepared without a start.
- **Gate after it.** `scripts/dev ci` at `0011416f5`: exit 0; `4369 tests run: 4369
  passed (1 slow), 17 skipped` (`target/audit/slice22/gate2.log`).
- **Security scan.** Vidoc is switched off for this project in Yulius: disabled, not passed.
- **Not tested.** A real systemd (the tests answer as `systemctl show` does, with a fake
  `systemctl`); a real copy that holds the helper's report back for a minute.
- **Review, round 2** (the same session, head `e417e1c6c`): approve; two nits in the spec
  ("the diff adds five tests", and a line of MOV-001 over the wrap), answered in slice 23.
- **Delivery.** Pull request #1277 merged into `dev` at `fb9f12a60`, the approved head
  `e417e1c6c` squashed, whose own Yulius CI run passed.

### Slice 23 (2026-11-14)

- **Tests failing before the fix**: the domain rule found a paused task with a permission
  prompt NeedsYou; the sidebar's counter counted the awaited onboarding task working and
  showed an armed achieved goal "Done"; with the server's group fixed, a task chained after
  an armed achieved goal was still started (`task_is_done`, a second copy of the Done rule);
  the live document after a 403 kept "abcd", busy and unsaved, a close waited, and a new
  join stayed read-only; the Editor never said the refusal.
- **Tests failing without their fix** (`target/audit/slice23/mutate.py`; `mutations.log`,
  `mutations-local.log`, after the review `mutations-r1.log` and `mutations-r1-join.log`;
  each case failed its test): the pause, the awaited onboarding task and the armed merge
  taken out of the rule and, each, out of what the server reads; the armed merge out of
  both Done functions (the local one is what a supertask's wait asks: its test was added
  when this case survived) and out of the task's view; the sidebar's three changes; and in
  the live document, the refusal keeping the typing, any 403 taken for lost access, the
  tab's text left as typed, the tab strip not told, typing after a refusal kept, a join not
  clearing the refusal, a join clearing it before it is answered (its test was added when
  this case survived), the Editor not saying it.
- **Gate.** `scripts/dev ci` at `969a33a91`: one failure,
  `remote_mcp::a_persons_agent_edits_orders_and_stops_tasks_remotely` (a queued task the
  queue started before the test's bulk stop, under load; 6 of 6 alone; filed as #1278: a
  race in that test, which this slice does not touch); rerun at the same head: exit 0,
  `4371 tests run: 4371 passed (1 slow), 17 skipped` (`target/audit/slice23/gate2.log`).
- **Review, round 1** (a fresh session, head `969a33a91`): request changes. One major in own
  code: "After a refused edit the tab strip keeps the unsaved mark", and the Editor test's
  "status does not contain Unsaved" could not fail (the status says read-only instead).
  Two minor: "Any 403 is taken as lost write access, but the server also answers 403 for
  csrf_invalid"; and a refused catch-up after the refusal rejoins every second, with
  `readOnly` false during each attempt. All three answered: the typing is given up in the tab
  (its text the document's as it had it, the tab strip told), only `forbidden` is a refusal
  of access, a refused catch-up asks no more, and only a successful join clears read-only
  (LVE-001, AC-134; the Editor test asserts the tab `notes.txt×`). Nits answered: the mock's
  summary carries `merge_armed`; the counts read the pause without building its view
  (`App::is_paused`); `docs/api.md` says a run paused by a person or an update never needs
  you; doc lines rewrapped; GRP-003 names how it narrows SPEC-487 CHN-003; the observation
  that "after all current tasks" now names the awaited onboarding task is in the Decisions.
- **Gate after it.** `scripts/dev ci` at `d72d28945`: exit 0; `4371 tests run: 4371 passed
  (1 slow), 17 skipped` (`target/audit/slice23/gate3.log`). e2e `live-editor.e2e.ts`,
  `editor.e2e.ts` and `task-done.e2e.ts` at the same head: 71 passed
  (`target/audit/slice23/e2e.log`).
- **Security scan.** Vidoc is switched off for this project in Yulius: disabled, not passed.
- **Not tested.** A real server refusing an edit after a worktree's owner changed (the mock
  refuses on a switch; the server's 403 for that case is `forbidden`, as the mock's); the
  counts with a real update's hold (the domain test and a person's pause cover the fact).
- **Review, round 2** (the same session, head `f9c68ec17`): approve; two nits, answered in
  slice 24: a doc line of the group rule over the wrap, and a refused rejoin retried every
  second (pre-existing, W8).
- **Delivery.** Pull request #1279 merged into `dev` at `d6fe5d400`, the approved head
  `f9c68ec17` squashed, whose own Yulius CI run passed.

### Slice 24 (2026-11-14)

- **Tests failing before the fix**: the proxied document came back without the app's
  `X-Frame-Options`, with the bridge script appended and no length, and the app saw no
  `Accept-Encoding` for a document; a refused rejoin was asked again every
  second (more than the two joins expected in 2.5 s; the count is not in the log). After the
  review: a 409 edit whose rejoin was refused left the tab busy, unsaved, with "abcd" and a
  close that waited.
- **Tests failing without their fix** (`target/audit/slice24/mutate.py`, `mutations.log`, then
  `mutations-r1.log`; each case failed its test): documents fetched uncompressed again; the
  app's `X-Frame-Options` removed again; no `frame-ancestors` of Yulius's own; the app's
  `frame-ancestors` dropped again; a refused rejoin retried every second; after the review,
  a script or the Yulius origin back on the proxy's own pages, and a forbidden rejoin that
  only marks the document read-only.
- **Gate.** `scripts/dev ci` at `91f197b4e`: exit 0; `4371 tests run: 4371 passed (1 slow),
  17 skipped` (`target/audit/slice24/gate.log`). The whole e2e suite at the same head: `84
  passed | 1 skipped` files, `1026 passed | 1 skipped` tests (`target/audit/slice24/e2e-all.log`;
  it covers the deleted web code: nothing imported it).
- **Review, round 1** (a fresh session, head `91f197b4e`): request changes. One major in own
  code: a rejoin refused as `forbidden` "sets only `readOnly = true; onState()`, leaving this
  tab's unconfirmed typing (`outstanding`/`buffer`), its timers and its text in place", the
  W3 state again behind a read-only banner, which the `ended` case could not reach. Two
  minor: PRV-001's "the proxy's own pages carry no script and the same framing rule" had no
  test; SPEC-002 (ADR-14's framing and bridge paragraphs) and SPEC-001 AC-BRW-7 still
  described the old proxy. Nits: `docs/operations.md` justified same-site preview origins by
  "the panel's frames" and the web README suggested `frame-src` for previews (pre-existing);
  a line of `docs/api.md` over the wrap and a short line left in the group rule's doc; the
  preview test's name. All answered in `92c51654d`: the
  forbidden rejoin calls `refused()` (the `gone` case), the landing's page is asserted, the
  two specs, operations and the README say what the proxy does now, the lines are rewrapped
  and the test renamed.
- **Gate after it.** `scripts/dev ci` at `92c51654d`: exit 0; `4371 tests run: 4371 passed
  (1 slow), 17 skipped` (`target/audit/slice24/gate2.log`); e2e `live-editor.e2e.ts` and
  `editor.e2e.ts` at the same head: 49 passed (`target/audit/slice24/e2e-r1.log`).
- **Security scan.** Vidoc is switched off for this project in Yulius: disabled, not passed.
- **Not tested.** A real browser on a real preview (the proxy is tested over HTTP with a
  fixture app; framing and compression are header and body checks); a real GitHub manifest
  answer (the fake answers the field, as GitHub does, and it parses).
- **Review, round 2** (the same session, head `3eddcf9eb`): approve; four nits, answered in
  slice 25: two sentences of this Evidence (a join count no log records, and "two lines over
  the wrap"), the proxy page's criterion in AC-136 instead of AC-135, and SPEC-002's "Cookies
  in iframes" still arguing from the frames (pre-existing).
- **Delivery.** Pull request #1281 merged into `dev` at `3864dc11b`, the approved head
  `3eddcf9eb` squashed, whose own Yulius CI run passed.

### Slice 25 (2026-11-14)

- **No new test** (DED-001). In code and tests (`crates/`, `e2e/`, `Cargo.*`): 44 files, 840
  lines out and 159 in, against `3864dc11b` (slice 24 merged).
- **Gate.** The first run failed at `cargo fmt --check` (a blank line the removed imports
  left) and the second at `cargo doc` (two links to removed import paths and a public doc
  linking the now-private `slug_or`); both fixed. `scripts/dev ci` at `b8ce13c3b`: exit 0;
  `4369 tests run: 4369 passed (1 slow), 17 skipped` (`target/audit/slice25/gate.log`; two
  fewer than slice 24: the deleted tests of the workflow shim, and the tests that only
  exercised a deleted item were folded into others); clippy, cargo-shear, cargo-deny and the
  repository check passed.
- **The whole e2e suite** at `b8ce13c3b`: one failure,
  `navigation.e2e.ts` "a sandbox tab follows the pointer…": the test renamed a sandbox by
  pushing a `sandbox.updated` event, which the server never sends and whose handler this slice
  removed. The test now pushes `resync`, as the server does, and waits for the page to read
  the sandboxes again; the drag-time check (SMO-004) holds on that real path: 83 files
  passed, 1 skipped; 1025 tests passed (`target/audit/slice25/e2e-all.log`); then, at
  `fbc08fa12` (the test's fix), `navigation.e2e.ts` and `home.e2e.ts`: 15 passed (`e2e-r.log`).
- **Review, round 1** (a fresh session, head `f2874b71d`): request changes. One major in own
  code: a second e2e test, "a sandbox tab dragged past either end of the row stops at its
  border", still renamed a sandbox with the removed `sandbox.updated`, so it passed only when an
  unrelated reload of the sandboxes came after the rename. Minor: the new module doc of
  `task.rs` said the supervisor writes a run's statuses itself, while it takes every change but
  two from `apply` (the Decisions said the same); "`TokenUsage::is_unknown` … is read by the
  harness" (tests only); SPEC-002 and SPEC-001 AUT-006 and AC-AUT-5 still described the
  removed retirement, and `docs/operations.md` and DED-001 said a leftover "Feature" row could be
  deleted in Settings, while the default answering to its id hides it. Nits: an e2e comment,
  four lines over the wrap, a store test's name, the observe tests' comparison. All answered in
  `cf89b680d`: the test pushes `resync`; the module doc and the Decisions say which changes go
  through the table; `is_unknown` is gone; the specs and operations say what an instance keeps
  (a hidden `wf-feature`, a listed `wf-quick-task`).
- **Gate after it.** `scripts/dev ci` at `cf89b680d`: exit 0; `4369 tests run: 4369 passed (1
  slow), 17 skipped` (`target/audit/slice25/gate2.log`). The whole e2e suite at the same head:
  84 files passed, 1 skipped; 1026 tests passed (`target/audit/slice25/e2e-all2.log`).
- **Security scan.** Vidoc is switched off for this project in Yulius: disabled, not passed.
- **Not tested.** An instance last started before 2026-10-11 (what it keeps of its two old
  workflows is read from the catalog's rule, as `docs/operations.md` says).
- **Review, round 2** (the same session, head `3706b94f8`): approve; three nits, answered in
  slice 26: a line of slice 24's Evidence still over the wrap, a test line over it, and the
  task module's doc missing the one switch outcome that goes through the table.
- **Delivery.** Pull request #1282 merged into `dev` at `debab9992` (by its merge train), the
  approved head `3706b94f8` squashed, whose own Yulius CI run passed.

### Slice 26 (2026-11-14)

- **Tests failing before the fix**: the request file named "**Never promote to `main`**", which
  `production_instruction` reads as an instruction to promote; the merge news said "Do not
  promote unless the task asks for it"; the Chore workflow named neither `cd_deploy` nor
  "production is a person's"; the Docs workflow said "no independent review"; the guard's
  refusal said "does not end your goal's delivery"; the Git page's sentence said agents
  release.
- **Tests failing without their fix** (`target/audit/slice26/mutate.py`, `mutations.log`, 7
  cases, each failed its test): each old text brought back (the goal's sentence, the
  definition of done's bullet, the merge news, the Chore and Docs workflows' lines, the guard's
  reason, the Git page's sentence).
- **Gate.** The first run failed at the repository check: the new auto-promote sentence named
  the product in a string (`BRAND.name` instead). `scripts/dev ci` at `87f1f76be`: exit 0;
  `4373 tests run: 4373 passed (1 slow), 17 skipped` (`target/audit/slice26/gate.log`; four
  more than slice 25 came with `dev`). e2e `git.e2e.ts`, `cd.e2e.ts` and `workflows.e2e.ts`:
  33 passed (`target/audit/slice26/e2e.log`, before the brand fix; the run of `git.e2e.ts`
  after it, 6 passed, kept no log: the run after the review below has one).
- **Security scan.** Vidoc is switched off for this project in Yulius: disabled, not passed.
- **Review, round 1** (a fresh session, head `589eaf0ac`): request changes. Blocking, in own
  code: the skill offered `Fixes #N` as a way to close an issue, but "GitHub honours closing
  keywords only for a pull request into the repository's default branch": on a `dev` before
  `main` project the issue stays open and the open-issues automation may start a second task
  on it. Non-blocking: the `product.md` exception garbled its sentence and said production
  where only promotion is automatic; the new texts told an agent of an `auto_promote` project
  to ask a person for a promotion Yulius does itself. Nits: the delivery line's comment still
  said agents release; "deploy" was dropped with "release" from the UI copy; an environment
  run gets the stage branch's files without Git, not a checkout; the guard's reason left out
  the merge's own checks; wraps, the spec's criteria order, a run with no log; the mock still
  answered a store failure with 503 `unavailable`. All answered in `8bfd4f812`: the skill says
  `Fixes #N` works only on the default branch and to close the issue by hand otherwise;
  `product.md` says Yulius promotes (not deploys production) where the profile says so, after
  the integration environments; the texts say promotion is "not yours" (a person's, or
  automatic where the profile says so); the UI copy says agents merge and deploy the
  integration environments; the rest as reported.
- **Gate after it.** `scripts/dev ci` at `8bfd4f812`: exit 0; `4373 tests run: 4373 passed (1
  slow), 17 skipped` (`target/audit/slice26/gate2.log`); the mutation cases, their old texts
  updated, all failed their tests again (`mutations-r1.log`); e2e `git.e2e.ts` and `cd.e2e.ts`:
  32 passed (`e2e-r1.log`).
- **Review, round 2** (the same session, head `1c5877e7d`): approve, with four nits (left for
  slice 27). It asked for the e2e files that use the mock's changed store failures:
  `production-protection.e2e.ts` and `vidoc-switch.e2e.ts`, 26 passed
  (`target/audit/slice26/e2e-r2.log`).
- **CI at `1c5877e7d`** (run `9c6a7aaa`): `quality` failed in a test this slice does not touch,
  `usage_credits::a_request_at_a_limit_nothing_says_the_end_of_ends_when_a_restart_retries_the_task`
  (expected the audit "Yulius waits for a subscription…", found none); 8 of 8 alone passed. The
  cause is in the test: `end_limit_requests` ends the request in one write and audits why in
  the next, and six reads of the audit in `usage_credits.rs` came right after waiting for the
  end. With a 300 ms delay before the audit write two tests failed
  (`flake-probe-before.log`); with the reads waiting for the audit (`resolved_soon`, bounded
  at 10 s) all 23 passed with the delay in (`flake-probe-after.log`). Fixed in `14a5821c3`.
  (The probe reproduced two of the six reads; the other four have the same order. The "8 of
  8 alone" runs kept no log.)
- **Gate after it.** `scripts/dev ci` at `14a5821c3`: exit 0; `4373 tests run: 4373 passed (1
  slow), 17 skipped` (`target/audit/slice26/gate3.log`).
- **Review, round 3** (the same session, head `1cec0323b`): approve; three nits answered in
  slice 27 (the helpers' doc comments, naming `stalled_request_resolved`, the Evidence's
  reproduction note above).
- **Delivery.** CI at `1cec0323b` passed; pull request #1283 merged into `dev` at `8a7fc3955`,
  the approved head squashed.
- **Not tested.** An agent's reading of the new texts (the tests read the texts, not a model's
  behaviour).

### Slice 27 (2026-11-14)

- **Tests failing before the fix**: the spec template (old head, old sections) failed
  `validate_spec` with the missing frontmatter and six missing sections; the check's unit test
  failed for want of `delivery_instruction_errors`; the Git page's sentences named "the
  integration environments" a profile without any lacks.
- **Tests failing without their fix** (`target/audit/slice27/mutate.py`, `mutations.log`, 6
  cases, each failed its test): the template's old head; the template without "Test plan"; the
  check letting `gh pr merge` or a self-merge through; `git.md` telling to merge by hand again
  (the repository check refuses it, which also shows the check runs); the Git page naming
  environments it lacks.
- **Generated skill copies.** `apm install` (apm-cli 0.35.1, in a virtual environment) refused
  to overwrite the old copies it had no record of; they were moved aside
  (`target/audit/slice27/apm-aside/`) and installed again; only the four content hashes of
  `apm.lock.yaml` were kept (the newer apm also renamed a target).
- **Gate.** `scripts/dev ci` at `af88f6eb1`: exit 0; `4374 tests run: 4374 passed (1 slow), 17
  skipped` (`target/audit/slice27/gate.log`); e2e `git.e2e.ts` and `cd.e2e.ts`: 32 passed
  (`e2e.log`).
- **Security scan.** Vidoc is switched off for this project in Yulius: disabled, not passed.
- **Review, round 1** (a fresh session, head `8b8665fff`): request changes. Two major, own
  text: the `--chromium-no-sandbox` row said the browser "is then contained only by Yulius's
  sandbox", while the sandbox browsers run on the host as the service user, outside every
  project's sandbox; `git.md` said `repo_hygiene` and `repo_cleanup` remove review worktrees
  and scratch files, which they never do. Minor: "fix/revert through `dev` and promote" was
  left, and the cleanup steps told an agent to remove its task worktree by hand (Yulius's);
  the check missed the very wording of the old AGENTS.md and pull request template ("Author
  self-review + passing checks → squash PR to dev", "squash merge by its author", "happens
  only when a task asks for a release"), refused "Yulius squash-merges …" and "Do not
  promote …", let any line with "refused" through, and skipped catalog automations, skill
  references and templates; `apm.lock.yaml`'s legacy map kept the old hashes; SPEC-001
  GIT-001 and AC-UI-18. Nits: a clean dev merge keeps the approval, the CD deploy is refused
  until the checks pass, `[commands]` and the briefing, wraps, the check's row and summary,
  `spec_check` in the skill, chores in AGENTS.md. All answered in `19a660b6e`.
- **Gate after it.** `scripts/dev ci` at `19a660b6e`: exit 0; `4374 tests run: 4374 passed (1
  slow), 17 skipped` (`target/audit/slice27/gate2.log`); the mutation cases, the check's
  updated (`mutations-r1.log`, 9 cases; "any line that says refused" was first written so
  that it could not fail and is rerun as the old behaviour), all failed their tests.
- **Review, round 2** (the same session, head `3d6240361`): approve, with two minor findings in
  own code, fixed here rather than deferred (ADR-0141): the self-merge pattern no longer caught
  the old `git.md` wording "squash-merges their own PR", and the rule read one line at a time,
  so the old AGENTS.md promotion sentence, wrapped over two lines, passed. Nits: two legitimate
  sentences refused, a greedy `gh pr checks` pattern, an exemption not tied to its match, the
  Decisions' "four content hashes", wraps, no test of the scanned files. All answered in
  `ed3193394`: the rule reads paragraphs and list items; the old AGENTS.md, `git.md`, pull
  request template, `yulius-git` skill and `workflow.md` from `8a7fc3955` are all refused by
  it; a match that says the rule ("not", "never", "a person's") is not refused; the scanned
  files have a test.
- **Gate after it.** `scripts/dev ci` at `ed3193394`: exit 0; `4374 tests run: 4374 passed (1
  slow), 17 skipped` (`target/audit/slice27/gate3.log`); the mutation cases
  (`mutations-r2.log`, 11) all failed their tests (the paragraph case was first written to
  change only the first of the two places that join a paragraph, and is rerun on the other).
- **Review, round 3** (the same session, head `bed1d9ee2`): approve, with one minor finding in
  own code (R3-1: a heading, a table row and a fence joined the text around them) and one nit
  (R3-2: an exemption by "not" anywhere in a match, within the decision above). The merge was
  armed at the approved head; R3-1 is fixed in slice 28, the first change after it, and R3-2
  stays as the decision's known limit.
- **Not tested.** An agent following the rewritten instructions (the check reads the wording).

### Slice 28 (2026-11-14)

- **Tests failing without their fix** (`target/audit/slice28/mutate.py`, `mutations.log`, 12
  cases): Back and Forward not guarded, Back moving before it asks, a navigation by code not
  guarded, Leave going nowhere, a link not guarded (`places.e2e.ts`), the old "checks failed",
  unknown checks without why, a merged chip saying its checks, sizes over a gibibyte in
  megabytes, "unknown" for no commit, headings, tables and fences joined: each failed its test.
  "A fence's lines join" survived (no case had two lines inside a fence that read as a refusal
  only together); a case was added and it fails now (`mutations-b.log`).
- **Gate.** `scripts/dev ci` at `9e4e1e5d9`: exit 0; `4374 tests run: 4374 passed (1 slow), 17
  skipped` (`target/audit/slice28/gate.log`); e2e of the editors, places, navigation, pull
  requests, Git, file drops, CD, CI and the instance page: 12 files, 148 passed (`e2e.log`).
- **Security scan.** Vidoc is switched off for this project in Yulius: disabled, not passed.
- **Review, round 1** (a fresh session, head `402f4eb31`): request changes. One major, own
  code: a guarded Back or Forward put the address back with a new history entry, so Back,
  Leave, Back landed in the editor again, and after Forward and Cancel a later Back went to the
  page Forward had been declined; the Decisions' claim that the browser offers nothing better
  was wrong. Minor: Leave ended the guard of a page that stays (a way out to its own address);
  `slice(0, 7)` copies were left; the chip test read titles only. Nits: an export nothing
  imports, `limitSize` rounding where the server cuts, a line starting with "#465" read as a
  heading. The reviewer also saw the Git page's review column say "No checks" where it said
  "In review". (Six `slice(0, 7)` copies were left in all, `stages.js` included.) All answered in `b7efd61d8`, `396bd75e1` and `742015de3`: entries carry their
  place and a guarded move is undone and made again (Decisions); writing it, the move from the
  tab's first entry (entry 0) was taken for no move, and the second event of a move in hash mode
  was not tested: both are now.
- **Gate after it.** `scripts/dev ci` at `742015de3`: exit 0; `4374 tests run: 4374 passed (1
  slow), 17 skipped` (`gate2.log`); e2e of 16 files (the editors, places, navigation, pull
  requests, Git, file drops, CD, CI, editor, sandbox Docker, shortcuts, loading shapes): 206
  passed (`e2e2.log`); the mutation cases (`mutations-r1.log`, 21) all failed their tests (a
  flag that told the router's own moves apart survived: `emit` already ignores a return to the
  address in view, so the flag went, and the check that lets a move's own events by got a test).
- **Review, round 2** (the same session, head `2a7ac2395`): approve, with two nits: N1, own
  code, an address typed over unsaved work in hash mode left an unlabelled entry, so a later
  move across it was miscounted and, after a Cancel, Back and Forward were ignored until the
  next navigation; N2, the count of `slice(0, 7)` copies above. The merge was armed.
- **CI at `2a7ac2395`** (run `60e1d04b`): `e2e` failed, `responsive.e2e.ts` "every top-level page
  is centred": the test opens each page by its address, and the automation editor, left before
  it had loaded, counted its empty baseline as a change, which the guard now asks about on a
  typed address (before, a link click while it loaded asked too). Reproduced locally; the
  automation and MCP editors have nothing unsaved until they have loaded (the workflow editor
  already had none), tested on all five editors. N1 is fixed with it (its test failed first),
  N2 in the Evidence above, in `ebaff95d1`.
- **Gate after it.** `scripts/dev ci` at `ebaff95d1`: exit 0; `4374 tests run: 4374 passed (1
  slow), 17 skipped` (`gate3.log`); the whole e2e suite: 85 files, 1032 passed, 1 skipped
  (`e2e-full.log`); the new mutation cases (`mutations-ci.log`: a typed entry left unlabelled,
  a loading automation or MCP server counted as unsaved) failed their tests.
- **Review, round 3** (the same session, head `d2c1a673f`): request changes. Both fixes right
  and tested; one major in own code that rounds 1 and 2 missed: replacing the automation
  editor's own guard dropped its mount-time `renderStart()`, so reached from a page whose tasks
  were already read, the start chip was an empty, nameless button until the automation loaded,
  not the shimmering "Start" (ADR-0073); the existing test passed only through a first-load
  `tasks` event. Its whole-suite run failed once in `menus.e2e.ts` and passed alone three times
  (#1217, a known load flake). Answered in `abdcb1bc0`, test first (`mutations-r3.log`). Past the
  third round the finding is fixed rather than deferred (ADR-0141); the other four pages' removed
  lines were compared again: only their guards went.
- **Gate after it.** `scripts/dev ci` at `abdcb1bc0`: exit 0; `4374 tests run: 4374 passed (1
  slow), 17 skipped` (`gate4.log`); the whole e2e suite: 85 files, 1033 passed, 1 skipped
  (`e2e-full2.log`).
- **Review, round 4** (the same session, head `56b576065`): approve. The fix of N3 fails its
  test without it; a second look over every line the slice removed found nothing else lost
  (the five editors lost only their guards; the chip, line and strip keep their classes,
  handlers, names and icons; every `shortSha` caller gives what it gave).
- **Merged.** The merge armed at `56b576065` waited behind `dev`; its first merge train was
  dissolved when `dev` moved, the second passed, and pull request #1287 was squash-merged into
  `dev` (`fea9402df`, whose own Yulius CI run passed).
