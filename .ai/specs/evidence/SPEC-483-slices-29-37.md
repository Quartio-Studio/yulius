# SPEC-483: evidence of slices 29 to 37

Moved here, word for word, from the Evidence section of
[SPEC-483](../SPEC-483-deep-security-and-quality-audit-fix-ever.md) when that file neared its
size bound. Paths like `target/audit/sliceN/` name ignored scratch files of the task's
worktree. Slice 37's last two entries (its fourth review round and its merge) were written at
the move.

### Slice 29 (2026-11-14)

- **Tests failing before the change**: `one_bearer_reader…` and
  `github_tokens_are_replaced_whole_by_one_scrub` did not compile (no shared function); the
  welcome page test's new refresh case passes before and after (it pins the shared output).
- **Tests failing without their fix** (`target/audit/slice29/mutate.py`, `mutations.log`, 9
  cases, each failed its test): the bearer token untrimmed; any scheme read as a bearer; the
  scrub missing a prefix; `ghs_` not first; a stateless token cut; a bare prefix replaced;
  Git's or GitHub's adapter scrubbing no tokens; the refresh listing nothing. (A first "missing
  prefix" case only reordered two prefixes, which changes nothing, and was rewritten:
  `mutations-b.log`.)
- **Gate.** `scripts/dev ci` at `4359dbbb6`: the first run failed one test,
  `remote_mcp::a_persons_agent_edits_orders_and_stops_tasks_remotely` (#1278, open since slice
  23; it passed alone 5 of 5); the second exited 0, `4382 tests run: 4382 passed (1 slow), 17
  skipped` (`gate.log`, `gate2.log`). Reading the code for it found a product race: each stop of
  a bulk stop ends with `drain_queue()`, so stopping the first queued task lets the queue start
  the next before the bulk reaches it (commented on #1278); it is slice 30.
- **Security scan.** Vidoc is switched off for this project in Yulius: disabled, not passed.
- **Review, round 1** (a fresh session, head `dcafa7c26`): request changes. One minor, own code:
  no route test pinned the one behaviour change (the leftovers key trimmed) nor that the key
  goes through the shared reader. Nits: no test that another `Authorization` scheme beside a
  valid session cookie is refused rather than taken as absent (pre-existing); the moved
  evidence gained slice 28's last two entries, and named the pull request's head where the
  squash commit on `dev` (`35a12fb98`) was meant. All answered: the leftovers test takes a
  padded key, a new test refuses `Basic` beside a cookie, the evidence file says what was added
  at the move. Each new test fails without its behaviour (`mutations-r1.log`: the key read
  untrimmed; the extractor rewritten to fall back to the cookie).
- **Gate after it.** `scripts/dev ci` at `368bd692b`: exit 0; `4383 tests run: 4383 passed (1
  slow), 17 skipped` (`gate3.log`).

### Slice 30 (2026-11-14)

- **Failing before the fix.** A temporary 300 ms sleep after each stop of `bulk_tasks`' loop:
  `remote_mcp::a_persons_agent_edits_orders_and_stops_tasks_remotely` failed 3 of 3 (`done` 1,
  the second task `failed_precondition`); with the fix and the same probe it passed 3 of 3
  (`target/audit/slice30/probe-before.log`, `probe-after.log`); the probe was removed.
- **Stress.** `--stress-count 50` beside a busy loop: 50 of 50 passed (`stress.log`).
- **The supertask's pass is still tested.** Without it, `supertask::stopping_a_queued_supertask_starts_the_tasks_it_held`
  fails (`mutation-no-supertask-drain.log`); the supertask, supervision, queue and remote MCP
  tests: 124 passed.
- **Gate.** `scripts/dev ci` at `8fbd91be5`: exit 0; `4383 tests run: 4383 passed (1 slow), 17
  skipped` (`gate.log`).
- **Security scan.** Vidoc is switched off for this project in Yulius: disabled, not passed.
- **Review, round 1** (a fresh session, head `8731beaaa`): request changes. One major, own code:
  no test failed without the fix: the flaky test passes 50 of 50 with the old line unless the
  host is loaded or the probe is in, so "Stress: 50 of 50" proved nothing about the fix, and the
  Decisions' reason for committing no test was wrong (the harness has no background tick, so a
  bounded absence check cannot fail falsely). The reviewer confirmed the narrowed condition for
  every case (no slot freed, no hold changed, a capacity pause closed by `end_pause`, chained
  successors started by `release_chained`, every event that frees a slot runs its own pass) and
  the corrected claim on #1278. Answered: `supertask::stopping_a_waiting_task_that_is_no_supertask_starts_no_other`
  fails 10 of 10 with the old line and passes 10 of 10 with the fix (`--stress-count 10`,
  `test-old-line.log`, `test-fixed.log`).
