---
yulius-task: 1f58ed73-02d4-4c49-8c66-dbf323eb4a3f
status: done
---

# SPEC-624: Notices that replace and expire: a key per notice, newest wins, an expiry

## Goal

A person's bell, Telegram chat, digest and project notices stop collecting stale and repeated
items. Every notice Yulius raises for a person carries a **key** and an **expiry**. A newer
notice with the same key replaces the older one for that person and project, and an expired
notice goes away by itself. A notice that waits for a person's decision never expires silently.
The owner approved this on 22 Nov 2026 after reviewing the inbox of iTermX, a separate tool. Its
design was read as a reference and none of its code is used. Task #614 ("Script watches") posts
its output as notices keyed `watch:<name>` and builds on the API defined here.

## Context

What people see today (inventory at `2a6e07423`):

- **The bell** (`web/assets/notifications.js`) builds entries in the browser from the event
  stream and keeps the last 100 in localStorage. It already has a local `key`, but uses it
  unevenly. A task's Needs you entries are keyed per episode (`task:<id>:<episode>`), so each
  new red CI run of the same pull request adds another entry. Control-taken, automation and
  "project removed" entries have no key at all. No entry ever expires: they stay until the
  100-entry cap or until someone presses Clear.
- **Telegram** (`yulius-app/src/notify.rs`) sends one message per change and dedups only in
  memory. A task that goes from Needs you to Done leaves both messages in the chat.
- **The digest** (`yulius-app/src/digest.rs`) is rebuilt every day from task state. The bell
  keys each one as `digest:<id>`, so yesterday's entry stays beside today's.
- **Project / Status notices** (`GateNotice`, `ci/gate_history.rs`, SPEC-612) are computed on
  each read for an integration failure that has no repair worker. Nobody is told when one
  appears, and they carry no key.
- **Subscription problems** are derived twice, from `account.updated` in the browser and in the
  notifier. Neither knows when a usage limit ends.
- **Agent `needs_you` requests** (`tasks.needs_you`) already keep one open request per task: a
  new request replaces the previous one. Each is a decision a person owes, so it must not expire.
  They stay as they are and are covered here only as task-state notices (NTC-008).

Facts and assumptions:

- No server table holds person-facing notices. Migration 0091 (reserved) adds one.
- Telegram's `deleteMessage` lets a bot delete its own messages for 48 hours (Bot API). Older
  messages stay; a failed delete is logged and does not hold up the new message.

Non-goals: an agent tool that posts notices (#614 posts through `App::post_notice`); grouping
or snoozing; changing who a notice reaches (the event bus rule stays the only rule).

## Requirements

- **NTC-001 Key.** A notice key is 1 to 120 characters, from ASCII letters, digits and
  `:._/@#-`. Anything else is refused with the rule in words. Keys are scoped by project
  (`None` for an instance-wide notice) and by recipient (one person, or everyone the project's
  visibility lets see it). Yulius's own keys use the prefixes in the table below; `watch:` is
  reserved for #614.
- **NTC-002 Expiry.** An expiry is `never`, a duration `<n>m`, `<n>h` or `<n>d` (1 minute to 90
  days), or an RFC 3339 time in the future. It resolves against the server's clock at posting
  time to an absolute `expires_at`, or none. Without an expiry a notice gets its kind's default.
- **NTC-003 Newest wins.** Posting a notice whose (project, recipient, key) matches a stored one
  replaces it in one transaction. The new notice gets a new id, comes back unread for everyone, and
  is published to the people it reaches. No two stored notices share a scope and a key.
- **NTC-004 Expired is hidden.** A notice whose `expires_at` has passed is left out of every read:
  `GET /api/me/notices`, the person tools `notices_list`, the digest and the bell. The bell hides
  it at its time without reloading. The background tick deletes expired notices within one tick
  (60 s), and keeps at most 200 per (project, recipient). The cap drops the oldest notices that
  wait for no decision first, and pages hear of each one it drops. A read decides which projects
  the reader sees before it applies its limit, so notices they cannot see never crowd theirs out.
- **NTC-005 A decision never expires silently.** A notice posted with `decision: true` (it waits
  for a person to decide or act) has no expiry. Posting it with an expiry is refused, never
  shortened, so a caller learns of the conflict. It leaves only when its producer removes or
  replaces it, or when the person dismisses it. Yulius's own decisions are open `needs_you`
  requests, requests for control, browser action requests and a subscription that needs to sign
  in again. Each one expires never, and its entry ends when the request ends.
- **NTC-006 Remove and dismiss.** A producer removes a notice by its scope and key; open pages
  drop it. A person dismisses a notice for themselves (`DELETE /api/me/notices/{id}`,
  `notice_dismiss`), and others still see it. A replacement brings it back. Dismissing changes
  the person's own state: a `read` token may not do it, and the call is audited.
- **NTC-007 Channels.** A posted notice reaches a person's open pages (`notice.posted`,
  `notice.removed`) and their Telegram chat through the notifier's existing rules (reach, the
  person's choices by the notice's category, redaction, the rate bound). In Telegram, a keyed
  message replaces the bot's earlier message with the same scope and key in that chat. The bot
  deletes the old message, best effort, after the new one is sent. A chat's place for a notice is
  its project, recipient and key, so a notice for one person and a project-wide one with the same
  key keep their own messages. The bot forgets a chat's messages when the chat is unlinked or
  changes hands, or when the bot is replaced, and forgets any message older than 48 hours.
  Keys starting with `task:`, `control:`, `needs-you:` or `digest` are Yulius's own derived
  notices and cannot be posted.
