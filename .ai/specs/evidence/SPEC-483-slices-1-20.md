# SPEC-483: evidence of slices 1 to 20

Moved here, word for word, from the Evidence section of
[SPEC-483](../SPEC-483-deep-security-and-quality-audit-fix-ever.md) when that file reached its
size bound. Paths like `target/audit/sliceN/` name ignored scratch files of the task's
worktree.

### Slice 1 (2026-11-13)

- **The defect, on `dev` d044172cb.** A temporary test asked `/internal/scrub/fingerprints` 28
  times with a run's token, built the 28 equations from `base` and the first needle's `rolling`
  and solved them modulo 2^61 − 1: `recovered from 28 requests: sk_test_51NzQ9vaultValue8f3a`,
  the test's 28-character value.
- **The fix.** `secrets::repeated_fingerprint_requests_answer_the_same_set` (which replaced that
  test) passes: equal answers, no `base`, no `rolling`.
- **The old wrapper, for real.** In a checkout of `dev` d044172cb, a temporary test of
  `yulius-harness` gave the wrapper there the answer and the fingerprint file this server made
  (dumped from `secrets_scopes::a_starting_server_rewrites_the_fingerprint_files_of_live_runs`):
  the answer alone: `Unavailable { server: Unusable, file: None }`; with its file: `loaded:
  version 1 base 0 names ["MY_TOKEN", "SECOND"]`, a text with the raw and the hexadecimal form
  of one value and a second value came back as `A=[secret:MY_TOKEN] B=[secret:SECOND]
  hex=[secret:MY_TOKEN]`, and `run_scrub_exec` of `echo run=<value>` printed
  `run=[secret:MY_TOKEN]` with exit 0.
- **Speed** (release build, 1 MB of text, a probe test in `yulius-harness`, secrets of 24 to 43
  characters and every tenth of 3000):

  | Secrets | Needles | `dev` | This change |
  | --- | --- | --- | --- |
  | 10 | 60 | 460 ms | 126 ms |
  | 40 | 240 | 781 ms | 205 ms |
  | 200 | 1200 | 819 ms | 314 ms |

- **The old wrapper's speed on the file it now gets** (the same probe in the checkout of `dev`,
  `base` 0 and the last byte as its hash): 614 ms, 896 ms and 923 ms for 10, 40 and 200
  secrets, against 460, 781 and 819 ms with the file it had.
- **Gate.** `scripts/dev ci` at 106bbb5e1: exit 0; `4112 tests run: 4112 passed, 16 skipped`; doctor, repository check, lint,
  the macOS compile, cargo-deny, cargo-shear and rustdoc passed. (At 6535968fe, before round 2:
  exit 0, `4112 tests run: 4112 passed, 16 skipped`.)
- **Security scan.** Vidoc is switched off for this project in Yulius: disabled, not passed.
- **Review, round 1** (head 4b5e66fdb): request changes, one blocker and four major findings,
  all in this change's code, all taken:
  - the rustdoc link to a private function failed the gate: backticks;
  - "a store error answered an empty set": the guest question is asked with `?`
    (`matcher_for`), tested with the guests' table dropped;
  - "the empty set drops redaction of values its runs still hold": a guest playground keeps
    its MCP servers' values, tested through the wrapper's matcher; `.env` leftovers are stated
    in the ADR;
  - "a holder of the set learns more than the ADR says": tags went from 4 to 3 bits with the
    hexadecimal cases and base64 alphabets folded, two-factor seeds left a run's set, and the
    ADR tables the search that is left by value shape (16 letters and digits: 2^62);
  - "live contained sessions stop working after the update": a session from before version 2
    keeps a file in the old format with `base` 0, proved with the wrapper of `dev`;
  - minor: the Superagent branch and a failing rewrite are tested, the assertions that could
    not fail are gone, the launch file equals the endpoint's answer and the salt survives a
    restart (tests), files are rewritten at a guest's join, SPEC-002 no longer says "the
    rolling hash", the matcher's corners are tested, this section exists, a failed rewrite
    empties an old file; nits: `checked_add`, the salt without a silent zero, a window hashed
    once per length.
- **Review, round 2** (head cb3b3587a): request changes, one major and seven minor findings in
  this change's code, all taken; every finding of round 1 confirmed as answered, the ADR's table
  re-derived by the reviewer:
  - "the two-factor seed exclusion has no test": the fixture with a seed now shows the run's
    set leaving the seed alone and Yulius' own redaction replacing it;
  - "an emptied old-format file loses its format": a failed rewrite leaves a file of this
    server as it is and turns an older server's into `{"version":1}`; the test fails, heals
    and fails again;
  - "the ADR understates the base-0 file": the ADR says what it tells (about the last three
    characters), for which values, and the search that is left; operations and the change log
    say to rotate once the sessions from before the update have ended;
  - "20 random characters is wrong for hex": 20 letters and digits or 32 hexadecimal digits;
  - "the cache bump at a guest's join sits after a fallible call": moved to the join itself;
  - the Vidoc ignore's line and comment; the run that cannot be rewritten is seeded first and
    last; the old wrapper's timing is measured (above);
  - nits: one `agent_context` call decides the branch, the Plan item says what the code does,
    the read is bounded, the corner test names the secret, and #1239 tracks the removal of
    the old format's writer. The Global Superagent has no fingerprint file and no test of
    that absence.

### Slice 2 (2026-11-13)

- **The defects, each by a test that failed before its fix** (on the code of `dev`):
  - `a_bwrap_planted_on_the_runs_path_never_starts_a_run`: failed, the stand-in `bwrap` in
    `<sandbox>/home/.cargo/bin` had left its marker.
  - `managed_attributes_follow_no_link_an_agent_planted`: failed at its first check, the file
    outside the repository held `*.png binary` and Yulius's block instead of `kept`.
  - `the_push_guard_follows_no_link_an_agent_planted`: failed, the file outside the repository
    held the `pre-push` script instead of `kept`.
- **After the fix** all three pass, with the 95 tests of `yulius-git` (the guard, the scrub and
  the union merge among them) and `delivery::the_changes_log_merges_by_union_in_any_repository`.
- **Gate.** `scripts/dev ci` at 70e27996a (on `dev` b4eaca1e5 merged in): exit 0; `4117 tests
  run: 4117 passed, 16 skipped`; doctor, repository check, lint, the macOS compile, cargo-deny,
  cargo-shear and rustdoc passed.