- **Gate after it.** `scripts/dev ci` at `0767c877e`: exit 0; `4384 tests run: 4384 passed (1
  slow), 17 skipped` (`gate2.log`).
- **Review, round 2** (the same session, head `047b6f76e`): approve; the new test fails 20 of 20
  on the old line.
- **Merged.** Pull request #1295 was squash-merged into `dev` (`dcc00aa24`) after its merge
  train passed; #1278 closed.

### Slice 31 (2026-11-14)

- **Test failing before the change**: `time::tests::one_rfc3339_reader_takes_the_same_inputs_everywhere`
  did not compile (no reader in the domain). GitHub's test that a fraction was dropped
  (`2024-02-29T12:00:00.123+02:00` read as `…:00.000`) went with its reader; the domain's test
  pins the fraction kept.
- **Tests failing without their fix** (`target/audit/slice31/mutate.py`, `mutations.log`, 10
  cases, each failed its test): an impossible day taken, the fraction dropped, a leap second as
  one more second, no space separator, offset hours unchecked, trailing text taken, the
  formatter's or the egress certificate authority's or analytics' or the app's date wrong.
- **Gate.** `scripts/dev ci` at `5306dd677`: exit 0; `4391 tests run: 4391 passed (1 slow), 17
  skipped` (`gate.log`).
- **Security scan.** Vidoc is switched off for this project in Yulius: disabled, not passed.
- **Review, round 1** (a fresh session, head `fabe6bae1`): approve; a differential run of the
  four old readers against the new one (3 million mutations of real-shaped timestamps) showed
  only malformed inputs read differently, no real writer affected. Four nits, answered in slice
  32: the Decisions' account of the app's and imports' readers (corrected), a test case refused
  by the length check (two full-length unpadded cases added), the formatter's round trip (true
  for four-digit years; the doc says so), the fake's formatter at absurd inputs (no change: no
  caller comes near).
- **Merged.** Pull request #1296 was squash-merged into `dev` (`98eb2af79`) after its merge
  train passed.

### Slice 32 (2026-11-14)