- **NTC-008 Task-state notices are keyed per task.** A task's Needs you, Done and failed notices
  use the key `task:<task id>` in the bell and in Telegram. A newer state of the same task, or a
  newer red CI run of its pull request, replaces the earlier entry or message.
- **NTC-009 Digest.** The digest lists the person's live posted notices (newest first, at most
  20) with their keys, and leaves out expired and dismissed ones. A digest is keyed `digest`, so
  today's bell entry and Telegram message replace yesterday's. Digests stored before this change
  still load.
- **NTC-010 Yulius's own repeated notices use keys** (table below):
  - A subscription problem is posted as `account:<id>` and removed once the subscription is
    usable again. A usage limit expires at its reset when the reset is known.
  - An integration failure is posted to the project's people as `gate:<test>` while no task
    works in the project, or while every task that does has been unable to take the repair for
    10 staging ticks in a row (`GATE_GRACE_TICKS`, about ten minutes). The notice holds the
    failing test, its first failing revision and the merge that showed it, and a link to the CI
    page. At most 20 failures get a notice each, and the rest share `gates:more`. A notice is
    removed once the failure is repaired or a worker can take the repair again. Within the grace
    period a notice already told stays and no new one is posted, so neither a worker that is only
    momentarily busy nor a restart (which starts the count again) retells it. A green branch
    ends the count.
    `GateNotice` carries its `key`.
  - A subscription's notice follows its stored state. `account.updated` updates it at once,
    and the background tick also catches a status written without one, and removes the notice
    of a subscription that no longer exists. Both read the stored subscription under one lock. A repeat is compared with the texts as posting
    stores them, so a redacted or cut text is no news.
  - The prefixes `gate:` and `account:` belong to these producers, and `watch:` to #614.
    `App::post_notice` is internal: no person or agent posts through it.
- **NTC-011 Bell compatibility.** The bell gives every entry a key and an `expires` time.
  Entries already stored in a browser without `expires`, or with one that cannot be read, get
  their kind's default, counted from their own time. Entries already past it are pruned on load.
  The bell removes expired entries itself within ten seconds, panel open or not. On each load
  and resync it drops server notices the server no longer lists.

### Kinds, keys and default expiries