- **Security scan.** Vidoc is switched off for this project in Yulius: disabled, not passed.
- **Not tested here.** A joined run (`yulius sandbox-exec` after the holder's namespaces): this
  sandbox has no holder to join; the refusal of a bare name there is a unit test, and the
  process host's lookup, which every launch passes first, is tested end to end.

### Slice 3 (2026-11-13)

- **Tests.** The seven tests named in the Test plan pass. Against the code before the fix:
  `the_push_guard_follows_no_link_an_agent_planted` failed without the comparison of the common
  directory (run with the comparison switched off: the install into the directory elsewhere was
  not refused); `managed_attributes_follow_no_link_an_agent_planted` failed on `dev` in slice 2.
  The transcript, tail, `passwd`, listing, usage and review tests were written with their
  fixes: on `dev` the readers they cover take a path (`File::open`, `is_file()`,
  `read_dir(path)`), their FIFO cases would not return, and their link cases read the target;
  they were not run against `dev`.
- **Gate.** `scripts/dev ci` at fc7de6a02 (on `dev` 5f581c3f3 merged in): exit 0; `4123 tests
  run: 4123 passed, 16 skipped`; doctor, repository check, lint, the macOS compile, cargo-deny,
  cargo-shear and rustdoc passed.
- **Security scan.** Vidoc is switched off for this project in Yulius: disabled, not passed.
- **Review of slice 2** (head 7248a0cec): approve, no blocker or major; its four minor findings
  and two of its nits are taken in this slice (HRD-006, the test case, the change log's
  wording, `ok_or_else`, a test that cannot replace its own process); "each new test failed on
  `dev` first" in the slice 2 Test plan holds for the three tests its Evidence names, not for
  the two unit tests of functions that are new; the directory is not synced after a rename in
  `replace_in`, as before the change.
- **Not tested here.** A sandbox with a real system layer and holder: `system_file_text` is
  tested on folders laid out like one.
- **Review of slice 3, round 1** (head cb7da51fc): request changes, two major findings, taken:
  - "the walk can use up the server's file descriptors" (own code): `MAX_DEPTH` 128, deeper
    folders counted and not listed, the measurement marked truncated;
    `usage::tests::folders_nested_deeper_than_the_bound_are_not_listed`;
  - "one transcript reader still goes by path" (there before): `continue_session` opens the
    imported transcript with `transcripts::open` and copies its side files through handles;
    `session_docs::tests::side_files_are_copied_without_following_a_link`;
  - minor: a transcript that is no regular file reads as none (pages, a task's transcript,
    `session_read`), not as a 500; `native.exists()` became a no-follow check; the review's
    anchor (the worktree's folder, above which a link misleads Git first: finding G1) is
    stated in ADR-0315; nits: path parts below the anchor must be plain names, the change
    log's long line is wrapped. Left as they are, with the reviewer's reasons: an opaque
    `upper/etc` without `passwd` shows the base's (a server-built copy); a root is not kept
    in the hard-link set; a FIFO regression would end in the test runner's timeout.
- **Gate after round 1.** `scripts/dev ci` at bb4859b97: exit 0; `4125 tests run: 4125
  passed, 16 skipped`; the other steps passed.

### Slice 4 (2026-11-13)

- **Tests failing before their fix.**
  `delivery::a_pull_request_pointed_at_another_branch_is_never_merged`: with the three checks
  switched off the armed merge merged the pull request that had been pointed at `main`.
  `ui_contract::embedded_assets_serve_with_content_types_and_every_import_resolves`: failed
  while the lab was embedded (`GET /assets/lab.js` answered 200). `yulius-store`
  `writes_that_change_access_grow_the_access_epoch`: failed at "an API token revocation".
  `remote_mcp::a_read_token_reads_nothing_above_view_access`: its last route
  (`superagent/requests`) answered 200 until that function refused a read token; the other
  seven routes were refused by `require_for`, written before the test. The sign-in tests
  (`a_github_sign_in_completes_only_in_the_browser_that_started_it`,
  `github_sign_in_starts_are_limited_per_client`) and the production-stage command's
  refusals were written with their fixes and not run against `dev`.
- **Web UI.** `cd e2e && npx e2e run`: `Test Files 84 passed | 1 skipped (85)`, `Tests 1018
  passed | 1 skipped (1019)`, exit 0, on this slice's tree.
- **Gate.** `scripts/dev ci` on this slice's tree (the commit after this text names it): exit 0;
  `4129 tests run: 4129 passed, 16 skipped`; doctor, repository check, lint, the macOS compile,
  cargo-deny, cargo-shear and rustdoc passed.
- **Security scan.** Vidoc is switched off for this project in Yulius: disabled, not passed.
- **Review of slice 3, round 2** (head 5810cc78f): approve, no blocker or major; its three
  minor findings and two nits are taken in this slice: the bound on an imported session's side
  files holds across folders (`copy_folder` stops at 0), the depth test no longer assumes a
  file system where folders take no blocks (run with `TMPDIR` on ext4: passed), one file of an
  imported session is copied only up to 1 GiB, the side files' path must be plain names, and a
  folder that cannot be listed is left out.
- **Not tested here.** A real browser against a real server (the cookie on a top-level
  navigation back from github.com): the routes are tested through the router with the cookie
  header set as a browser would.

### Slice 5 (2026-11-13)

- **Tests failing without their fix** (`target/audit/slice5/mutate.py`: one fix switched off,
  its tests run, the file restored). The unlock (`fresh_to_widen` answering "not needed"):
  `secrets::a_use_by_use_secret_goes_somewhere_new_only_after_two_factor`,
  `secrets::agents_request_hosts_and_people_allow_them`,
  `secrets::agents_bind_the_hosts_of_a_secret_they_set_up` and
  `remote_mcp::secrets_are_managed_by_name_and_binding_only` failed. The proxy using an empty
  set instead of refusing: `a_request_whose_answer_cannot_be_checked_for_secrets_is_not_sent`
  failed. The terminal not withholding: `output_is_withheld_while_agent_secrets_cannot_be_read`
  failed. `vault_update` without the value in its compare:
  `an_edit_read_before_a_rotation_does_not_undo_it` failed.
- **Slice 4's tests, as its reviewer asked** (the same way): the callback not asking for the
  cookie: `a_github_sign_in_completes_only_in_the_browser_that_started_it` failed; no limit on
  starts: `github_sign_in_starts_are_limited_per_client` failed; the production-stage command
  without `may_deploy`: `delivery::a_production_environment_run_starts_in_production_containment`
  failed.
- **Web UI.** `cd e2e && npx e2e run` at `7f3379e06` (nothing under `web/` or `e2e/` changed
  after it): `Test Files 84 passed | 1 skipped (85)`,
  `Tests 1018 passed | 1 skipped (1019)`, exit 0. (At `78c891dac` one test failed in the whole
  suite and passed alone: `cd.e2e.ts` "a needs_you decision reads Decide, and an action alone
  only To do", the known flake under load of issue #1175, in a page this slice does not touch.)
- **Review of slice 5, round 1** (head 78c891dac): request changes, one major and seven minor
  findings and two nits, all in this slice's code, all answered in it. Major: the copy budget
  took a file's stated size, so a file growing while it was copied was not counted (now what
  is read counts; the test with a file stating 0 failed before). Minor: a secret that never
  opens made every chunk of output read and open all secrets and log (a retry after a second,
  one log line; ADR-0317 no longer says it recovers by itself); allowing production deploys
  was not asked for the code (it is); an answer given without the code because `auto_allow`
  was on could land after it was switched off (the two store writes compare it); an overtaken
  team edit answered 404 (409); the app's own failing paths were untested (a Superagent's real
  terminal and the app's proxy policy with a secret whose key is gone); closing the code
  dialog was untested (e2e, with "nothing before the code" for `.env`, production and the
  switch). Nits: a doc comment on the wrong helper; the mock compared hosts as typed.
- **Review of slice 5, round 2** (head fa200a9ed): request changes, two minor findings and two
  nits; round 1's answers all verified. Minor: with a withheld screen the supervisor's wait
  timed out and typed blind, which a withheld folder-trust dialog would have taken as its
  answer (the wait now types nothing while the screen is withheld and flags the task; ADR-0317
  corrected); "revoking production needs neither" had no test (a token revokes in
  `production_secrets_need_their_own_allowance`). Nits: the race guards are tested in the
  store only (below, "Not tested here"); the copy budget's failing run is not in the mutation
  log (its test was run and seen failing before the fix was written: "Ok(())" where a refusal
  was expected); issue #1175 is the flake's own issue (`gh issue view 1175`).
- **Review of slice 5, round 3** (head f4e864bec): request changes, one minor finding and three
  nits; round 2's answers verified. Minor: `type_into_session` (messages from agents and
  Yulius's own) read the trust dialog off the same withheld screen and typed (it refuses now).
  Nits: a failure at a program's last output stayed "withheld" and flagged a healthy run with
  the wrong remedy, and after a recovery the screen missed what was dropped (one rule now: a
  screen with dropped output is unknown until the next whole screen, the wait flags at once
  and says to stop and continue); ADR-0317 said no run starts meanwhile (one whose own
  secrets are readable does); the test did not show that nothing was typed (it reads the
  transcripts now). The round-2 fix had also flagged a goal given at the launch, which is not
  typed: the launch goal's watch is left as it was.
- **Gate.** `scripts/dev ci` at `5fde92f16` (round 3's answers; `2021b3604` after round 2:
  4138 passed; `7f3379e06` after round 1: 4137 passed; `78c891dac` before the reviews: 4134
  passed): exit 0; `4138 tests run: 4138 passed, 16 skipped`; doctor, repository check, lint,
  the macOS compile, cargo-deny, cargo-shear and rustdoc passed.
  `first_message::a_withheld_screen_gets_nothing_typed_and_flags_the_task` with the wait's
  check switched off: "timed out waiting for the flag"; with the message's check switched off:
  its assertion on the refusal failed.
- **Security scan.** Vidoc is switched off for this project in Yulius: disabled, not passed.
- **Review of slice 4** (head f07fde13c): approve, no blocker or major. Its two minor findings
  are fixed here (one byte budget for an imported session's transcript and side files; the
  `busy` text), and of its five nits: tests for the cleared cookie and the plain-HTTP cookie
  name, two consequences added to ADR-0316, the fail-before-fix runs above; `form_train` is
  left as it is (Decisions).
- **Not tested here.** The two races through the app (an `auto_allow` switched off between an
  answer's read and its write; a second edit of one team secret): the store's compare and the
  refusal's wording are tested, the interleaving is not. The proxy process end to end with the app's policy failing (the proxy is
  tested with a policy that has nothing to redact with, the app's policy on its own); a task
  agent's terminal (the Superagent's goes through the same `scrub_output`). The two-factor
  dialog over the open edit form is tested in the mock (e2e), not against a real server.

### Slice 6 (2026-11-13)

- **Reproduced on a real browser** (Google Chrome 157.0.8103.50 on Xvfb, this machine, the
  opt-in test with `YULIUS_TEST_CHROMIUM`): before the fix a click through
  `BrowserInstance::dispatch_mouse` on a page's file input mapped the window `"Open File"`,
  1280x800, over the page (`xwininfo -root -tree`), and the test failed with "a native file
  dialog opened". With the fix and no policy installed: "the host's browser policy is not
  installed: the click was refused", no window, passed. With the policy written to
  `/etc/opt/chrome/policies/managed/yulius.json`: "the host's browser policy is in force: the
  click reached the page" (the page's title turned to "clicked"), no window; then Ctrl+O,
  Ctrl+S, Ctrl+P, Ctrl+U, F12 and Ctrl+Shift+I through `dispatch_key`: no window shown (GTK
  keeps one unmapped), no file dialog, no `devtools:`, `view-source:`, `file:` or print page
  among the targets; and a click on a link to another program's address
  (`yulius-test-scheme://open`): under the first draft of the policy the page stayed and the
  address was handed over; with only web addresses allowed the tab shows it blocked; passed.
- **Probes of the policy on that Chrome.** `Page.navigate` to the marker host:
  `net::ERR_BLOCKED_BY_ADMINISTRATOR`; to `file:///etc/hostname`: the same; to another
  `.invalid` host: `net::ERR_NAME_NOT_RESOLVED`. `showOpenFilePicker()`: `NotAllowedError`.
  With `DeveloperToolsAvailability: 2` in the file `Target.attachToTarget` answered "Not
  allowed" for every target, so it is left out. The existing real smoke test
  (`drives_a_real_chromium_when_configured`: load, snapshot, type, click, screenshots, PDF
  frames) passed with the policy installed.
- **Tests failing without their fix** (`target/audit/slice6/mutate.py`, ten cases; held tabs
  dropped and a tab at Chrome's own page dropped fail
  `browser::tabs_made_while_the_probe_waits_are_listed_and_a_tab_at_chromes_page_stays`): the mouse
  gate off, the key gate off, typed text ungated, the probe's tab listed:
  `browser::viewer_clicks_and_keys_wait_for_the_hosts_browser_policy` failed; Chrome's own
  pages listed: `browser::chromes_own_pages_are_no_tabs` failed; a look-alike hidden by its
  address: `browser::a_page_cannot_hide_a_window_behind_the_probes_address` failed; host setup
  writing nothing:
  `host_setup::tests::the_browser_policy_is_written_where_it_is_missing_or_differs` failed; no
  flag on a refused message: `task_message::a_message_kept_out_of_a_withheld_screen_flags_the_task_once`
  failed.
- **Web UI.** `cd e2e && npx e2e run` at `2c4b50dbd`: `Test Files 84 passed | 1 skipped (85)`,
  `Tests 1019 passed | 1 skipped (1020)`, exit 0 (nothing under `web/` or `e2e/` changed
  after it).
- **Review of slice 6, round 1** (head 53cb2786d): request changes, two major and four minor
  findings and three nits, all in this slice's code, all answered in it. Major: the update
  helper's unit could not write the policy directories, so its re-check could never bring a
  newer policy (the shipped unit lists them, the setup writes a drop-in for a unit from
  before; test); the probe's tab was hidden by its address, which a page can open (it is
  known by Chrome's id now; test). Minor: a tab that went to a page of Chrome's own was
  dropped (only a target created as one is no tab); texts that did not match the code; the
  real-browser proof covered the file input only (accelerators and a link to another program
  added, which changed the policy to web addresses only); typed text and releases untested.
  Nits: a yes stood for the browser's life (asked again after 5 minutes); the probe could
  hold a viewer's connection (3 seconds at most); another policy file can override a setting
  (ADR-0318 says so).
- **Review of slice 6, round 2** (head 0a566a925): request changes, two minor findings and
  three nits; round 1's answers verified. Minor: the probe's tab could still be listed when
  Chrome announces it under another address first, or a resync runs meanwhile (every new
  page no page opened now waits for the id, and the probe's is taken off the list when the
  id is known); the replay of what waited and "a tab at Chrome's own page stays" were
  untested (test with a tab made beside the probe and a tab sent to `chrome://settings/`;
  two more mutations). Nits: the wait for the id survives a dropped probe now (a drop guard)
  and the tab's creation is timed; a replayed announcement no longer overwrites a newer one;
  `ws:` and `wss:` are in the policy's allowed addresses (whether the blocklist reaches
  WebSocket handshakes was not measured).
- **Gate.** `scripts/dev ci` at `d42465f95` (round 2's answers; `84a173793` after round 1:
  4147 passed; `53cb2786d` before the reviews: 4145 passed): exit 0; `4148 tests run: 4148
  passed, 16 skipped`; doctor, repository check, lint, the macOS compile, cargo-deny,
  cargo-shear and rustdoc passed. Both real-browser tests passed again with the final policy
  file.
- **Security scan.** Vidoc is switched off for this project in Yulius: disabled, not passed.
- **Review of slice 5, round 4** (head cf2f03a6e): approve, two nits; one is fixed here (the
  flag on a refused message), one is left with its reason (Decisions).
- **Not tested here.** That a task flagged for a withheld screen is flagged again after it was
  stopped and continued (the flag's reset at attach): a start reads the secrets, so a
  continued task does not start while the secret that withheld the screen is unreadable.
  The update helper's unit under systemd (its writable paths are tested against the fake
  host's answer to `systemctl show`). `yulius host setup` on a real host (the item is tested against the fake
  host; the policy file was written by hand for the real-browser runs, with the content
  `managed_policy()` gives). Chromium and a snap-packaged Chromium (only Google Chrome is on
  this machine): the browser is asked itself, so a flavor that reads no policy from these
  directories refuses viewers' input instead of taking it unlocked. The context menu's
  entries (Save as, Inspect) were not clicked on the real browser; their accelerators were
  pressed. That the accelerator check would see a window or page were one to open was not
  shown (without the policy the keys are refused before they reach Chrome).

### Slice 7 (2026-11-13)

- **Tests failing without their fix** (`target/audit/slice7/mutate.py`, ten cases; the two
  added after the review: the wrong page asked after the move, no activity entry for a
  secret gone astray): the page
  not asked under the lock: `instance::a_secret_is_typed_only_into_a_page_it_may_go_to` and
  `browser::agents_type_secrets_only_into_pages_of_their_hosts` failed (the secret was typed
  into `evil.example`); the page it moved to not asked:
  `instance::a_secret_typed_while_the_page_moves_on_is_reported` failed; navigate, reload and
  history each without the lock: `instance::a_navigation_waits_for_a_secret_being_typed`
  failed each time; a later held announcement dropped, a closed tab's announcement kept: the
  two probe tests failed.
- **Real browser.** `drives_a_real_chromium_when_configured` (Google Chrome 157, opt-in; it
  types secrets into a real page and takes screenshots): passed.
- **Web UI.** Nothing under `web/` or `e2e/` changed in this slice; the e2e suite last ran at
  slice 6 (1019 passed).
- **Review of slice 7, round 1** (head 48dbe9523): request changes, three minor findings and
  three nits in this slice's code, and one major finding that was on `dev` before it. Minor:
  the astray test could not tell which page the rule was asked about (the fake now moves to
  another site mid-typing and the test asserts both addresses and the host named); the
  application's answer and activity entry for a secret gone astray were untested (they are,
  through the MCP tool); "none of them can put another page under the keys" claimed more
  than a lock on the request gives (ADR-0319 and FSR-002 say what it covers; finding E9).
  Nits: a typing error no longer hides the astray report; one task owns the probe tab's
  creation and closes it unless the probe takes it, also when the probe is dropped; the
  ADR says who waits behind a navigation. The major one, a secret typed into a focused
  frame of another site, is finding E8 with its own slice.
- **Gate.** `scripts/dev ci` at `5a89ab57f` (round 1's answers; `0d924205e` before the review:
  4152 passed): exit 0; `4152 tests run: 4152 passed, 16 skipped`; doctor, repository check,
  lint, the macOS compile, cargo-deny, cargo-shear and rustdoc passed.
- **Security scan.** Vidoc is switched off for this project in Yulius: disabled, not passed.
- **Slice 6's delivery.** Its first CI run failed in one e2e test of a page it does not touch
  (`notes.e2e.ts` "a failed reload after a teammate's edit backs off": 4 loads counted in a
  fixed 2.5 s window, at most 3 expected), filed as issue #1246; the run was repeated once at
  the same head and passed.
- **Review of slice 6, round 3** (head 70cc7698d): approve, three nits, all answered here.
- **Not tested here.** The race on a real browser (a slow screenshot, a fill and a navigation
  at once): the order is tested on the fake with the lock held by a delayed look at the page.
  A page that redirects by itself while it is typed into, on a real site.

### Slice 8 (2026-11-13)

- **Tests failing without their fix** (`target/audit/slice8/mutate.py`, ten cases, each
  switched one check off and ran its test): the frames not asked (browser and through the MCP
  tool), the frames in other processes not asked, frames that cannot be listed typed into:
  `instance::a_secret_is_not_typed_into_a_frame_of_a_site_it_is_not_for` and
  `browser::a_secret_is_typed_neither_into_another_sites_frame_nor_a_tab_that_is_leaving`
  failed; where the tab is going not asked, a navigation given up still standing (each in the
  browser and through the MCP tool), only a link's navigation counting, a navigation inside
  the document counting:
  `instance::a_secret_is_not_typed_into_a_tab_leaving_for_another_site` and the MCP test
  failed.
- **Real browser** (Google Chrome 157, opt-in, `YULIUS_TEST_CHROMIUM`): four tests passed.
  `a_secret_is_not_typed_into_another_sites_frame_in_a_real_chromium`: a page at `127.0.0.2`
  framing `localhost` with the focus in the frame's field got nothing typed and the answer
  named `localhost`; with both allowed it was typed. Before the fix the same steps typed the
  secret into the frame. `a_secret_is_not_typed_into_a_tab_that_is_leaving_in_a_real_chromium`:
  a fill right after a click on a link to `localhost` (answering after 1.5 s) typed nothing.
  Measured there with a probe since removed: `Page.getFrameTree` on a new session answered
  1.52 s after the click with the new address (4.01 s with a 4 s server, also for a link to
  the same site), so on Chrome 157 the answer is `SecretPageRefused` by slice 7's rule and
  `SecretPageLeaving` is not reached; the test accepts either and says so.
- **The gate's first run** at `8d90eacc5` failed in
  `first_message::a_withheld_screen_gets_nothing_typed_and_flags_the_task` (slice 5's test):
  reproduced with a 300 ms delay before the typing flag was cleared (the same panic), fixed
  by ending the typing before the task is flagged, passed with the delay in place, delay
  removed.
- **Gate.** `scripts/dev ci` at `2f17216be`: exit 0; `4157 tests run: 4157 passed, 16
  skipped`; doctor, repository check, lint, the macOS compile, cargo-deny, cargo-shear and
  rustdoc passed.
- **Web UI.** Nothing under `web/` or `e2e/` changed; the e2e suite was not run for this slice.
- **Security scan.** Vidoc is switched off for this project in Yulius: disabled, not passed.
- **Review of slice 7, round 2** (head ec5c5c240): approve; merged as pull request #1247.
- **Not tested here.** `SecretPageLeaving` on a real browser: Chrome 157 holds its answer
  during a navigation, so only the fake reaches it. A frame nested in another site's frame
  in a process of its own on a real Chrome (the fake lists such targets; the real test uses
  one frame). Chromium builds other than Google Chrome 157.
- **Review of slice 8, round 1** (head a3320e970): request changes. Own code, all answered
  test-first:
  1. (major) the frame check hung on a focus test that failed open for `<object>`, `<embed>`,
     a frame in a closed shadow root and `<fencedframe>`. Now only a text field of a document
     the probe read is the page's own; anything else asks the frames; a fenced frame is
     refused. On Chrome 157, with round 1's gate, the secret was typed into `localhost` in an
     `object` and in a closed shadow root's frame of a `127.0.0.2` page
     (`target/audit/slice8/real-chrome-mutations.log`); with the fix both are refused.
  2. (minor) nothing was asked again between the look and the keys: the document is read
     again after the focus (a changed one gets nothing typed), where the frames are going is
     asked last, and a frame that has the focus after the keys is reported
     (`SecretFrameAstray`, with an activity entry).
  3. (minor) the record of announced navigations had no bound and only the main frame was
     asked: `Leaving` holds 256 frames, drops a frame's entry when it is detached, counts
     nothing older than 30 s and says "not known" past its bound; every frame of the page's
     tree is asked.
  4. (minor) untested branches: the fake sends any page event (`emit`); the tests cover
     `Page.frameRequestedNavigation` and its disposition, `Page.frameNavigated`,
     `Page.frameDetached`, a frame of the page, an announcement during the look, the 30 s,
     the bound, a session's end, a page that cannot be read, frames that cannot be told, and
     the order of focus and look. The last step of the leaving test now does what it says.
  Nits: skipped events lose the record ("try again" for 30 s); the frame error says what is
  known ("the focus is not in a field of this page itself, and the page holds a frame of
  …"); an address without a host is "an unknown site"; ADR-0320 names the page that cannot be
  read, the address of another program, and the frames inside an out-of-process frame that
  are not asked. Two lines stay without a test of their own (Decisions say why).
  Pre-existing (major), finding E10: fixed here (`tool_type_beside_secret`).
- **Tests failing without their fix, after the review** (`target/audit/slice8/mutate.py`, 28
  cases, `mutations2.log`): each failed its test: the frames not asked (browser, MCP), round
  1's gate, a fenced frame typed into, frames in other processes not asked, frames that
  cannot be listed typed into, a page that cannot be read typed into, the page read before
  the focus, a page that changed since the look typed into, a frame that took the focus not
  reported (browser, MCP), where the tab is going not asked (browser, MCP) or asked before
  the look, only the main frame's way asked, a navigation given up, arrived, or of a detached
  frame still standing, only a link's navigation counting, a same-document one counting,
  another tab's counting, the requested navigation not kept, an old announcement standing
  for ever, nothing lost past the bound, a session's end keeping its frames, lost
  announcements forgotten at once, and the user name typed without the page rule (E10).
- **Real browser after the review** (Chrome 157): the four opt-in tests passed; the frame
  test now also covers an `object`, a closed shadow root's frame that takes the focus by
  itself, the user name's path, and the page's own field beside a frame (typed).
- **The gate after the review.** At `c323db624` it failed in
  `superagent_browser::the_global_superagent_drives_a_named_projects_browser`: the fake's
  new focus kinds called an unnamed focused password field "other", so a screenshot was
  refused (a fault of the fake, fixed there). `scripts/dev ci` at `b71c09db1`: exit 0; `4160
  tests run: 4160 passed, 16 skipped`; doctor, repository check, lint, the macOS compile,
  cargo-deny, cargo-shear and rustdoc passed.
- **Not tested here, added by the review's answers.** A fenced frame on a real Chrome (the
  probe names the element; no page with one was built). The loss of announcements on skipped
  events, end to end (the function is unit-tested).
- **Review of slice 8, round 2** (head 2c1af3d04): request changes; "All nine round 1
  findings are answered as the coordinator described, and I checked each in the code." Four
  new ones in the new code, all answered test-first:
  1. (minor) an announced address cut to 2048 bytes could read as a host of a wildcard
     binding (`https://<long>.bank.example@evil.example/`): one too long is kept as no
     address, which no secret is for.
  2. (minor) a frame inside a closed shadow root that took the focus during the keys was not
     reported: after the keys the frames are asked by the rule asked before them (the focus
     not in a field of the page), and screenshots follow the same condition.
  3. (nit) an announcement sent a moment before the keys may not have been read: named in
     ADR-0320 (the report afterwards covers it).
  4. (nit) more than 256 frames on their way in one tab stopped fills in every tab: the bound
     and its loss are per tab; skipped events still lose every tab's.
- **Tests failing without their fix, after round 2** (`mutate.py`, 32 cases,
  `mutations3.log` and `mutations3b.log`): all 32 failed their tests; new: a cut address
  kept, one tab's loss not asked, after the keys only a fenced frame reported.
- **Real browser after round 2** (Chrome 157): the four opt-in tests passed.
- **The gate after round 2.** `scripts/dev ci` at `20404f5ae`: exit 0; `4161 tests run: 4161
  passed, 16 skipped`; doctor, repository check, lint, the macOS compile, cargo-deny,
  cargo-shear and rustdoc passed.

### Slice 9 (2026-11-13)

- **The hole, on a real browser.** With the proxy's switches taken out (the browser as it
  was), Google Chrome 157 of sandbox `s1` opened `http://localhost:43020/`, a port of sandbox
  `s2`, and showed its page ("other page";
  `target/audit/slice9/real-chrome-mutations.log`). With only
  `--proxy-bypass-list=<-loopback>` taken out the same test failed too: Chrome 157 sends the
  loopback past a proxy unless told otherwise (also seen with a logging proxy,
  `probe_proxy.py`: `localhost` reached the origin directly, and through the proxy with the
  switch).
- **Real browser, with the fix** (Chrome 157, opt-in, `YULIUS_TEST_CHROMIUM`):
  `browser_network::a_real_chromium_reaches_its_sandboxes_ports_and_no_other_local_address`
  passed: its own server by `localhost` and `127.0.0.1` and the welcome page load; another
  sandbox's port (three spellings), a host service, the internal API, `169.254.169.254` and
  two HTTPS addresses show none of those pages; a page's `fetch`, images and a WebSocket to
  them arrived at no server while its own answered. The four tests of `real_chromium.rs`
  passed as before.
- **Tests failing without their fix** (`target/audit/slice9/mutate.py`, 18 cases,
  `mutations.log` and `mutations-b.log`): each failed its tests: every port of the loopback
  reached, local addresses not refused, every path of Yulius's listener, a tunnel to it, a
  listener bound to every address not taken as Yulius's, what a name resolved to not asked,
  the proxy's hop headers sent on; the loopback past the proxy, names resolved by Chrome, UDP
  past the proxy, a browser started without its proxy, the proxy closed while the browser
  runs, the proxy outliving its browser (this one survived at first: the test let go of the
  browser itself; it now holds on to it, as a viewer's session does); every sandbox's ports
  for every browser, a proxy before the egress proxy runs, the server's browsers left to
  connect by themselves; and the two answers to slice 8's nits (a long address kept cut, or
  kept as none).
- **The gate.** Its first run (at `e57456953`) stopped at rustdoc: a doc comment had ended up
  on the wrong function. Its second (at `b814ad528`) failed in this slice's own
  `browser_network::each_browsers_proxy_reaches_its_own_sandboxes_ports`: the test's "service
  of the host" took a port the system gave, 43013, which lies in the fixture sandbox's range
  (43000-43019), so the sandbox's browser rightly reached it. Reproduced by putting that
  server into the range on purpose (the same assertion, `flake-proof.log`); the test now
  takes a port outside both ranges. `scripts/dev ci` at `b9bd0eb68`: exit 0; `4172 tests
  run: 4172 passed, 16 skipped`; doctor, repository check, lint, the macOS compile,
  cargo-deny, cargo-shear and rustdoc passed.
- **Web UI.** Nothing under `web/` or `e2e/` changed; the e2e suite was not run for this slice.
- **Security scan.** Vidoc is switched off for this project in Yulius: disabled, not passed.
- **Review of slice 8, round 3** (head 4fb8a2eab): "No critical, major or minor own-code
  finding is open." Approve, two nits, both answered here; merged as pull request #1248 (its
  first armed merge was refused for a title of 112 characters; retitled, merged at the same
  head).