- **Test failing before the change**: `auth::tests::a_device_grant_is_said_as_the_cli_reads_it`
  did not compile (the server's poll had no state type and no mapping).
- **Tests failing without their fix** (`target/audit/slice32/mutate.py`, `mutations.log`): an
  unknown state said as pending, `denied` said as expired, an approved token not given: each
  failed its test. Two survived: the check that a consumed grant gives no token again guards two
  concurrent polls, code this slice moved but did not change, which a sequential test cannot
  reach (a second poll reads the `consumed` state anyway); and dropping the separators at
  positions 7 and 16 of a timestamp, which no case had a wrong character at: four cases with a
  wrong separator were added and the mutation now fails (`mutations-b.log`).
- **Gate.** `scripts/dev ci` at `e36541b04`: exit 0; `4405 tests run: 4405 passed (1 slow), 17
  skipped` (`gate.log`).
- **Security scan.** Vidoc is switched off for this project in Yulius: disabled, not passed.
- **Review, round 1** (a fresh session, head `f3095f36c`): approve, three nits (two own: the two
  "unpadded" cases are refused by the separator check the new separator cases cover; "a
  sequential test cannot reach" the consumed-grant check is too strong, a test store could;
  one pre-existing: `DeviceStart`'s derived `Debug` prints the device code). The merge was armed.
- **CI at `f3095f36c`** (run `ca58bbe9`): `e2e` failed in `ownership.e2e.ts` "Archive all leaves
  other people's Done tasks", a test added by #1292 the same day, which this slice does not
  touch: the toast said "Archived 2 Done tasks.", the other person's task archived too. "Archive
  all" and a row's menu act on the task the page holds; the test changed the task's owner
  through the mock and pressed the button before the page had the change. A 500 ms delay of the
  mock's change (`touchTask`) made it fail 2 of 2 and the menu test of the same file fail too;
  waiting until the row shows its owner's mark before pressing, the file passed 9 of 9 with the
  delay (`probe-file-after.log`; the run without it was not kept). No open pull request touched
  the file; the fix is its own commit (`6287e8220`).
- **Review, round 2** (the same session, head `593fd0e83`): request changes. At a 2.5 s delay a
  third test of the file ("…archived from the sidebar and deleted from its menu") opened task
  #4's menu before #4's owner change (the 500 ms probe hid it behind #13's archive). Answered:
  the same wait before its menu; with a 2.5 s delay the file passes 9 of 9 twice
  (`probe-2500-after.log`), without it 9 of 9 twice (`no-probe-after.log`). Every other
  `touchTask` of the file waits on something the change shows before acting.
- **Review, round 3** (the same session, head `2b6e73abd`): approve.
- **Merged.** Pull request #1297 was merged into `dev` at `2b6e73abd`, its own Yulius CI run
  passed. Nits answered in slice 33.

### Slice 33 (2026-11-14)

- **Tests failing before the change**: the `model` and `effort` test failed on the old schema
  (`sandbox_task_create`'s were bare strings); the redaction test failed on the derived `Debug`
  of `DeviceStart`, then of `DevicePollRequest`; the dispatch test passes before and after (it
  guards the next change).
- **Tests failing without their fix** (`target/audit/slice33/mutate.py`, `mutations.log`): a
  shared tool missing from the dispatch, `task_message` not dispatched remotely, the model
  re-typed, either polling code shown: each failed its test. A version negotiation that ignores
  what the client asked survived: nothing tested the handshake (on `dev` neither); a test was
  added and the mutation fails (`mutations-b.log`).
- **Gate.** `scripts/dev ci` at `d7d01ba5e`: exit 0; `4408 tests run: 4408 passed (1 slow), 17
  skipped` (`gate.log`).
- **Security scan.** Vidoc is switched off for this project in Yulius: disabled, not passed.
- **Review, round 1** (a fresh session, head `efceecb23`): request changes. One minor, own code:
  the new items were inserted between the Superagent instructions' doc comment and
  `COCKPIT_INSTRUCTIONS`, so the const lost its doc (the rule that its text has no room to
  grow) and `model_property` carried it. Nits: the new tests sat between the test module's
  `use` lines; the dispatch test covers a person's token, not the Global Superagent's browser
  tools. The reviewer found every name keeps its arm for every caller, `initialize` the same
  answer, the remote `tools/list` the same set for each scope and `task_create`'s schema the
  same byte for byte. All answered: the doc is back on its const, the tests follow the `use`
  lines, AC-153 and the test's doc say a person's token.
- **Gate after it.** `scripts/dev ci` at `96612dc2b`: exit 0; `4408 tests run: 4408 passed (1
  slow), 17 skipped` (`gate2.log`).
- **Review, round 2** (the same session, head `04a639909`): approve; three cosmetic nits (two
  lines past the width, a missing blank line, the test's reason), answered in slice 34.
- **Merged.** Pull request #1300 was squash-merged into `dev` (`1fa35c08e`) after its merge
  train passed.

### Slice 34 (2026-11-15)

- **Test failing before the change**: `agent_endpoint::tests::the_endpoint_is_plain_http_to_loopback_with_a_printable_token`
  did not compile (no shared reader); the cases the Git and hook parsers disagreed on (a
  non-ASCII path, `localhost`'s addresses) are in it.
- **Tests failing without their fix** (`target/audit/slice34/mutate.py`, `mutations.log`,
  `mutations-b.log`, 9 cases, each failed its test): a non-ASCII path, an empty host or any
  address taken, `localhost` on IPv4 only, a blank token taken, the token shown in `Debug`, the
  path kept with its slash, the Git client or the hook client trying no address.
- **Gate.** `scripts/dev ci` at `cbed41969`: exit 0; `4413 tests run: 4413 passed (1 slow), 17
  skipped` (`gate.log`).
- **Security scan.** Vidoc is switched off for this project in Yulius: disabled, not passed.
- **Review, round 1** (a fresh session, head `490fe6d50`): request changes. The reviewer found
  security holding (the addresses come from the parsed literal and are never resolved again; no
  token in `Debug` or errors; the `Host` header printable and checked), messages and exit codes
  unchanged. Two minors, own code: every address got the whole timeout, so a `localhost` whose
  127.0.0.1 never answers could take twice the bound (and `agent_call` report `Lost` for
  `Unsent`); nothing tested the fallback to the next address. Nits: a stale doc line, the
  error-to-message match twice, the changes entry under the previous day, and (pre-existing in
  both old parsers) `[127.0.0.1]`, an unbracketed `::1:80` and ports `+80` taken. All
  answered: `connect` takes the one deadline and gives each attempt the time left, tests for
  the fallback in both clients (a server on ::1 only) and for a spent deadline,
  `EndpointError::message`, the stricter spelling. Each new test fails without its fix
  (`mutations-r1.log`, 5 cases).
- **Gate after it.** `scripts/dev ci` at `2819ef95b`: exit 0; `4416 tests run: 4416 passed (1
  slow), 17 skipped` (`gate2.log`).
- **Review, round 2** (the same session, head `7d9e26a33`): approve; nits: two spec lines past
  the width, a rare port collision of the new `::1` tests with another test's 127.0.0.1
  listener, and (pre-existing) the hook client's `exchange` giving each address the whole
  timeout. The first two answered in slice 35.
- **Merged.** Pull request #1305 was squash-merged into `dev` (`2123763fb`) after its merge
  train passed.

### Slice 35 (2026-11-15)

- **Test failing before the change**: `tests::the_capacity_views_read_their_window_through_an_index`
  failed on `SCAN a` (`attempts`, every run ever recorded) and then, with the indexes, on
  `SCAN capacity_pauses USING INDEX capacity_pauses_started` (the pauses' `OR` and `ORDER BY`).
- **Tests failing without their fix** (`target/audit/slice35/mutate.py`, `mutations.log`): each
  of the three indexes left out, the pauses' `OR` back: each failed the plan test. Leaving the
  pauses unsorted survived (nothing tested their order); a test was added and the mutation
  fails (`mutations-b.log`).
- **Gate.** `scripts/dev ci` at `ba96b2551`: exit 0; `4426 tests run: 4426 passed (1 slow), 17
  skipped` (`gate.log`).
- **Not measured.** The time a dashboard load takes on a large instance (no such database here);
  the plans say what SQLite reads.
- **Security scan.** Vidoc is switched off for this project in Yulius: disabled, not passed.
- **Review, round 1** (a fresh session, head `83a6e75ce`): request changes. Blocking, own code:
  the new migration was listed at position 60, ahead of `dev`'s schema version 60
  (`0059_task_control_from`, #1301): a database at version 60 would run 0059 again on open,
  fail with "duplicate column name: control_from" and never get the indexes (reproduced by the
  reviewer from `dev`'s list); no test opened a database made with `dev`'s list. Nits: the
  plan test built the pauses' query with another column list than the store's; its doc should
  say it assumes no statistics (nothing runs `ANALYZE`; with statistics and a whole-table
  window SQLite rightly scans); a Test plan line past the width; the `::1` helper narrows the
  port race without closing it (accepted). The reviewer found the rewritten pauses' query the
  same rows in the same order, `token_runs` reading both terms through the index, and each
  query 0.2 to 1.8 ms at 200 000 runs. Answered: the migration is schema version 61 at the
  end, a test pins the latest versions to their files (it fails with the old placement,
  `mutations-r1.log`), the test uses `PAUSE_COLS`, its doc says it. (The Test plan line was
  in fact not wrapped then: review round 2; it is wrapped in slice 36.)
- **Gate after it.** `scripts/dev ci` at `02bafbf68`: exit 0; `4427 tests run: 4427 passed (1
  slow), 17 skipped` (`gate2.log`). The reviewer's reproduction, a database made with `dev`'s
  migration list (version 60) opened by this head, applies only 0061 and gets the three
  indexes (`migorder-after.log`).
- **Review, round 2** (the same session, head `a361e0eaa`): approve; one nit: the Test plan line
  was still past the width though the round 1 entry said it was wrapped (both corrected in slice
  36).
- **Merged.** Pull request #1306 was squash-merged into `dev` (`f2a339e96`) after its merge
  train passed.

### Slice 36 (2026-11-15)

- **Tests failing before the change**: `transcripts::tests::a_read_from_a_resume_point_gives_what_a_read_from_the_start_gives`
  did not compile (no resume point); `task_transcript::a_watcher_s_next_poll_reads_only_what_was_written_since`
  fails while the handler reads from the start (the slipped newline renumbers the new line 5).
- **Tests failing without their fix** (`target/audit/slice36/mutate.py`, `mutations.log`): a
  point past the asked line or off a line's start used, the byte offset not counted, the
  handler reading from the start or keeping no point: each failed its test. The two that drop
  the dotenv calls at the point survived the first test, whose dotenv value is also caught by
  the token patterns: a case with a value only the dotenv rule hides was added and both fail
  (`mutations-b.log`). Not tested on its own: the clear in `reindex_session` (see round 1:
  the file identity is the guard; the reason first given here was wrong).
- **Timing probe** (a test not kept, release build, `probe-timing.log`): a poll at the end of a
  30 MB transcript of 125 830 lines took 497 ms read from the start and 18 µs from the point.
- **Gate.** `scripts/dev ci` at `3c9919d85`: exit 0; `4433 tests run: 4433 passed (1 slow), 17
  skipped` (`gate.log`).
- **Security scan.** Vidoc is switched off for this project in Yulius: disabled, not passed.
- **Review, round 1** (a fresh session, head `7ba75f9e9`): request changes. Minor, own code,
  security-relevant: the point had no file identity, and a step-back renames a rewritten copy
  over the transcript; a poll in flight could keep a point of the old file after
  `reindex_session`'s clear, and once the resumed session grew past it with a newline before
  the byte, the point was used on the new file: the reviewer's probe showed
  `DB_PASSWORD=plainword` where a read from the start redacts it. The Evidence's reason for
  not testing the clear (a step-back shortens the file) held only until the file grew again.
  Nits: an earlier point overwrote a farther one; the byte count of an over-long line had no
  test. The reviewer found the byte counting (complete, over-long, partial last line, CRLF),
  the page and end points, the carried dotenv calls, the bound and the map's sharing right.
  Answered: the point records the file's device and inode (the reviewer's case, a replacing
  file whose dotenv read ends exactly at the old point, failed first with `plainword` shown);
  the farther point is kept; a test of an over-long line. Each new test fails without its fix
  (`mutations-r1.log`, `mutations-r1b.log`, 10 cases, none survives).
- **Gate after it.** `scripts/dev ci` at `6793ba26e`: exit 0; `4435 tests run: 4435 passed (1
  slow), 17 skipped` (`gate2.log`).
- **Review, round 2** (the same session, head `e65afc7d2`): approve; nits: a kept farther point
  stays when an agent shortens its transcript in place (outside the threat model), the
  handler's use of `replaces` has no test of its own, a doubled line in a test. The merge was
  armed.
- **Merge train at `e65afc7d2`** (run `0dfa6dad`): `e2e` failed in `cd.e2e.ts` "a refusal after
  its view was left is a toast; a page left with it open draws nothing", the open flaky test
  #1171, which this slice does not touch. Its cause, found here: the CD page left while a
  deploy is being checked reads its runs when shown again, possibly before the server
  answered, and then waits for its 30 s idle poll, showing the earlier run meanwhile (a
  product race, not only the test's). Made deterministic (the deploy released after the page is
  back and shows the earlier run: it failed with the CI's very message), then fixed: an answer
  that comes after its page went makes the CD page shown for that sandbox read again at once;
  the rollback had the same gap (a hold for rollbacks added to the mock, a test that failed
  first). Separate commit, `Fixes #1171`. With it, the nits: a kept point past a transcript
  shortened in place is replaced (tested), the doubled line removed; the handler's own use of
  `replaces` stays untested (speed only).
- **Gate after it** (`dev` merged in). `scripts/dev ci` at `41117af64`: exit 0; `4438 tests run:
  4438 passed (1 slow), 17 skipped` (`gate3.log`); the whole e2e suite: 85 files, 1049 passed,
  1 skipped (`e2e-full.log`).
- **Review, round 3** (the same session, head `81cb0bc7a`): approve; the #1171 fix checked
  (listener lifetimes, one page per window, another sandbox's answer ignored, the asking page
  unchanged, the loader keeping the latest read; cancel has no 30 s gap, set-up navigates
  away); nits: the handler's use of `replaces` and the sandbox filter have no test of their
  own (speed only, both accepted).
- **Merged.** Pull request #1309 was squash-merged into `dev` (`e1b934573`) after its merge
  train passed; #1171 closed.

### Slice 37 (2026-11-15)

- **Measured before and after** (a temporary counter of store calls and a timer around `GET
  /api/sandboxes/s1/tasks`, release build, tasks a third each running, queued and closed;
  `target/audit/slice37/probe-before.log`, `probe-after.log`, the probe kept as
  `zz_probe37.rs` there): 50 tasks 688 calls, 11 ms before, 671 calls, 9 ms after; 200 tasks
  2 738 calls, 88 ms before, 2 671 calls, 31 ms after.
- **The list against each summary.** The supertask test now compares every queued task's
  `held_by` in the list with its own summary; handing the summaries an empty project or only
  the task itself fails it (`mutations.log`, 2 cases). It passes before the change too: the
  change keeps the answer and drops the re-reads.
- **Gate.** `scripts/dev ci` at `2a1dea047`: exit 0; `4441 tests run: 4441 passed (1 slow), 17
  skipped` (`gate.log`).
- **Security scan.** Vidoc is switched off for this project in Yulius: disabled, not passed.
- **Review, round 1** (a fresh session, head `c36cace24`): request changes. One minor, own text:
  the same re-read stays in the queue's pass (`drain_queue_now` asks `held_by_supertask` for
  every queued task), which runs every 60 s and on events whether anyone looks or not, so the
  ledger's closure was incomplete. Nit: nothing pins that the list hands the project (only the
  read count changes; accepted, the probe measured it). The reviewer found the list the same
  query as before, the order irrelevant (`holders` sorts), archived lists on the old path, the
  probe's saved calls equal to the queued tasks, and the kept linear reads documented.
  Answered: the pass reads each project once (`held_by_supertask_among`); handing it an empty
  project fails `supertask::a_new_supertask_relabels_the_queue_at_the_session_limit`
  (`mutations-r1.log`, 3 cases, none survives); the supertask, queue, supervision and capacity
  tests: 95 passed.
- **Gate after it.** `scripts/dev ci` at `9a0388054`: exit 0; `4441 tests run: 4441 passed (1
  slow), 17 skipped` (`gate2.log`).
- **Review, round 2** (the same session, head `e1227e517`): approve; nits: no test runs one
  pass over two projects, fully qualified `HashMap` where it is imported.
- **Merge train at `e1227e517`** (run `9ef69f03`): `e2e` failed in `sessions.e2e.ts` "the spider
  web sticks to the top of the view while the session scrolls" (`expected 199 to be greater
  than 200`), the open flaky test #1185, which this slice does not touch: the test read the
  scroll range once, on the frame before the entries took their size. A probe that holds the
  entries back 2 s failed it (`expected 0`); waiting on the range (`expect.poll`), it passes
  with the probe and without (`target/audit/slice37/flake/`). The file's other one-shot read
  after the locked box (the edge-to-edge fit) waits the same way. Separate commit,
  `Fixes #1185`.
- **Gate with `dev` merged in** first failed one test, `task_done::a_switch_whose_old_run_had_to_be_killed_starts_no_new_run`
  (slice 17's): the run's end was recorded by its reader (`switch_timeout`) before the switch's
  record (`lost`), an order two sibling tests force each way; this unordered test now accepts
  either (the task's rest is the same), 30 of 30 under `--stress-count 30` beside six busy
  loops (`stress-switch.log`). (The two sibling tests force host-loss-first and own-exit-first;
  neither forces the reader recording a terminated host before the switch, the order behind
  this flake: review round 3.)
- **Gate after it.** `scripts/dev ci` at `9a0517a00`: exit 0; `4442 tests run: 4442 passed (1
  slow), 17 skipped` (`gate4.log`); the whole e2e suite before the switch test's change:
  85 files, 1050 passed (`e2e-full.log`).
- **Review, round 3** (the same session, head `60761a34f`): approve; the two recorders confirmed
  legitimate (the first write wins, both rest the task as `stopped_switch_timeout`, nothing
  branches on which); the e2e waits right (entries use `content-visibility: auto`, so the range
  settles a frame later). Nits: the sibling tests' order described inexactly (corrected above),
  a pre-existing cockroach read on the same race, the changes entry without the queue pass
  (added).
- **Merge train at `60761a34f`** (run `59e92e63`): `quality` failed one test,
  `task_done::a_stop_settles_a_run_no_server_holds`, the open flaky test #1190 (from #1173),
  which this slice does not touch: it checked the stopped host with `kill(pid, 0)`, which a
  process that exited but is not reaped yet still answers. It now uses the file's zombie-aware
  `process_gone`, and `an_exited_process_not_yet_reaped_is_gone` shows on a real unreaped child
  that `kill(0)` answers while `process_gone` says gone. Separate commit, `Fixes #1190`.
- **Gate after it** (`dev` merged in). `scripts/dev ci` at `64e23f4b7`: exit 0; `4446 tests run:
  4446 passed (1 slow), 17 skipped` (`gate5.log`); the whole e2e suite: 85 files, 1050 passed
  (`e2e-full2.log`).
- **Review, round 4** (the same session, head `06adb2fa1`: `#1190`'s fix and `dev` merged in):
  approve. The fix matches the product's own proof of a gone host
  (`ptyhost::process::alive`: `kill(0)` answers and the process is no zombie); the new test is
  sound (the unreaped child polled into Z with a deadline, both children reaped, 15 of 15 under
  stress). N4 (the test's doc wording, `task_done.rs:2926-2928`) stays a wording nit.
- **Merged** as pull request #1313 (squash `03ad7567b` on `dev`), on its own green Yulius CI run;
  #1185 and #1190 closed by hand.