| Kind | Where it shows | Key (scope) | Default expiry |
| --- | --- | --- | --- |
| Task needs you: open `needs_you`, a question, a flag | bell, Telegram, digest (waiting) | `task:<task id>` (project) | never while it waits (decision); the next state replaces it |
| Red CI on a task's pull request (`pr_fail`) | bell, Telegram | `task:<task id>` (project): a newer run replaces | never while the task needs you; the next state replaces it |
| Task done | bell, Telegram | `task:<task id>` (project) | 3 days |
| Task failed or stopped abnormally | bell, Telegram | `task:<task id>` (project) | 7 days |
| Sandbox Docker needs you (shared cause) | bell, Telegram | `needs-you:<cause>` (project) | never while it waits (decision) |
| Request for control of your task | bell, Telegram | `control:<task id>` (project, owner) | never while asked (decision); 1 day once answered |
| Control taken / taken back | bell, Telegram | `control:<task id>` (project, person) | 3 days |
| Browser action request | bell | `browser:<tab>` (project) | never while asked (decision); 1 day once cleared |
| Automation run failed / started a task | bell | `automation:<id>` (project) | 3 days |
| Subscription needs to sign in again | bell, Telegram, `notices_list` | `account:<id>` (instance, admins) | never (decision); removed when usable |
| Subscription at its usage limit | bell, Telegram, `notices_list` | `account:<id>` (instance, admins) | its reset when known (none is posted once it has passed); else until usable; removed when usable |
| Subscription reports an error | bell, Telegram, `notices_list` | `account:<id>` (instance, admins) | until usable, then removed (told once, not each day) |
| Integration failure with no repair worker | bell, Telegram, Status, CI page, `notices_list` | `gate:<test>` (project), past 20 failures `gates:more` | never while unrepaired; removed when repaired or owned |
| End-of-day digest | bell, Telegram | `digest` (person) | 3 days |
| Update available / update failed | bell (superadmins) | `update` / `update-run:<id>` | 7 days |
| Connection trouble | bell | `connections:<trouble id>` | 1 day |
| Instance setup hints (recovery code, GitHub) | bell | `notice:<name>` | 30 days |
| Project removed or no longer visible | bell | `project-gone:<sandbox id>` | 1 day |
| Script watch output (#614) | bell, Telegram, digest, `notices_list` | `watch:<name>` (project) | the watch's expiry, else 1 day |
| Any other posted notice | as posted | the caller's key | 1 day |

## Plan

- [x] Notice keys and expiries parse and resolve in the domain
- [x] Notices are stored newest-per-key with dismissals and pruning (migration 0091)
- [x] Yulius posts, replaces, removes and lists notices for the people they reach
- [x] People read and dismiss their notices over HTTP and the person tools
- [x] Telegram replaces a keyed notice's earlier message in the chat
- [x] The digest lists live notices under a key and hides expired ones
- [x] Independent review of the notice core, every finding answered
- [x] The notice core's round-2 review notes are fixed
- [x] Subscription problems are keyed notices that end with the problem
- [x] Unrepaired integration failures are keyed notices for the project's people
- [x] The bell shows server notices and keys and expires every entry
- [x] Old browser notification history migrates with expiries
- [x] Product, API and architecture docs describe keyed, expiring notices
- [x] Independent review of the producers and the bell, every finding answered

## Acceptance criteria

- **AC-1** (NTC-003) Post `gate:quick::a` to project S twice with different titles. The store
  holds one notice with the second title and a new id. `GET /api/me/notices` returns one item. A
  person who dismissed the first sees the second.
- **AC-2** (NTC-003) The same key in another project, or for another recipient, is a separate
  notice.
- **AC-3** (NTC-004) Post a notice that expires in 30 minutes, then move the clock past its expiry.
  `GET /api/me/notices`, `notices_list` and the next digest leave it out, and the tick deletes it.
  A notice with `never` stays.
- **AC-4** (NTC-005) A decision posted with `expires: 30m` is refused. Without an expiry it is
  stored with none, and it is still listed after 90 days on the clock.
- **AC-5** (NTC-001, NTC-002) A key with a space, or with 121 characters, and the expiries `0m`,
  `91d`, `5w` and a past time are refused with a message.
- **AC-6** (NTC-007) With a linked chat: a keyed notice is sent, then a second one with the same
  key. The chat receives the second message and the first is deleted. A failing delete still
  sends the new message. An unkeyed message deletes nothing.
- **AC-7** (NTC-008) A task that enters Needs you and then Done gives one bell entry
  (`task:<id>`, kind ok) and one Telegram message. The Needs you message is deleted.
- **AC-8** (NTC-009) A digest built after a notice expired leaves it out and lists the live ones
  with their keys. A digest body stored before the change (without `notices`) still loads.
- **AC-9** (NTC-010) A subscription that reaches its limit with a known reset posts `account:<id>`
  that expires at the reset. When it is usable again the notice is removed. A second limit replaces
  the notice and does not add one.
- **AC-10** (NTC-010) A gate failure with no worker posts `gate:<test>` once across ticks. When a
  worker becomes available, or the test passes, the notice is removed.
- **AC-11** (NTC-011) A browser whose localStorage holds old entries without `expires` loads them
  with defaults and drops those already past them. A server notice and an older local entry with
  the same key become one entry, and an entry is hidden at its expiry while the page stays open.
- **AC-12** (migration) A store at the schema before 0091 (88 after dev's 0089) with digests and Telegram rows migrates. The rows
  are kept and the notice tables are empty and usable.

## Test plan

- Domain unit tests (`yulius-domain/src/notice.rs`): key and expiry parsing, bounds, defaults,
  and the decision rule (AC-4, AC-5).
- Store tests (`yulius-store/src/notices.rs`): replace by scope and key, expired hidden, prune,
  per-scope cap, dismissals cleared by a replacement (AC-1 to AC-3). The `lib.rs` migration test
  goes from 87 with rows to the current version (AC-12).
- Integration tests in the `yulius` binary (`tests/notices.rs`): post → `notice.posted` reaches
  only visible people, `GET`/`DELETE /api/me/notices`, `notices_list` and `notice_dismiss`
  through the person tools, the digest's notice section and the expired one left out (AC-1,
  AC-3, AC-8). Telegram replacement with the fake channel, including a failing delete
  (`tests/telegram.rs`, AC-6, AC-7). Subscription limit and gate failure producers (AC-9, AC-10).
- Chromium e2e (`e2e/tests/notices.e2e.ts`): server notices, replacement by key, hiding at
  expiry with the page's clock, migration of old localStorage entries (AC-7, AC-11).
- Mutation by hand: undo the replacement statement, the expiry filter and the decision refusal,
  and record which tests failed.

## Decisions

- **Notices are stored on the server, not derived in each client.** Telegram, the digest, MCP
  reads and every open browser need the same key and expiry. They can share them only if
  there is one record. Task state stays the truth for task-state notices: their key and expiry are
  rules (NTC-008), so a second record of each task's state would add nothing. ADR-0462.
- **The scope is (project, recipient, key).** iTermX keys globally per user. Yulius notices
  belong to projects, and two projects can both have `gate:quick::a`. A recipient of `None` means
  everyone the project's visibility lets see it, so one row serves a project's people and
  dismissals stay per person.
- **Telegram replaces by deleting the older message, after sending the new one.** Editing would
  not notify the person of a new state, and deleting first could leave the chat empty if the send
  failed. Deletion is limited to 48 hours by Telegram, and that limit is accepted.
- **A decision cannot be posted with an expiry.** The other options were to shorten the expiry
  silently, or to let it expire with a final "expired" message. Both still let a request vanish
  while it waits. Refusing makes the caller choose.
- **Defaults (table above).** One day for anything that reports, three days for good news and the
  digest, seven for failures and updates, never while something waits for a person. The 24-hour
  iTermX default fits reports. Failures stay longer because they are read after a weekend.
- **The bell keeps its local history.** It merges server notices into it under
  `notice:<project>:<key>`, so no browser loses what it had. Local kinds get a key and an expiry
  in the browser, following the same table.
- **Agent `needs_you` requests are not posted notices.** A task already holds one open request,
  which a new request replaces, and each is a decision. Both rules already hold, so they are kept
  in task state (NTC-008).
- **Two pull requests in this task.** The core (store, API, channels) comes first, and the
  producers and the bell second. Each pull request then stays one concern of reviewable size, and
  #614 can start on the first.

## Evidence

### First pull request: the notice core (steps 1 to 6)

- `scripts/cargo nextest run -p yulius -E 'binary_id(yulius::main)'` after merging `origin/dev`
  (with SPEC-622's gate notice UI): 2,613 passed, 24 skipped. The run includes the new
  `notices::` file (6 tests) and two new Telegram tests.
- `scripts/cargo nextest run -p yulius-app -p yulius-store -p yulius-domain -p yulius-contracts
  -p yulius-server`: 1,575 passed. This covers the store tests (replacement, scope, expiry and
  prune, decision refusal, scope cap, keyed Telegram swap), the migration test
  `keyed_notices_upgrade_keeps_digests_and_sent_messages` (AC-12), the digest compatibility test
  (AC-8) and the domain parsing tests (AC-5).
- `scripts/cargo clippy` on the six touched crates with `-D warnings`: clean. `scripts/dev quick`:
  the repository contracts and `cargo fmt --check` passed.
- Mutation by hand:
  - Without the delete of the replaced notice, `the_same_key_replaces_and_comes_back_unread`
    failed on the unique index.
  - With the expiry filter removed from `live_notices`, `an_expired_notice_is_hidden_then_pruned`
    failed.
  - With a decision that accepts an expiry, `a_decision_never_expires_and_refuses_an_expiry` and
    `notices::a_decision_never_expires_silently` failed.
  - Without the Telegram swap and delete, `a_tasks_newer_state_replaces_its_message` and
    `a_posted_notice_replaces_its_message_by_key` failed.
- Security scan: Vidoc is switched off for this project in Yulius, so the scan reports disabled,
  not passed.

### Review round 1 of PR #1705 (head `91a6017bb`): request changes, all answered

All findings were in this PR's own code. Each fix has a test, and each test was seen failing
without its fix:
- **Read limit before visibility (major).** Reads now pass the reader's reach (the projects that
  hold their notices and that they see, instance-wide unless a guest, admin-only for
  administrators) to the store before the limit. Covered by `a_readers_reach_comes_before_the_limit`
  and `notices::hidden_notices_never_crowd_out_a_readers_own` (200 newer notices in a hidden
  project), which fails with the SQL filter undone.
- **`notice_dismiss` under a read token (major).** `person_tools::PERSONAL_CHANGES` marks it as
  a change: a `read` token is refused and the call is audited `remote.tool` and
  `superagent.tool`. `notices_list` is now pre-allowed in the Cockpit. Covered by
  `notices::a_dismissal_is_a_change_of_ones_own`, which fails with the scope check undone.
- **`telegram_keyed` lifetime (major).** Its rows are cleared wherever `telegram_sent` is (unlink,
  a chat changing hands, a person moving chats, every link cleared with the bot), and rows older
  than 48 hours are pruned by the tick. Covered by `keyed_messages_end_with_their_chat_and_their_age`,
  which fails ("unlinked") with the clearing undone.
- **`Expiry::parse` panicked on a multi-byte last character (major).** It now splits at the last
  character's boundary. `5д`, `é`, `3dé` and `２h` are refused with the message.
- **Minor findings:**
  - The cap keeps decisions and publishes `notice.removed` for each notice it drops
    (`the_cap_keeps_decisions_and_names_what_it_dropped`).
  - The chat scope includes the recipient (the Telegram test posts a personal notice beside a
    project-wide one with the same key and sees no delete).
  - Posts and removals are stored and published one at a time (`POSTING`), so pages and chats
    hear them in store order. No test drives two posts at once; the lock makes that order hold
    by construction.
  - Texts are redacted before they are cut (`texts_are_redacted_whole_and_links_and_keys_checked`,
    with a secret straddling the title bound).
  - `NoticeKey::lossy` adds `#` and a hash when it alters or cuts a name
    (`a_lossy_key_never_merges_distinct_names`).
  - Dismissing someone else's personal notice is tested to be refused.
- **Nits:**
  - Links with `\` or control characters are refused.
  - Keys of derived notices are refused.
  - This spec says "one transaction", and its Plan boxes 1 to 6 are ticked.
- **Direction question.** A never-expiring notice, a decision or a gate failure, appears in each
  daily digest until it is removed. That is intended: the digest lists what still waits for the
  person, as it already does for open `needs_you` requests.

### PR #1705 merged

PR #1705 landed in `dev` at `6dda721e6` from head `8fbd51cb1`. Its own Yulius CI run
`742ea20e-d77f-4e04-a5b9-ad9fa6d0ff8d` passed. Review round 2 (Fable 5.2) approved `e7c04d9b3`.
Round 3 (Opus 5.5) approved the dev-merge delta at `8fbd51cb1`.

### Second pull request: producers and the bell (steps 8 to 13)

Round 2's notes, all own code and each with a test that failed without its fix:
- A `read` token is no longer served `notice_dismiss` (`scope_may_call`, the read-token test in
  `remote.rs`).
- A guest's own instance-wide notice is read
  (`a_readers_reach_comes_before_the_limit`), and a guest reaches `/api/me/notices` and its
  DELETE (`a_guest_reaches_only_their_playgrounds_work`, mock `guestMayReach`).
- `notice.removed` is published for a notice the cap drops and for a replacement that narrows
  `admin_only`, and titles are one line (`the_cap_and_a_narrowed_replacement_are_heard`).
- A report that the cap would drop at once is refused (`a_scope_full_of_decisions_refuses_a_report`).
- `notice_dismiss` keeps to its project (`a_dismissal_names_its_own_project`).
- The Cockpit audits a dismissal (`cockpit::the_superagents_notice_dismissal_is_audited`, which
  fails with the audit tied back to `Action::View`).
- `notice_dismiss` is listed among the changing tools in `manage_tools.rs`.
- The wording in `notify.rs`, the architecture document and AC-12 is fixed.

Producers:
- `notices::a_subscription_problem_is_one_keyed_notice` (AC-9) covers a limit that expires at
  its reset, no repost of the same state, a later reset that replaces it, removal once usable,
  and sign-in as a decision. It and `telegram::a_state_entered_before_the_start_is_not_told`
  fail with the repeat check (`says_the_same`) undone.
- `staging::an_unrepaired_failure_is_one_keyed_notice_until_it_passes` (AC-10) covers one
  `gate:flaky::result` notice that never expires, nothing new on the next tick, and removal
  after the fix passes. `GateNotice.key` is in `/api/status`.

The bell (`e2e/tests/notices.e2e.ts`, AC-7, AC-11). Each of its 4 tests fails against dev's
`notifications.js`:
- a newer server notice replaces the older one, and the server's removal drops it;
- an entry goes away at its expiry by the page's clock, and Clear dismisses the server's notice;
- a newer red run replaces the task's entry;
- old history gets its kind's default, and entries past it are dropped.

`shared-needs-you.e2e.ts` now expects the key `needs-you:<project>:<cause>`. Its first test
checks that an answered shared problem's entry expires a day later.

Runs at the second PR's head before review:
- `scripts/cargo nextest run -p yulius -E 'binary_id(yulius::main)'`: 2,645 passed, 24 skipped.
- `scripts/cargo nextest run` on the 6 library crates: 1,858 passed.
- `npx e2e run` (Chromium): 1,341 passed, 1 configured skip.
- `scripts/cargo clippy -D warnings` on the touched crates: clean.
- `scripts/dev quick`: passed.
- The Vidoc scan is switched off for this project: disabled, not passed.

Not tested here: a real Telegram chat (the fake Bot API stands in, and Telegram's 48-hour
delete limit is not exercised), and a desktop operating system's notification tag (Chromium's
`Notification` is a stand-in).

### Review round 1 of PR #1711 (head `974e24b7c`): request changes, all answered

All findings were in this PR's own code. Each fix has a test, and each test was seen failing
without its fix:
- **Gate notices flapped with momentary worker availability (major).** "Nobody repairs it" now
  means no task works in the project (`gate_workers`). Covered by
  `staging::a_momentarily_busy_worker_raises_no_gate_notice`, which fails on the worker
  assertion with the old `available` rule.
- **The repeat check compared raw texts with stored ones (major).** `stored_texts` is shared by
  posting and the producers. The same test fails at "a normalised name is no news" with the old
  check (a two-space, 300-character test name). `subscription_notices_follow_the_stored_state`
  covers a redacted error and a two-space label.
- **Status changes without `account.updated` (major).** The tick runs `sync_all_account_notices`,
  which also removes a deleted subscription's notice.
  `notices::subscription_notices_follow_the_stored_state` covers this through `set_account_state`
  and `delete_account`, with no event.
- **The bell kept notices the server removed (major).** Each load and resync reconciles with
  `GET /api/me/notices`. `notices.e2e.ts` "a notice the server removed while no page was open"
  fails without it.
- **A shared decision was settled from an unloaded list (major).** The bell settles it only
  from a loaded task list. The extended `shared-needs-you.e2e.ts` "seen from another project"
  case fails without the guard.
- **Minor findings:**
  - Gate notices are bounded: 20, plus `gate:more` (`staging::many_unrepaired_failures_are_bounded_notices`).
  - The store keeps the new notice on a time tie (`the_cap_keeps_the_new_notice_on_a_tie`).
  - The expiry timer has its own test, with the panel closed ("an entry goes at its expiry
    while the panel stays closed"), and fires within 10 s.
  - An unreadable `expires` counts as none.
  - An expired replacement saves and renders.
  - The gate text is aligned in the requirement: revision to 12 characters, and no "run it
    again" for readers.
- **Nits:**
  - Comments and the test-plan file name are corrected.
  - A late read after `destroy()` arms no timer.
  - Reserved prefixes, not changed: the spec now says `gate:` and `account:` belong to the
    producers and that `post_notice` is internal (no person or agent posts through it), so a
    refusal would only guard Yulius against itself. `sync_gate_notices` owning every
    project-wide `gate:*` notice is intended.
- **Not changed:** `mcp_config::probe::tests::silent_stdio_server_times_out_and_is_reaped`
  failed in the reviewer's run. Its file is outside this change, and it passed in this task's
  runs of the library crates (1,858 passed).
- Runs after the round-1 fixes:
  - `yulius::main`: 2,675 passed at first. 4 `forks::` tests (from dev, task #612) timed out
    against a stale `fake-agent` build; after `cargo build -p fake-agent` all 7 `forks::` tests
    passed.
  - Library crates: 1,874 passed.
  - Chromium: 1,344 passed, 1 configured skip.
  - Clippy `-D warnings` clean, and `scripts/dev quick` passed.

### Review round 2 of PR #1711 (head `bfdf9c659`): request changes, all answered

All findings except the last nit were in this PR's own code:
- **A limit's notice came back after its reset (major).** Once a known reset has passed,
  `account_notice` calls for no notice. An error, or a limit with no known reset, holds until
  the subscription is usable (`Expiry::Never`), so it is told once rather than each day; the
  spec table says so. Covered by the reviewer's probe, adopted as
  `notices::a_limit_past_its_reset_is_not_posted_again`, and by
  `a_lasting_subscription_problem_is_told_once`. Both failed before the fix.
- **The bell's reread could drop a live notice (minor).** A read keeps what the event stream
  said while it ran, and a newer read supersedes an older one's answer.
  `notices.e2e.ts` "a notice posted while the server list is read stays" (the reviewer's probe)
  failed with `[]` before the fix. The button's `data-notices-read` counts the reads handled.
- **The resync half had no test (minor).** Now covered by "a resync drops a notice the server no
  longer holds". The background call itself stays untested; the tests call
  `sync_all_account_notices` directly.
- **The tick and the event loop raced (minor).** Both reconcile under one lock
  (`ACCOUNT_SYNC`) from the stored subscription. An event for a subscription the store does not
  hold is taken as it says. The Telegram seed test now stores `limited` before publishing it,
  as Yulius does.
- **Direction: a long-unavailable worker hid a red gate (minor).** After 10 staging ticks in a
  row with no worker able to take the repair (`GATE_GRACE_TICKS`), the people are told.
  `staging::a_worker_unavailable_past_the_grace_period_raises_the_gate_notice` fails with the
  grace rule disabled.
- **Nits:**
  - The summary key is `gates:more`, so it can no longer equal a test named `more`.
  - The bell's timer redraws only when an entry went.
  - The README row names the new tests.
  - NTC-004 says 60 s (pre-existing).
  - `gate_workers` is still read twice per project and tick when the branch is red: one
    indexed query, kept for clarity.
- Runs after the round-2 fixes:
  - `yulius::main`: 2,712 passed.
  - Library crates: 1,889 passed.
  - Chromium: 1,350 passed, 1 configured skip.
  - Clippy `-D warnings` clean, and `scripts/dev quick` passed.

### Review round 3 of PR #1711 (head `59258cb79`): approved, notes fixed within the approval

All notes were in this PR's own code:
- **The grace counter's reset was untested (minor).** Now covered by
  `staging::a_green_branch_starts_the_grace_period_again`, which fails with
  `ticks.remove(&sandbox.id)` undone.
- **A limit with no known end lost its notice when a window's reset passed (minor).**
  `limit_reset` returns none for the subscription's own limit without an end (as
  `AccountUsage::available_at_for` reads it), so the notice lasts until usable. Covered in
  `a_subscription_notice_follows_the_table`, which failed before the fix.
- **A restart retold a notice raised past the grace period (minor).** Within the grace period
  `sync_gate_notices` keeps the notices already told and posts no new one (`hold_new`).
  `staging::a_momentarily_busy_worker_raises_no_gate_notice` now expects the stored notice to
  stay when a worker without a session appears, which is the restart case. It failed before
  the fix.
- **Nits:**
  - The stray ". and" in the architecture document and the unit test's comment are fixed.
  - The event fallback for a subscription the store does not hold, and grace counts of
    removed projects, are unchanged. The fallback is what the tests publish, and it was so
    before this PR; the next tick removes a deleted subscription's notice. The counts are one
    integer per project.
- After these fixes, `staging::`, `notices::` and the `yulius-app` tests passed (799).