- **Not tested here.** A real server with real sandboxes: the browser of one sandbox against
  the relayed ports of another on a host (the test's servers stand on the loopback where the
  relay would listen). WebRTC and QUIC being off (the switches are asserted, their effect is
  not). A viewer's address bar on a virtual display (it navigates the same Chrome through
  the same proxy). Chromium builds other than Google Chrome 157. macOS, where the same
  switches are passed.
- **Review of slice 9, round 1** (head 9c2af2437): request changes; no critical or major
  finding. Own code, all answered test-first:
  1. (minor) the loopback rule took any name that resolved to the loopback, resolved anew for
     each request: a name's owner could answer a public address for a page and `127.0.0.1`
     for the page's next request, and the page would read the sandbox's own dev server. The
     loopback's ports and the welcome page are now reached only by an address or `localhost`.
  2. (minor) `*.localhost` was asked of the host's resolver, which not every host answers:
     `localhost` and names below it are the loopback without a resolver.
  3. (minor) a plain request's wait covered its upload with one fixed bound: it now restarts
     with every piece of the body sent on (`Progress`, as an agent's).
  4. (minor) ADR-0320, its index line, FSR-005 and AC-46 still said a long address is kept as
     none: amended (its site is kept).
  5. (minor) the share test proved only the browser's own limit: an agent run's tunnel now
     takes the place and the browser is refused, and the other way round.
  Nits: the proxies hold the egress proxy weakly (no cycle through the application); the
  listener backs off on accept errors and closes a connection that sends no request for 60 s;
  the origin's `Host` and the error texts carry no user and password; the public listener is
  never reached at its own address, also with its port in the sandbox's range; tunnel by
  `localhost`, listed and answered hop headers are tested; a refusal with no known ports says
  so. Pre-existing: ranges that are never given again drift into the system's free ports,
  where a browser's proxy can take one: issue #1249.
- **Tests failing without their fix, after the review** (`mutate.py`, 31 cases,
  `mutations2.log` and `mutations2b.log`): all 31 failed their tests. One reported "passed"
  in the first run: the script's filter had left the new integration test out; with the
  filter corrected it failed there too. New: a name that resolved to the loopback reaching
  the sandbox's port or the welcome page, the test map's answer counted as an address,
  `localhost` asked of the resolver, the public listener reached, the bound not restarting
  with the body, a silent server waited for without end, the origin's hop headers coming
  back, a listed header sent on, a share of the browser's own, unknown ports read as a
  range, user and password sent on as the host, the proxies keeping the egress proxy alive,
  the public listener not passed to the proxies.
- **Real browser after the review** (Chrome 157): the opt-in network test passed.
- **The gate after the review.** `scripts/dev ci` at `d2b1afed2`: exit 0; `4176 tests run:
  4176 passed, 16 skipped`; doctor, repository check, lint, the macOS compile, cargo-deny,
  cargo-shear and rustdoc passed.
- **Not tested, named by the review.** How Chrome 157 classes a proxied answer's address
  space (the fix does not depend on it); WebTransport under a fixed proxy; a managed proxy
  policy on the host overriding the switches (documented in `docs/operations.md`).

### Slice 10 (2026-11-13)

- **The hole, on the adapter as it was** (`target/audit/slice10/unconfined-proof.log`: the
  `git_confined` tests with the confinement taken out of the tests' adapter): the server's
  Git ran the clone's `filter.mine.process`, `core.sshCommand`, `remote.origin.uploadpack`
  and `remote.origin.receivepack` (four markers); a fetch replaced a file outside the sandbox
  with Git's `FETCH_HEAD` ("…branch 'main' of …/playgrounds/s1"); a push by the remote's name
  arrived at another sandbox's origin. By hand, a link at `.git/shallow` gave "fatal: bad
  shallow line: secret-line-of-the-host".
- **Confined, by hand first** (`yulius git-confined`, Landlock ABI 8 here): `status`, `log`
  and an HTTPS `ls-remote` work; a filter's command fails with "cannot exec 'sh -c …':
  Permission denied"; the linked `FETCH_HEAD` is refused ("cannot open '.git/FETCH_HEAD':
  Permission denied") and the file outside kept; a local fetch needs the shell ("cannot exec
  'git-upload-pack …'") and works with it; with the shell, `core.alternateRefsCommand` and
  `core.sshCommand` through a rewritten address ran, and with the pins they did not
  ("transport 'ssh' not allowed").
- **The binary's whole suite, confined.** Every test that uses the real adapter now uses it
  confined in the server's layout (`support::confined_git`, 45 places). The first such run
  failed 181 tests and showed what a confined command needed named: destinations of `init`
  and `clone`, the other folder of local transfers, bundles, a CD export's index, worktree
  paths, a playground's origin for pushes by the remote's name, import worktrees beside the
  sandbox's folder. After that: 2134 of 2134 passed.
- **Tests failing without their fix** (`target/audit/slice10/mutate.py`, 23 cases,
  `mutations.log`): each failed its tests: Git unconfined, the shell always executable,
  every program executable, writes not confined, reads not confined, a Git not on `PATH`
  started by name, a local transfer without its pins (attack test and unit), with the
  adapter's configuration, without the shell, the shell for every command, what a command
  names not reached, any folder taken as an origin (attack test and unit), a sandbox's
  folders being the whole data folder (attack test and unit), import worktrees no sandbox's,
  a push or a fetch with the own origin by the remote's name, the server's adapter not
  confined, the doctor passing a host that cannot confine; and the two extras: a request
  without a body framed as chunked, a withheld screen flagged once per run.
- **A flaky test of slice 5**, met in this slice's first confined run:
  `task_message::a_message_kept_out_of_a_withheld_screen_flags_the_task_once` failed 5 of 24
  runs alone (the task read `Waiting`, or its flag had no reason): the prompt the test typed
  started a turn of the fake agent, whose states replaced the flag. With a key that starts
  no turn: 30 of 30. The flag is now raised again after such a replacement (test first:
  `Waiting` where `NeedsAttention` was expected).
- **G2, found here** (`urlmatch-proof.log`): with `http.https://github.com/.sslVerify=false`
  and `.proxy` in the clone's configuration, `git config --get-urlmatch` under the server's
  command line (`-c http.sslVerify=true -c http.proxy=`) and environment answered `false`
  and the clone's proxy. Listed, not fixed here.
- **Gate.** `scripts/dev ci` at `f819929cd`: exit 0; `4188 tests run: 4188 passed, 16
  skipped`; doctor (with "git confinement: ok"), repository check, lint, the macOS compile,
  cargo-deny (the new `landlock` crate: MIT OR Apache-2.0), cargo-shear and rustdoc passed.
- **Web UI.** Nothing under `web/` or `e2e/` changed; the e2e suite was not run.
- **Security scan.** Vidoc is switched off for this project in Yulius: disabled, not passed.
- **Review of slice 9, round 2** (head 72b4a521f): "No critical, major or minor own-code
  finding is open; two nits." Approve; both nits answered here; merged as pull request #1250.
- **Not tested here.** A kernel without Landlock (the launcher's refusal is code read, not
  run: this machine enforces it). A real server with real sandboxes, a relay and GitHub over
  HTTPS from the confined adapter (HTTPS was run by hand through this sandbox's proxy; the
  suite's "GitHub" is folders, with the shell). Git builds whose helpers live outside their
  `exec-path`. macOS, where Git runs unconfined as before.

### Slice 11 (2026-11-13)

- **The hole, on the code as merged in slice 10** (`target/audit/slice11/mutations-b.log`,
  the courier switched off): with the clone naming a proxy for the remote and no certificate
  check, the server's fetch connected to that proxy
  (`git_confined::a_transfer_with_the_token_is_not_shaped_by_the_clone`: "the clone's proxy
  was asked for the server's connection"). The precedence itself:
  `target/audit/slice10/urlmatch-proof.log`.
- **The loader, by hand** (`target/audit/slice10/loader-gadget.log`): a confined bare
  `git diff` with `diff.external` set to the loader executed a new file of the clone that was
  a copy of `env` ("added.bin: '/dev/null': Permission denied" is that program trying to run
  its argument); with `--no-ext-diff --no-textconv` it did not. The adapter's own diff
  commands carried those options already, so its attack test passed before the change too;
  the runner now adds them itself (the unit test failed first: "left: [diff, --stat, HEAD]").
- **Tests failing without their fix** (`target/audit/slice11/mutate.py`, 13 cases,
  `mutations.log` and `mutations-b.log`): each failed its tests: transfers with the token in
  the clone as before (alone, and with its configuration given there), a command in a
  sandbox's folder given the adapter's configuration, a courier's transfer allowed between
  folders, the courier left behind, the clone not taking the result, no pruning of the
  clone, a forcing push, no diff options from the runner, the `ssh` transport kept, signing
  kept, an editor allowed, a sandbox being made given the folder of all sandboxes. One step
  has no test of its own: the courier learning what the clone has of the remote before it
  fetches. Without it the fetch still arrives; it only asks GitHub for more than it needs.
- **The binary's whole suite**: 2246 of 2246 with the courier and the stricter runner; three
  tests of `yulius-git` whose stand-in Git read its subcommand by position were adjusted to
  the longer list of constant options.
- **Slice 10's delivery.** Its first CI run failed in one e2e test of a page it does not
  touch (`cd.e2e.ts` "a refusal after its view was left is a toast…", the known flake of
  issue #1171); the run was repeated once at the same head and passed.
- **Review of slice 10, round 1** (head cdfab7d11): "No critical, major, or minor own-code
  finding is open — only nits plus the documented, deferred pre-existing G2." Approve; the
  four nits are answered here; merged as pull request #1251.
- **Gate.** `scripts/dev ci` at `1f7cee192`: exit 0; `4192 tests run: 4192 passed, 16
  skipped`; doctor, repository check, lint, the macOS compile, cargo-deny, cargo-shear and
  rustdoc passed (the run before it stopped at rustdoc: a public comment linked a private
  item).
- **Web UI.** Nothing under `web/` or `e2e/` changed; the e2e suite was not run.
- **Security scan.** Vidoc is switched off for this project in Yulius: disabled, not passed.
- **Not tested here.** A transfer with the real GitHub through a courier (the tests' remote
  is a folder the address is rewritten to, or an address where nothing listens); how much a
  courier's fetch asks of a large repository; a proxy that would really read the header
  (the test's proxy counts connections). macOS and other unconfined Git: unchanged.
- **Review of slice 11, round 1** (head dda456b3b, request changes; two major, five minor,
  seven nits, all answered in the slice). The majors, in the reviewer's words: "A confined
  Git still starts the clone's file through the loader, via `core.askPass` on the
  `Route::Named` fallback" (reproduced by the reviewer under the launcher) and "Every
  transfer with GitHub fails when the data folder path is not canonical". What changed for
  each finding is in Decisions; each has a test that fails without it (below).
- **G3, by hand** (`target/audit/slice11/lazy-fetch-proof.log`, Git 2.53.0, the launcher of
  head dda456b3b): with a promisor remote in the clone's configuration whose command for
  the other side writes a file, a confined push between two folders (the shell allowed) of
  a commit whose tree no repository has wrote the file; a confined command without the
  shell could not start the command ("cannot exec … Permission denied"); with
  `GIT_NO_LAZY_FETCH=1` the push wrote nothing. A fetch into such a clone did not start the
  command in these tries. **Ubuntu 24.04** (`git-2404-lazy-env.log`, a container with
  2.43.0-1ubuntu7.4): the variable is honoured there ("with the variable: not-run",
  "without: /tmp/RAN"), though that Git does not know the option of the same name
  (`git-2404-lazy.log`), which is why the doctor's probe asks by doing, not by version.
- **Tests failing without their fix, round 1's answers** (`target/audit/slice11/mutate2.py`,
  `mutations-c.log`, 17 cases): a program asked for a password; a missing object fetched; a
  remote without an address named to Git; a clone that writes in a sandbox's folder given
  the configuration; every `GIT_CONFIG_` variable withheld; the diff flags dropped past the
  options' end; a signed push left to the clone; the objects allowed anywhere; the
  sandbox's folders compared as written (the linked data folder); a path of two lines; a
  listing that borrows objects; a clone made in the sandbox's folder; no note of the
  default branch; the note made over the clone's own; a `..` part naming a sandbox; a
  doctor that takes any Git; the first commit made in the sandbox's folder. Each failed its
  test. The 13 cases of the first round were run again on the new code
  (`mutations-d.log`): each still fails. Still without a test of its own: what the courier
  is told the clone has (it only saves the remote work).
- **Gate after round 1's answers.** `scripts/dev ci` at `7b35303aa`: exit 0; `4197 tests
  run: 4197 passed, 16 skipped`; doctor, repository check, lint, the macOS compile,
  cargo-deny, cargo-shear and rustdoc passed (the run before it stopped at rustdoc once
  more: a public comment linked a private item).
- **Not tested, round 1's answers.** A clone and a transfer with the real GitHub through a
  courier (as above); the race of a sandbox's process against a new clone's configuration,
  which the clone in a courier removes (the reviewer did not reproduce it either); a Git
  that really lacks `GIT_NO_LAZY_FETCH` (the doctor's test takes the variable away from
  this machine's Git with a wrapper); whether a fetch, rather than a push, can be made to
  fetch a missing object (it did not in the tries above; the variable covers both).
- **Review of slice 11, round 2** (head 99083c33b, request changes): all fourteen findings
  of round 1 answered; two new major ones, in the reviewer's words: "A push from a
  sandbox's clone to a folder recurses into submodules, and the shell runs the submodule's
  own `receivepack` command" (the mechanism from slice 10) and "`clone_through`'s second
  step lets the shell run a filter raced into the new clone's configuration" (own code of
  round 1). Both reproduced here before the fix: by hand under the launcher
  (`target/audit/slice11/n1`: the push wrote the marker, and with the two pins it did not),
  and by the new tests with their fix switched off (`mutations-e.log`, 8 runs: the push
  into a submodule with neither key pinned; each pin missing, in the runner's test; a new
  clone's files checked out by the transfer, three runs of the race, each won by the
  filter; a new clone's files not checked out at all). Either pin alone switches both of
  the clone's keys off, as Git reads the command line last; the attack needs both gone.
- **A mistake of the first try, kept as a fact about the tests:** the stand-in for GitHub
  allows the shell to every command, so a filter in a clone made through it runs at the
  checkout whatever the code does; the raced clone is therefore tested from a playground's
  origin with the adapter as the server builds it, and a clone through a courier takes the
  same two steps (`copy_without_files`, `check_out_files`).
- **Gate after round 2's answers.** `scripts/dev ci` at `3e200cfce`: exit 0; `4199 tests run: 4199
  passed, 16 skipped`; doctor, repository check, lint, the macOS compile, cargo-deny,
  cargo-shear and rustdoc passed.
- **Open after round 2:** G5 (the ledger), then meant for the next slice.
- **Review of slice 11, round 3** (head 6a385ee61, request changes): both findings of round
  2 answered; one new major one, in the reviewer's words: "a fetch that may start the
  shell, run in the clone, downloads the clone's `fetch.bundleURI` over HTTP despite
  `protocol.http(s).allow=never`. On a 401, Git starts the clone's credential helper
  through `/bin/sh`", in "the clone's fetch from a courier, and the folder fetch". Its
  fix as proposed (three more pins) is applied as depth; the finding itself is answered by
  the rule (Decisions, round 3).
- **The new test against the code of round 2**
  (`target/audit/slice11/n3-before.log`: the Git crate of head 6a385ee61 under the tests of
  this head): `git_confined::a_clones_transfer_settings_reach_no_transfer` failed with "the
  clone's bundle address was connected to", `left: 4`.
- **Git's part, by hand before the code** (`target/audit/slice11/y`, `sh`; Git 2.53.0): a
  fetch in a bare repository with `GIT_OBJECT_DIRECTORY` set to another repository's
  objects wrote the objects there and none into its own; it sent `have` lines for refs
  written into its `packed-refs`; it made the tag of an old commit that the other side had
  newly (v2) without a download; `update-ref --no-deref --stdin -z` set, made and deleted
  refs in one go, a missing ref's deletion included; `checkout -B <branch> --track
  origin/<branch>` checked the branch out in a repository without a commit, under the same
  and under another name; with the courier's `shallow` a link to the clone's, a fetch with
  `--update-shallow` from a shallow repository wrote the clone's `shallow` file through the
  link, the clone passed `git fsck`, and a second fetch kept it.
- **Tests failing without their fix, the slice as it is now**
  (`target/audit/slice11/mutate3.py`, `mutations-f.log`, 41 cases, each failed its test).
  The rule on the shell: a command in a sandbox's folder given the shell; a transfer
  between folders made there. The token: a command in a sandbox's folder, and one that
  writes a new repository into one, given the configuration; every `GIT_CONFIG_` variable
  withheld; a remote without an address named to Git. Every command: a program asked for a
  password, a missing object fetched, an editor, a signed push, either submodule key, a
  credential helper, a bundle address, a negotiating push, the `ssh` transport, signing,
  the diff options and their copy past the options' end, a command's input not written.
  The courier: objects anywhere, folders compared as written, a listing on the objects,
  the courier left behind, no pruning, a dropped branch kept, branches not set, a tag made
  again, no tag with a fetch, tags with a refresh of the branches, a forcing push, no link
  to the clone's `shallow`, no `--update-shallow`, no note of the default branch, the note
  made over the clone's own, the default branch not checked out, a clone made by a
  transfer in the sandbox's folder. And: a `..` part naming a sandbox, a sandbox without a
  folder given all sandboxes' folder, a doctor that takes any Git, the first commit made
  in the sandbox's folder. The earlier scripts (`mutate.py`, `mutate2.py`) are of code
  that is gone; their cases are in this one where the rule they check still exists.
- **The binary's and the two crates' suites with the new routing:** `2776 tests run: 2776
  passed` (`suite-y2.log`); on the first run two failed and showed what the design had
  missed: a playground made of a shallow upload (the clone was not told where the history
  ends: the link to its `shallow` file), and this slice's own test of a path that needed
  one line, a rule that went with the alternates file.
- **Gate.** `scripts/dev ci` at `1796cd4b2`: exit 0; `4203 tests run: 4203 passed, 16 skipped`;
  doctor, repository check, lint, the macOS compile, cargo-deny, cargo-shear and rustdoc
  passed.
- **Not tested, the slice as it is now.** A transfer with the real GitHub through a courier
  (the tests' remote is a folder the address is rewritten to, or an address where nothing
  listens); what the courier is told the clone has (it only saves the remote work: no test
  fails without it); that the courier's fetch runs no maintenance on the clone's objects
  (the options are given; Git's automatic maintenance needs thousands of loose objects to
  start, and no test makes them); a Git that really lacks `GIT_NO_LAZY_FETCH` (the doctor's
  test takes the variable away from this machine's Git); a repository of SHA-256 objects
  (the courier is made with the host's default, as a playground's origin is; GitHub has
  none); macOS and other unconfined Git: unchanged, G2 and G5 stay open there.
- **Fresh review of the slice as redesigned, round 1** (head 87dc1c353, a reviewer who had
  not seen the earlier rounds; request changes): the rules held ("The rule on the shell:
  `command()` gives `--shell` only when the command is not in a sandbox; `run_once` refuses
  a local transfer in a sandbox"; "No `GIT_CONFIG_*` reaches a command whose folder is a
  sandbox's"), and five findings in own code, all reproduced by the reviewer but one: "A
  fetch through a courier fails permanently after a branch on the remote is replaced by
  one below it (or the reverse)"; "Through the courier's `shallow` link, a sandbox's agent
  writes files into its playground's origin"; "A shallow clone cannot be fetched when the
  data folder is reached through a link"; "A failed new clone into a sandbox is never
  checked out afterwards" (not reproduced, the path read); the ADR's index line of the
  first design. Five nits, all answered too (Decisions).
- **Tests failing without their fix, after it** (`mutate3.py`, `mutations-g.log` and
  `mutations-g2.log`, 50 cases, each failed its test): the 41 before, of which the cases
  on setting refs are now four (a new branch not made, a moved one not set, a moved one
  set whatever the clone has by now, the dropped ones in the same step as the new), and:
  a fetch allowed to write in the folder it takes from, what a command only reads granted
  for writing, the courier reaching the objects' folder only, a failed clone left, a clone
  into a folder that holds something, a new clone with only the tags of its branches. One
  case passed at first and showed a test that could not fail: the shallow clone "behind a
  link" was under the tests' shared temporary folder, where the link's target is a data
  folder too; the test now builds a server's adapter whose data folder is the link itself.
- **The suites after it:** `2776 tests run: 2776 passed` (`suite-y3.log`).
- **Gate.** `scripts/dev ci` at `099291a12`: exit 0; `4203 tests run: 4203 passed, 16 skipped`;
  doctor, repository check, lint, the macOS compile, cargo-deny, cargo-shear and rustdoc
  passed.
- **Not tested, besides the list above:** a failed clone through the application (the
  adapter's clone from an address where nothing listens is); a clone with more refs than
  the runner reads in one answer (16 MiB).

### Slice 12 (2026-11-13)

- **Slice 11's delivery.** Pull request #1252 merged into `dev` at 67ab28d09, whose own
  Yulius CI run passed; the fresh reviewer approved that head in its second round ("every
  round-1 finding (R1–R5, N1–N5) is answered"; two nits, answered here: the folders a
  transfer only reads, and the index line of ADR-0323).
- **Tests failing without their fix** (`target/audit/slice12/mutate.py`, `mutations.log`, 6
  cases, each failed its test): an agent run given a production secret; the old hosts
  kept; their removal not written to the audit log; every secret's hosts removed; a
  production secret with hosts inserted; what a transfer only reads granted for writing.
- **The suites:** `2884 tests run: 2884 passed` for the four crates touched
  (`target/audit/slice12-suite1.log`); the secrets page's e2e file, `30 passed`
  (`target/audit/slice12-e2e.log`), with the part about a production secret's old hosts
  removed from it.
- **Gate.** `scripts/dev ci` at `a05564671`: exit 0; `4205 tests run: 4205 passed, 16 skipped`;
  doctor, repository check, lint, the macOS compile, cargo-deny, cargo-shear and rustdoc
  passed.
- **Security scan.** Vidoc is switched off for this project in Yulius: disabled, not passed.
- **Not tested.** Whether any instance holds a production secret with hosts (the migration
  changes nothing where none does; the audit line shows it where one did); the whole e2e
  suite (one page's file changed, and its file was run).
- **Review of slice 12, round 1** (head 3820558c5, request changes; the rule, the migration
  and the store's check held: "`may_use` is false for cd_prod for every caller", "No write
  path can make the pair cd_prod + hosts"). Three minor findings, in the reviewer's words:
  "Two docs still describe the old state"; "Two such call sites still get read-write"
  (the fetch and the listing by a remote's name); "Several integration assertions can no
  longer fail for the rule they claim to hold". Five nits. Answered: both documents; both
  sites, and a test that reads how each command reaches the origin from the launcher's own
  words; the comments of those assertions say what they show, and AC-74 no longer cites
  them for the proxy's rule; the doc comment a new test had taken from its neighbour; three
  stale comments; the audit line as a sentence with a name on the Secrets page; a command
  without a folder that takes from a sandbox's folder counted as one in it. Left as it is
  (the fifth nit): the store's compare-and-sets still let a production row keep hosts
  unchanged; no row has any, and the general rule they are part of (hosts change only
  against what the editor saw) is not this slice's.
- **Tests failing without their fix, after the review** (`mutations.log`, now 11 cases, each
  failed its test): the six above and: a production secret like any other to the proxy
  (the rule gone for every caller); a fetch, a listing and a clone allowed to write in the
  folder they take from; a command without a folder that takes from a sandbox's folder
  counted as none of a sandbox's.
- **The suites after it:** `2885 tests run: 2885 passed` (`slice12-suite2.log`); the secrets
  page's e2e file again, `30 passed`.
- **Gate after it.** `scripts/dev ci` at `19018a164`: exit 0; `4206 tests run: 4206 passed, 16
  skipped`; doctor, repository check, lint, the macOS compile, cargo-deny, cargo-shear and
  rustdoc passed.
- **Noted, not a defect:** a clone between two folders now copies the objects where it
  linked them (the source is read-only to the confined Git, and a link needs more); more
  disk and time for a playground's first clone of a large repository.
- **Review of slice 12, round 2** (head 5e30be1ed): "None open in own code"; approve. The
  commit after it ticks this slice in the Plan and adds this line; the reviewer was shown
  that delta.

### Slice 13 (2026-11-13)

- **Slice 12's delivery.** Pull request #1253 merged into `dev` at 0f04251be, whose own
  Yulius CI run passed.
- **The defect, by its test on the code before** (`target/audit/slice13/mutations.log`, the
  first case, which is the old line of `ci_rerun`):
  `local_ci::a_run_again_tests_a_pull_request_with_its_own_jobs` failed; the rerun ran the
  integration branch's `echo plain` and no `added` job.
- **Tests failing without their fix** (`target/audit/slice13/mutate.py`, 6 cases, each
  failed its tests): a run again given the integration branch's jobs; a commit's own jobs
  never applied; a commit that brings CI not tested; own jobs tested while the integration
  branch's configuration is broken; own jobs without the trusted security job; a branch
  behind given the jobs it still carries. The fourth and the sixth passed at first: no
  integration test held those two parts of the rule (the sixth had the unit test of the
  comparison only); AC-79 and AC-80 are their tests now.
- **The suites:** `2674 tests run: 2674 passed` for the app and the binary before the two
  added tests (`target/audit/slice13-suite1.log`); the trains', the playground's and the
  remote control's tests unchanged.
- **Gate.** `scripts/dev ci` at `f1279d914`: exit 0; `4209 tests run: 4209 passed, 16 skipped`;
  doctor, repository check, lint, the macOS compile, cargo-deny, cargo-shear and rustdoc
  passed.
- **Web UI.** Nothing under `web/` or `e2e/` changed; the e2e suite was not run.
- **Security scan.** Vidoc is switched off for this project in Yulius: disabled, not passed.
- **Not tested.** A run again of a merge train's run (refused, as before) and of a
  playground task's run (the rule is the same function; its checks' tests pass, none
  reruns one); a playground task that changes its jobs while `dev` has a scanning job of
  its own (the kept security job is tested for a pull request and for a train; the
  built-in scan is never on in a playground); the reasons a run again gives when nothing
  tests the commit.
- **Review of slice 13, round 1** (head d733a7edd, request changes; the scheduler, the train
  and the remote control "behave exactly as before, and the `ci_rerun` fix does what it
  claims"). One minor finding in own code, in the reviewer's words: "A playground task that
  fixes a broken `dev` yulius.toml now merges with no test at all." Answered by the rule
  itself (`Head::Merged`, Decisions) and AC-82. Four nits answered: the Decisions sentence
  about the playground's merge base; the reasons a run again gives; two sentences of
  `docs/changes.md` and `docs/api.md`; a playground test where a task changes a job
  (AC-81). A sixth point is D5's (the verdict of a head that brings its own jobs before
  any run exists) and stays with it.
- **Tests failing without their fix, after the review** (`mutations.log`, 7 cases, each
  failed its tests): the six above, and a merge not tested while the integration branch's
  configuration is broken.
- **Gate after it.** `scripts/dev ci` at `70fd1b606`: exit 0; `4211 tests run: 4211 passed, 16
  skipped`; doctor, repository check, lint, the macOS compile, cargo-deny, cargo-shear and
  rustdoc passed.

### Slice 14 (2026-11-13)

- **Slice 13's delivery.** Pull request #1254 merged into `dev` at e3c85c345. Its first
  Yulius CI run failed in one test of another area
  (`task_done::a_stop_settles_a_run_no_server_holds`, "the run's host still runs": the
  known flake of issue #1190, where this occurrence is noted); the run was repeated once
  at the same head and passed. Its review approved that head in its second round; the two
  nits left (a sentence about playgrounds, an arm that cannot be reached) are answered
  here.
- **The defects, by their tests on the code before** (`target/audit/slice14/mutations.log`,
  the first two cases, which are the old code): with the promotion pull request's check
  gone, `delivery::standard_flow_merges_on_green_checks_and_promotes_by_pr` failed; with
  the fast-forward's check gone,
  `delivery::product_flow_deploys_on_confirmation_then_fast_forwards_and_rolls_out` failed
  (it promoted past a failed run).
- **Tests failing without their fix** (`target/audit/slice14/mutate.py`, 7 cases, each
  failed its tests): a promotion pull request reading no checks; a fast-forward promotion
  reading none; an automatic promotion made to wait for a run of its own; a person's
  fast-forward going while the checks are pending; GitHub's checks deciding where Yulius
  CI ran; failing checks allowing what requires them; a commit that brings CI without
  checks before its first run.
- **Found before the review, by reading the callers:** the first shape of this slice held
  every fast-forward to the integration head's own verdict, which an automatic promotion
  right after a merge can never have (the commit is seconds old): a profile with
  `auto_promote` and `require_checks` would have stopped promoting. AC-87 is its test.
- **The suites:** `2680 tests run: 2680 passed` for the app and the binary
  (`target/audit/slice14-suite2.log`).
- **Gate.** `scripts/dev ci` at `35a35b810`: exit 0; `4213 tests run: 4213 passed, 16 skipped`;
  doctor, repository check, lint, the macOS compile, cargo-deny, cargo-shear and rustdoc
  passed.
- **Web UI.** Nothing under `web/` or `e2e/` changed; the e2e suite was not run.
- **Security scan.** Vidoc is switched off for this project in Yulius: disabled, not passed.
- **Not tested.** A promotion in a project without Yulius CI whose GitHub checks fail
  (pending and unreadable are tested, as before); an automatic promotion after an
  environment's command, as distinct from its confirmation (the same call); the cost of
  the extra read in a project without CI.
- **Review of slice 14, round 1** (head acda02182, request changes; "The four call sites
  that were right before behave as they did, and the promotion checks are mostly right").
  One minor finding, in the reviewer's words: "the automatic promotion's pending exception
  covers more cases than the spec says, and one of its outcomes is untested". Answered by
  the reviewer's second way (Decisions): the rule is said as it holds, for every automatic
  promotion, and AC-88 shows a failed head stopping one. Two nits answered: CHK-003's
  wording, and a sentence on `auto_promote` in `docs/api.md`.
- **Tests failing without their fix, after the review** (`mutations.log`, 8 cases, each
  failed its tests): the seven above, and an automatic promotion reading no checks.
- **Gate after it.** `scripts/dev ci` at `ee36e7c2e`: exit 0; `4214 tests run: 4214 passed, 16
  skipped`; doctor, repository check, lint, the macOS compile, cargo-deny, cargo-shear and
  rustdoc passed.

### Slice 15 (2026-11-13)

- **Slice 14's delivery.** Pull request #1257 merged into `dev` (through a merge train of
  one, at c7626f165), its own Yulius CI run passed; its review approved the head in its
  second round, and its two cosmetic nits are answered here (a long line in
  `docs/api.md`, the refusal's reason asserted in AC-88's test).
- **The defect, by its test on the code before** (`target/audit/slice15/mutations.log`, the
  first two cases, which are the old code): without the pass's rule,
  `delivery::an_armed_merge_ends_with_its_tasks_run` failed (a closed task's merge stayed
  armed and merged); with the start's old rule (`== Running`) it failed for the task
  waiting for capacity.
- **Tests failing without their fix** (`target/audit/slice15/mutate.py`, 5 cases, each
  failed its tests): a pass deciding whatever became of the task; a start restoring only a
  running task's merge; a start restoring every saved merge; a held run losing its armed
  merge; a closed task keeping it.
- **What the rule showed in the tests:** eight delivery tests armed a merge for a task that
  had a live run in the store and was not running; their fixtures (`live_agent`,
  `ArmedTask::new`) now make the task running, as a live run is.
- **The suites:** the app's, the domain's and the binary's with the rule: all passed but
  nine `workflow_limit`/`task_done` tests that timed out on the fake agent of before the
  merge of `dev` (rebuilt with `cargo build -p fake-agent`, then 9 of 9 passed; the gate
  builds it itself).
- **Gate.** `scripts/dev ci` at `9bf5df124`: exit 0; `4249 tests run: 4249 passed, 16 skipped`;
  doctor, repository check, lint, the macOS compile, cargo-deny, cargo-shear and rustdoc
  passed.
- **Web UI.** Nothing under `web/` or `e2e/` changed; the e2e suite was not run.
- **Security scan.** Vidoc is switched off for this project in Yulius: disabled, not passed.
- **Not tested.** A member of a merge train under test whose task is closed meanwhile (the
  pass ends its armed merge by the same line; the train's own handling of a member that
  left is as before); a playground's armed merge for a closed task (the same function).
- **Review of slice 15, round 1** (head 3c263ef00, request changes). Two minor findings in
  own code, in the reviewer's words: "a run that is only held can pass through `Queued`,
  and the rule treats that as ended" (capacity back while every slot is taken; a task
  revived by a host move) and "a merged pull request is reported as 'dropped'" (a person's
  merge does not clear the armed slot). Both answered (ARM-001, ARM-002, AC-90). A third,
  from before this slice with a new effect, is finding D8 of the ledger: a task with an
  armed merge can be grouped Done, and archiving it there ends the merge unseen; it goes
  with the lifecycles (the grouping is theirs). Three nits answered: the long line of
  `docs/api.md`, the reason's sentence asserted in AC-88's test, two doc sentences.
- **Tests failing without their fix, after the review** (`mutations.log`, 7 cases, each
  failed its tests): the five above with the predicate's cases split (held, queued,
  closed), and a merged pull request of an ended task said to be dropped.
- **Gate after it.** `scripts/dev ci` at `9eec2f02b`: exit 0; `4249 tests run: 4249 passed, 16
  skipped`; doctor, repository check, lint, the macOS compile, cargo-deny, cargo-shear and
  rustdoc passed.
- **Not tested, besides the list above:** the path that queues a held run (capacity back
  with every slot taken) end to end; the test sets the state.

### Slice 16 (2026-11-13)

- **Slice 15's delivery.** Pull request #1263 merged into `dev` (through a merge train of
  one, at 7c20df998), its own Yulius CI run passed; its review approved the head in its
  second round, and its three nits are answered here (the long line of `docs/api.md`, a
  long line of this spec, and the order in `merge_armed_task`: of an ended run only what
  became of its pull request is asked).
- **Tests failing without their fix** (`target/audit/slice16/mutate.py`, `mutations.log`, 3
  cases, each failed its test): a failure inside Yulius answered with 502; a table of its
  own in the loopback listener's routes; one in the remote MCP endpoint's. The third
  passed at first: the source test looked only for arms that begin with `AppError::`, and
  the type can be named by its path; it reads any arm that names it now.
- **The suites:** `2739 tests run: 2739 passed` for the server, the app and the binary
  (`target/audit/slice16-suite1.log`): hooks, the credential helper, `yulius wait` against
  a listener that answers 500, the remote MCP endpoint.
- **Gate.** `scripts/dev ci` at `de1cb8ba0`: exit 0; `4280 tests run: 4280 passed, 17 skipped`;
  doctor, repository check, lint, the macOS compile, cargo-deny, cargo-shear and rustdoc
  passed.
- **Web UI.** Nothing under `web/` or `e2e/` changed; the e2e suite was not run.
- **Security scan.** Vidoc is switched off for this project in Yulius: disabled, not passed.
- **Not tested.** A failure inside Yulius through the listener itself (it needs a store that
  fails on demand; the mapping and the absence of another are what is tested, and
  `yulius wait`'s reading of a 500 is); an outside client of the loopback listener that
  reads its 400 or 502 (none in this repository).
- **Review, round 1** (a fresh session, read-only, head `2a5f2d4ec`): request changes. One
  finding in this slice's code, minor: "A behaviour change with no test" (the condition
  `ended.is_none() &&` in `merge_armed_task`: an ended run is not asked where its branch
  is). `delivery::an_armed_merge_ends_with_its_tasks_run` now commits past the armed head
  before the task is closed and again before the pull request is merged by hand, and
  expects "run ended (closed)", and "merged" without "dropped". One finding from before,
  minor: "an Internal error is answered without a log line" on the routes of the agents'
  own clients, where its 500 now ends a `yulius wait`; `bare_status` logs it as the public
  API does and is what `routes/internal.rs`, `routes/mcp.rs` and `routes/secrets.rs` call.
  The nits are answered: the source test's comment says what it reads; STS-001 names the
  guest link's 410; `docs/api.md` speaks of the routes the agents' clients call;
  `docs/changes.md` says five kinds; the line of this spec is joined; the `unavailable`
  code that nothing produces is P15 in the ledger.
- **Tests failing without their fix, after the review** (`mutations.log`, 4 cases, each
  failed its test): the three above and "an ended run's moved branch is the reason it
  gives".
- **Gate after it.** `scripts/dev ci` at `c65fb7087`: exit 0;
  `4280 tests run: 4280 passed, 17 skipped` (`target/audit/slice16/gate2.log`).
- **Review, round 2** (the same session, head `93088ebf3`): approve; every finding of round
  1 answered, three nits of wording in this spec (a line's spaces, the order of AC-94 and
  AC-95, and what the test expects of "dropped"), answered in the commit after the merge.
- **Delivery.** Pull request #1265 merged into `dev` at `93088ebf3`, whose own Yulius CI
  run passed.

### Slice 17 (2026-11-13)

- **Tests failing before the fix** (`target/audit/slice17/before.log`): the three new
  tests failed on the code as it was: the session was moved with the look unanswered
  ("running (runs: Killed Some("switched"), Live None)"), and both switches whose old
  session did not end answered `Ok(())` with a second run started.
- **Tests failing without their fix** (`target/audit/slice17/mutate.py`, `mutations.log`, 6
  cases, each failed its test): a look without an answer read as no background work; a
  host that cannot be terminated taken for ended; the switch's intent left in the task's
  slot (the session that left by itself never closed its task); the task left switching;
  no cooldown; the run that had to be killed followed by a new one.
- **The suites:** `2714 tests run: 2714 passed, 17 skipped` for the app and the binary
  (`target/audit/slice17/suite1.log`), before the last additions to one test; the gate ran
  after them.
- **Gate.** `scripts/dev ci` at `092ff5309`: exit 0; `4283 tests run: 4283 passed, 17 skipped`
  (`target/audit/slice17/gate.log`); doctor, repository check, lint, the macOS compile,
  cargo-deny, cargo-shear and rustdoc passed.
- **Web UI.** Nothing under `web/` or `e2e/` changed; the e2e suite was not run.
- **Security scan.** Vidoc is switched off for this project in Yulius: disabled, not passed.
- **Not tested.** A real session that ignores its end for a minute (the test host drops
  the request and the wait is 300 ms); a probe that times out by itself (the test host
  answers "not known" at once); the restart on a newer Claude Code meeting a session that
  cannot be ended (it shares `switch_timed_out`; its cooldown differs and it says nothing
  of a switch); the host reporting the exit while it is killed, with the real host (the
  test makes the session leave at that moment instead); the end of a run recorded twice
  (Decisions).
- **Review, round 1** (a fresh session, read-only, head `44da4c6e2`): request changes. One
  major finding in this slice's code: "The outcome of a timed-out switch still depends on
  who records the run's end first, and two orders leave the task `switching` with no run"
  (the run's own exit taken at the deadline; the host reporting the exit while it is
  killed; and a run that exits before a refused kill, left `running` with no run). The
  slot now says the run was given up, every recorder reads it, a run whose end was taken
  is waited for, and four tests make the orders with pause points (AC-99, AC-100). Two
  minor: SPEC-002 names `stopped(switch_timeout)` for this and the slice rested the task
  as `stopped_lost` (it is `stopped_switch_timeout` now, and SPEC-002's row names the run
  that cannot be ended); a `Done` task moved while idle rests stopped, not closed (not
  changed: Decisions say why, with RBT-001's own test). Nits answered: no notice of a
  switch for a restart on a newer Claude Code, and the flag's action is "Stop the task to
  try again."; a session kept for a look without an answer is logged, and the architecture
  says what a host whose look never answers means; SWT-001 names both looks; the pair of
  waits stays a pair (Decisions).
- **Tests failing without their fix, after the review** (`mutations2.log`, 11 cases, each
  failed its test): the six above as they are now, and: the reader's loss keeping its own
  reason; the slot emptied when the switch gives the run up; the exit of a given-up run
  not resting the task; a run that ended before a refused kill called one that could not
  be ended; a run whose end is being recorded not waited for.
- **The suites after it:** `3222 tests run: 3222 passed, 17 skipped` for the domain, the
  app and the binary (`target/audit/slice17/suite2.log`).
- **Gate after it.** `scripts/dev ci` at `ea43f41d6`: exit 0;
  `4287 tests run: 4287 passed, 17 skipped` (`target/audit/slice17/gate2.log`).
- **Review, round 2** (the same session, head `188a85514`): request changes. One major
  finding: "A run that ends by itself after the switch has made the task Switching, but
  before the switch puts its sender in the slot, still leaves the task switching with no
  run." Such an end now ends the task as from `running` and the switch gives way
  (AC-101). One minor: after a refused kill the run was asked about before an end that
  was being recorded was written down, which left "a stale Needs-your-action flag and
  notice"; the switch waits for that record now (AC-102). Its round-1 finding on a `Done`
  task it withdrew ("the answer holds"). Nits answered: the status's text says
  "Restarting the session took too long" (a restart on a newer Claude Code ends there
  too); SWT-002 says that a stop after the give-up decides nothing about the label; the
  words are "the session did not end in time"; the one test that assumes a time says so.
- **Tests failing before those fixes** (`target/audit/slice17/before3.log`): both new
  tests failed, one with "the session could not be ended, so it was not switched" for a
  run that had ended, one with "it did not end in time" for a session that had left.
- **Tests failing without their fix, after round 2** (`mutations3.log`, 13 cases, each
  failed its test): the eleven above, a run that ends by itself under a switch that has
  not claimed it left with no way out, and a refused kill not waiting for an end being
  recorded. A fourteenth, asking RBT-001's rule at the exit of a given-up run, changed no
  outcome (a task's activity is cleared when it becomes `switching`) and is not counted.
- **The suites after it:** `3224 tests run: 3224 passed, 17 skipped`
  (`target/audit/slice17/suite3.log`).
- **Gate after it.** `scripts/dev ci` at `80e18ff9f`: exit 0;
  `4289 tests run: 4289 passed, 17 skipped` (`target/audit/slice17/gate3.log`). The web
  UI's one changed line is the text of a status; the e2e suite was not run.
- **Review, round 3** (the same session, head `cc9a2d507`): request changes. Both
  findings of round 2 fixed; one minor finding in this slice's code: at a server start a
  task left `switching` whose run exited while Yulius was down "rests Closed for a clean
  exit, or stopped_killed", where the start's sweep had rested it as lost to the restart
  and listed it for a host move. The start rests it first now (AC-103;
  `host_move::a_task_left_switching_whose_run_ended_is_lost_to_the_restart` failed before:
  "left: Closed, right: Stopped(ServerLost)"). Nits: the stale mark after one order on the
  kill's path (left, Decisions); the status's short label and the run's "move timed out"
  beside the new text (left: they name the same thing); the e2e suite not run locally for
  one changed text (Yulius CI runs it on the pull request's head).
- **Tests failing without their fix, after round 3** (`mutations4.log`, 15 cases, each
  failed its test): the thirteen above, a start recording the end of a switching task's
  run as a session that left, and that task missing from the tasks a move brings back.
- **Gate after it.** `scripts/dev ci` at `b41c5d11f`: exit 0;
  `4290 tests run: 4290 passed, 17 skipped` (`target/audit/slice17/gate4.log`).
- **Review, fourth look** (the same session, head `5690bb786`): approve; "Nothing of
  severity minor or above remains in own code." Two nits, answered in the commit after
  the merge: the start lists a switching task as lost only when its change was made, and
  Decisions give the review's word as it was.
- **Delivery.** Pull request #1268 merged into `dev` at `5690bb786`, whose own Yulius CI
  run passed.

### Slice 18 (2026-11-14)

- **Tests failing before the fix** (`target/audit/slice18/before.log`): the failed sign-in
  left `("error", "the sign-in ended without producing a token (exit code 2)")` where
  `("relogin", Claude's reason)` was expected, and the usage round turned the `error`
  subscription `Ok`. The e2e test failed on the page's old rule
  (`target/audit/slice18/e2e-old.log`: "Usage read again for 1 subscription." never shown).
- **Tests failing without their fix** (`target/audit/slice18/mutate.py`, `mutations.log`, 6
  cases, each failed its test): a failed sign-in turning a refused subscription into an
  error; replacing its reason; the usage round asking one in error; a reading making one
  in error usable; the e-mail round asking one in error; and asking one that needs a
  sign-in.
- **The suites:** `2723 tests run: 2723 passed, 17 skipped` for the app and the binary
  (`target/audit/slice18/suite1.log`), before the e-mail round's test was added; the gate
  ran after it.
- **Gate.** `scripts/dev ci` at `e45fb4f59`: exit 0; `4293 tests run: 4293 passed, 17 skipped`
  (`target/audit/slice18/gate.log`); doctor, repository check, lint, the macOS compile,
  cargo-deny, cargo-shear and rustdoc passed.
- **Web UI.** `cd e2e && npx e2e run` at `e45fb4f59`: exit 0; `Test Files 84 passed | 1 skipped
  (85)`, `Tests 1022 passed | 1 skipped (1023)` (`target/audit/slice18/e2e-full.log`).
- **Security scan.** Vidoc is switched off for this project in Yulius: disabled, not passed.
- **Not tested.** A subscription Claude really refuses (the test writes `relogin` with
  Claude's words into the store); a row left `error` with a credential by the old code on
  the running instance (the test writes one).
- **Review, round 1** (a fresh session, read-only, head `1529f8d10`): request changes. One
  major finding in this slice's code: the sign-in's dialog reads the subscription's
  error, so for a refused subscription whose sign-in fails "It gives the refusal as the
  reason the sign-in failed", and the sign-in's own reason "is now stored and shown
  nowhere". The reason is in the sign-in's `exit` frame now and the dialog shows that
  (AC-104, AC-108). Minor, answered: `docs/api.md` names the `relogin` case; the mock does
  what the server does for a failed sign-in and for a reading; the usage-round block of
  one test, which could not fail, is gone with its clause of AC-104; the two helpers in
  `usage.js` no longer sit under another constant's comment. Nits answered: the
  administration page uses `works` at its three places; the rule's comment says "the
  background rounds". From before, taken on the way: a failed sign-in's write takes the
  lock of the other status writes (not tested: the window is between a read and a write
  of one function), and the start's sweep lists a task as lost only when its change was
  made.
- **Tests failing without their fix, after the review** (`mutations2.log`, 8 cases, each
  failed its test): the six above, a viewer watching a failed sign-in not told why, and
  one who opens it later not told why. The new e2e test failed against the dialog's old
  code (`target/audit/slice18/e2e-old2.log`: expected text containing "The sign-in ended
  without producing a token").
- **Gate after it.** `scripts/dev ci` at `c8f2643e6`: exit 0;
  `4293 tests run: 4293 passed, 17 skipped` (`target/audit/slice18/gate2.log`). The run
  before it at the same head failed in one test this change does not touch,
  `mcp_bridge::tests::a_call_held_through_an_outage_is_answered_once_yulius_is_back`
  (`gate2-flake.log`): its helper gives up a port and expects nobody to take it for
  400 ms; another test's server took it. Reported as issue #1269; the gate was run again.
- **Web UI after it.** `cd e2e && npx e2e run` at `3f6edfc0f` (the head but for a lint in a
  Rust test): exit 0; `Test Files 84 passed | 1 skipped (85)`,
  `Tests 1023 passed | 1 skipped (1024)` (`target/audit/slice18/e2e-full2.log`).
- **Review, round 2** (the same session, head `f5cd00867`): approve. Three nits, answered
  in the commit after the merge: the dialog no longer asks for the subscriptions when a
  sign-in failed; the mock's ended sign-in tells a later viewer its cause too; the mock
  answers a reading of a subscription that needs a sign-in as the server does.
- **Delivery.** Pull request #1271 merged into `dev` at `4f8ed0dd9` (the approved head
  with `dev` merged into it by Yulius for its merge train), whose own Yulius CI run
  passed.

### Slice 19 (2026-11-14)

- **Tests failing before the fix** (`target/audit/slice19/before.log`): the task that was to
  resume past its limit came out of the queue as a plain resume (the checks of a resume
  past a limit failed), and the task continued on `b-second` ran on `a-first`, the pool's
  choice.
- **Tests failing without their fix** (`target/audit/slice19/mutate.py`, `mutations.log`, 8
  cases, each failed its test): the start not kept; a queued task started with the
  queue's plain start; the pool's own choice kept too (the task then ran on the
  subscription that had filled up); a person's choice not kept; a kept start losing its
  prompt; a kept resume past a limit coming back a plain resume; a start of an earlier
  wait read again; a start kept for a task that is not queued.
- **The suites:** `2854 tests run: 2854 passed, 17 skipped` for the app, the binary and
  the store (`target/audit/slice19/suite1.log`), before the unit test and the restart in
  one test were added; the gate ran after them.
- **Gate.** `scripts/dev ci` at `51d4412ff`: exit 0; `4315 tests run: 4315 passed, 17 skipped`
  (`target/audit/slice19/gate.log`); doctor, repository check, lint, the macOS compile,
  cargo-deny, cargo-shear and rustdoc passed.
- **Web UI.** Slice 19 changes nothing under `web/` or `e2e/`; the branch carries the
  nits of slice 18's last review there (the sign-in dialog, the mock), and the suite ran
  after the review of this slice (below).
- **Security scan.** Vidoc is switched off for this project in Yulius: disabled, not passed.
- **Not tested.** A person's held prompt through the queue with a real pause (the kind and
  the prompt are tested in the store and in the round trip, the path with a `limit` pause
  and with a person's Continue); a start that waits for a supertask's or an update's hold
  (the same branch as the session limit); the migration on the running instance's
  database (a version-56 database made in the test opens at 57).
- **Review, round 1** (a fresh session, read-only, head `6794d1eb8`): request changes.
  Four minor findings in this slice's code. "The kept start is never cleared": the status
  change out of the queue removes it now. Queueing and keeping were "two separate store
  calls": one transaction (`queue_task`). "A chosen subscription that cannot take the
  task when the slot frees now makes the task end `failed`": it is given up, said in the
  terminal, and the pool places the task (AC-112). The Evidence line on the web UI was
  wrong for the branch: corrected, and the suite run. One major from before: "`resume_paused`
  ends the pause before calling `start_run`", so a start the full queue refused left the
  task with nothing to start it again; the start ends the pause where it is made or
  queued (QST-002, AC-113). Nits: the edge of a person's move that goes on from a waiting
  task is in Decisions; the store test writes unreadable rows by hand and asks the
  database for its version; SPEC-002's table row and the mock's comment are right.
- **Tests failing before those fixes** (`target/audit/slice19/before2.log`): the task whose
  chosen subscription had filled up read `Failed(Precondition)`, and the paused task's
  pause was gone after the round that met the full queue.
- **Tests failing without their fix, after the review** (`mutations2.log`, 13 cases, each
  failed its test): the first seven above as they are now; a chosen subscription that
  cannot take the task asked for all the same; the pause ended before the start is tried;
  a start that waits in the queue, and one that is made, leaving the pause open; a task
  that no longer reads its status given a start; a task that leaves the queue keeping its
  start in the row; a start of an earlier wait read again.
- **The suites after it:** `2857 tests run: 2857 passed, 17 skipped`
  (`target/audit/slice19/suite2.log`).
- **Gate after it.** `scripts/dev ci` at `426c3c789`: exit 0;
  `4317 tests run: 4317 passed, 17 skipped` (`target/audit/slice19/gate2.log`).
- **Web UI after it.** `cd e2e && npx e2e run` at `426c3c789`: exit 0;
  `Test Files 84 passed | 1 skipped (85)`, `Tests 1023 passed | 1 skipped (1024)`
  (`target/audit/slice19/e2e-full.log`).
- **Review, round 2** (the same session, head `326e89925`): approve. Four nits, answered
  in the commit after the merge: the pause ends once the launch has a subscription, so a
  launch that finds none keeps an open pause as it was (a prompt a hook carried into a
  switch is not dropped); a queueing that lost to a concurrent change does nothing; a
  start the full queue refused reads as waiting in the pool's round; the change log names
  the two behaviours of the review's round. Moving the pause's end showed one more case,
  which `supertask::a_held_capacity_resume_keeps_its_pause` caught: a launch that fails
  before it has a subscription must end the pause too, and does (the task is `failed`).
- **Delivery.** Pull request #1272 merged into `dev` at `326e89925`, whose own Yulius CI
  run passed.

### Slice 20 (2026-11-14)

- **Tests failing before the fix** (`target/audit/slice20/before.log`): after the restart
  the hand-over restarted nothing ("timed out waiting for the resumed run"), and no run
  directory held the mark of a goal still to be typed.
- **Tests failing without their fix** (`target/audit/slice20/mutate.py`, `mutations.log`, 9
  cases, each failed its test): a run with personal servers leaving no mark; the restarted
  server not reading it; a goal still to be typed leaving no mark; the restarted server
  not watching it; its watch saying the launch goal's sentence; a typed goal that was
  taken keeping its mark; a recorded goal's mark staying after the watch; and, for the
  pause's end moved after slice 19's review, a launch that has its subscription and a
  launch that fails each leaving the pause open.
- **The suites:** `2750 tests run: 2750 passed, 17 skipped` for the app and the binary
  (`target/audit/slice20/suite1.log`), before the fourth test was added; the gate ran
  after it.
- **Gate.** `scripts/dev ci` at `05ba32d9b`: exit 0; `4321 tests run: 4321 passed, 17 skipped`
  (`target/audit/slice20/gate.log`); doctor, repository check, lint, the macOS compile,
  cargo-deny, cargo-shear and rustdoc passed.
- **Web UI.** Nothing under `web/` or `e2e/` changed since the suite's last run
  (`426c3c789`, slice 19); it was not run again.
- **Security scan.** Vidoc is switched off for this project in Yulius: disabled, not passed.
- **Not tested.** A real Claude Code session and a real update of the instance (the
  restart is a second server on the same data); a launch that cannot write the personal
  servers' mark; the two nits of slice 19 that have no order a test can make (a queueing
  that lost to a concurrent change; the pool's round reading a refused start as waiting).
- **Review, round 1** (a fresh session, read-only, head `3cd136696`): request changes. One
  major finding in this slice's code: the watch did not wait for the session's opening
  turn, so "the flag is gone and the task reads Your turn with no goal" once that turn's
  Stop ran. The mark says now whether the session opens with a turn, the restarted server
  gives the run that turn back and the watch starts at its end (AC-115 asks that the flag
  is not raised inside the turn). Four minor: the mark was written after the spawn (now
  before it, like its siblings); the trust dialog's branch told the person the task would
  start by itself (for a typed goal it tells them to type it); the refusal in the
  hand-over test could not fail without the fix (the server is turned off before the
  restart now, so only the run's record refuses); a hand-over cut by the restart left the
  previous owner's servers in the new holder's run (the reattached run is restarted,
  AC-117). Nits answered: a comment; an unreadable mark is logged.
- **Tests failing without their fix, after the review** (`mutations2.log`, 13 cases, each
  failed its test): the nine above as they are now, and: a reattached run that holds
  another person's servers left running; the opening turn not given back to a reattached
  run; the watch not waiting for the opening turn; the mark not saying that the session
  opens with a turn.
- **The suites after it:** `2752 tests run: 2752 passed, 17 skipped`
  (`target/audit/slice20/suite2.log`).
- **Gate after it.** `scripts/dev ci` at `c37c16ea1`: exit 0;
  `4322 tests run: 4322 passed, 17 skipped` (`target/audit/slice20/gate2.log`).
- **Not tested, after it.** The trust dialog's branch of the watch for a typed goal (it
  needs a session that shows the dialog across a restart); a session whose opening turn's
  Stop was held for capacity at the restart (Decisions).
- **Review, round 2** (the same session, head `5df74814d`): approve. Three nits, answered
  in the commit after the merge: the edge of an opening turn that ended while Yulius was
  down is in Decisions; the restart of a reattached run that holds another person's
  servers waits for the transcript's replay (at most 30 s), so that a run at a usage limit
  is resumed past it; `docs/architecture.md` names the opening turn given back and that
  restart.
- **Delivery.** Pull request #1274 merged into `dev` at `e310b08b6` (the approved head
  with `dev` merged into it by Yulius for its merge train), whose own Yulius CI run
  passed.
