---
yulius-task: 1f58ed73-02d4-4c49-8c66-dbf323eb4a3f
status: in-progress
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
  (30 s), and keeps at most 200 per (project, recipient). The cap drops the oldest notices that
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
  - An integration failure with no repair worker is posted to the project's people as
    `gate:<test>`, with the same text as the Status and CI pages. It is removed once the failure
    is repaired or a worker owns it. `GateNotice` carries its `key`.
- **NTC-011 Bell compatibility.** The bell gives every entry a key and an `expires` time.
  Entries already stored in a browser without `expires` get their kind's default, counted from
  their own time. Entries already past it are pruned on load.

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
| Subscription at its usage limit | bell, Telegram, `notices_list` | `account:<id>` (instance, admins) | its reset when known, else 1 day; removed when usable |
| Subscription reports an error | bell, Telegram, `notices_list` | `account:<id>` (instance, admins) | 1 day; removed when usable |
| Integration failure with no repair worker | bell, Telegram, Status, CI page, `notices_list` | `gate:<test>` (project) | never while unrepaired; removed when repaired or owned |
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
- [ ] Independent review of the notice core, every finding answered
- [ ] Subscription problems are keyed notices that end with the problem
- [ ] Unrepaired integration failures are keyed notices for the project's people
- [ ] The bell shows server notices and keys and expires every entry
- [ ] Old browser notification history migrates with expiries
- [ ] Product, API and architecture docs describe keyed, expiring notices
- [ ] Independent review of the producers and the bell, every finding answered

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
- **AC-12** (migration) A store at schema 87 with digests and Telegram rows migrates. The rows
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
- Chromium e2e (`e2e/tests/notifications.e2e.ts`): server notices, replacement by key, hiding at
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
