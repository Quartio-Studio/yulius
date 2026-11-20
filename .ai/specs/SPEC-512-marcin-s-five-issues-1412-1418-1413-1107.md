---
yulius-task: c2fc38ad-443d-48d0-ab7b-549eb0d1e4b0
status: in-progress
---

# SPEC-512: Marcin's five issues: parked sessions, answers from a second person, the digest, Telegram intake, handoffs

Status: in-progress
Date: 2026-11-19
Owner: Yulius task #494
Scope authorization: the request of task #494 (by MrFishPL for the owner, Michal Karp,
2026-11-19): "Fix the five open issues Marcin (`studenciak00owl`) filed on this repository:
#1412, #1418, #1413, #1107 and #1106. Each issue is its own piece of work with its own pull
request or pull requests, delivered in the order below, and each is closed by the pull request
that completes its acceptance criteria." Each issue's recommended option is followed; everything
that widens who can do what stays off by default; the owner rule (GIT-004) stays.

This spec is delivered in five parts, one per issue, each by its own pull request. A part's
requirements carry its issue's prefix: PRK (#1412), ANS (#1418), DIG (#1413), TGI (#1107), HOF
(#1106). Decisions: [ADR-0367](../../docs/decisions/0367-an-idle-session-is-parked-and-resumed-by-a-message.md)
(parking), [ADR-0379](../../docs/decisions/0379-a-needs-you-names-who-may-answer-it.md)
(named requests), [ADR-0380](../../docs/decisions/0380-an-end-of-day-digest-built-from-task-state.md)
(the digest) and the records the later parts add.

## Goal

1. **#1412.** A task whose turn ended and that waits for a person no longer holds a live session:
   after a grace period its session is parked (the process ends, the task stays where the list
   showed it, with its transcript, `needs_you` and delivery) and it resumes in the same session
   when someone writes to it. Only working sessions count toward `max_sessions`; a queued task
   says what it waits for; the Capacity card shows working, parked and queued sessions apart.
2. **#1418.** A `needs_you` can name who it waits for; a named person with write access answers
   it while it is open, on the web and with `task_message`, without taking the task.
3. **#1413.** An optional end-of-day digest, off by default: what waits for the person, the
   actions left in last messages, what was parked, what still works; to the browser and Telegram.
4. **#1107.** The personal Telegram channel's intake, off by default per person: a reply to a
   notification reaches its task, a new message becomes a task, `/status` and `/task`.
5. **#1106.** Tasks hand work to another project of the same instance behind an instance switch
   (off, ask, automatic; off by default) with a per-project opt-out, provenance and audit.

## Context

### #1412 today (read in the code on 2026-11-19)

- **What counts.** `App::sessions_in_use` (`crates/yulius-app/src/supervisor.rs`) counts live
  attempts except those of Done tasks (SPEC-050 SES-101). A task that waits for a person (`needs
  you`, "Your turn", an answered agent that is not Done) holds a slot, and every idle session,
  Done or not, keeps its Claude Code process: about 250 to 300 MB each (#1412's comment: 37
  processes, 9 GB of 11 GB).
- **What ends a session.** A person's close (`close_task_now`, `EndIntent::Close`) ends the
  session gracefully and the task is `closed`: the list shows it under Ended, its `needs_you` is
  hidden (ADR-0111: an ended task waits for nobody), and it continues only by Continue. That is
  more than parking: the task leaves where it was.
- **What resumes a session.** `RunStart::Resume` launches Claude Code (or Codex, Antigravity)
  with the task's recorded provider session (`--resume`), as Continue and the capacity loop do
  after a usage limit; a goal-mode task gets its `/goal` typed again (Claude's goal is per
  session).
- **The queue.** `start_run_if` queues a start when `sessions_in_use() >= max_live_sessions()`,
  an update holds starts, or a supertask holds the task; only the supertask hold is told on the
  task (`held_by`). The status panel and the Capacity card (`GET /api/capacity`, the Dashboard and
  Settings) do not show the sessions at all; the status panel says "live of limit, queued".
- **The tick.** `start_background` runs a 60-second tick that drains the queue, advances chains
  and releases prepared tasks.

### Later parts

Each later part adds its own context here when it starts, from `dev` as it is then: #1418 after
task #492 (SPEC-510, "take control") merged; #1107 against what pull request #1362 (ADR-0351)
merged; #1106 against ADR-0227.

## Requirements

### Part 1: parked sessions (#1412)

- **PRK-001 A parked status.** A task can be `parked`: its session was ended by Yulius to free
  its slot and memory, and it resumes in that session. Not live (no process, no slot), not
  resting: it keeps its worktree, its claims, its `needs_you`, its delivery and its place in the
  list. Transitions: `running → parked` (the parking run's end), `parked → starting` (a resume),
  `parked → queued` (a resume that waits for a slot), `parked → closed` (archive or close),
  `parked → stopped(cancelled)` (a stop).
- **PRK-002 Who is parked.** A `running` task is parked once all of these hold for the grace
  period: its activity is `waiting` or `done` (its turn ended; `needs_attention`, a question in
  its terminal, is never parked), the list shows it under Needs you or Done, it has no background
  shell or subagent, does not wait in `yulius wait`, has no merge armed and no pull request news
  due, no held prompt, no pending browser request, no person's pause, no possible draft in its
  prompt box and no keystroke within the grace period, nothing waiting in Claude Code's queue,
  and it is not the onboarding task (whose run's end decides the onboarding). Its background work
  is looked at afresh before the session ends; a look that gives no answer counts as work. No
  park starts in the first three minutes after a server start, so the tasks that went idle while
  it was down are woken first (SPEC-358 MSB-007). The grace period is `YULIUS_PARK_AFTER_MINUTES` (default 30; 0 turns parking off),
  counted from the last change of its status or activity (`state_at`).
- **PRK-003 How it parks.** The session ends as a close does (graceful exit, then escalation),
  and the run's end records `parked` instead of `closed`; the task keeps its activity. Parking
  is decided under the launch lock against the task as stored, so a turn, a message or a start
  that came meanwhile wins. Audited `task.parked` (actor `yulius`). The terminal shows that the
  task is parked and how it resumes.
- **PRK-004 It shows as it did.** A parked task is in the list's group it was in, labelled
  "· parked", never Working: nothing of it goes on by itself, so checks still running, a conflict
  with its base or a CI run that could not run make it Needs you (an achieved goal is Done
  whatever its checks still do), and otherwise it is Needs you or Done by the idle rule. Its
  `needs_you` stays open and is shown; `tasks_list` and `task_get` say `parked`. For chains and
  start rules, a parked task that is Done is done; its scope claims still count, and the
  briefing lists it among the parallel tasks.
- **PRK-005 Writing resumes it.** (An allowed answer from a named person resumes it too: ANS-002.)
  `task_message` to a parked task resumes it with the message as
  its first prompt (labelled as today) and ends its `needs_you`, delivered `"resumed"`; the web's
  Resume (the Continue of a parked task) resumes it with nothing typed, its `needs_you` kept
  until the person writes. Either way it is the same provider session, with its history; at the
  session limit the resume waits in the queue with its prompt (SPEC-483 QST-001). The owner rule
  is unchanged: only who may steer the task resumes it. `task_restart` of a parked task is its
  resume; `task_merge` tells to resume it to arm a merge.
- **PRK-006 What counts.** Only live sessions of tasks that are not Done count toward
  `max_sessions` (SES-101 stays); parked tasks hold no session and no subscription.
- **PRK-007 A queued task says why.** A queued task's summary carries `queue_reason`, one line:
  "Waiting for a free session: 8 of 8 in use", "Waiting for the Yulius update to finish", or the
  supertask's line (as today), and the list and the task page show it.
- **PRK-008 The Capacity card.** `GET /api/capacity` adds `sessions {working, limit, parked,
  idle, queued, park_after_ms}`; the Capacity card shows a Sessions tile: working of limit,
  with parked, idle (live but not counted: Done) and queued apart, and when idle sessions park.
  Parked sessions use no memory.

### Part 2: a request that names who it waits for (#1418)

Today (after task #498, SPEC-516, ADR-0370, merged as #1428): anyone with write access answers a
running task's open `needs_you` on its page (`POST /api/tasks/{tid}/needs-you/answer`), labelled
as theirs, audited `task.needs_you.answered`, without control moving; `task_message` stays the
owner's; stop, archive and delete are anyone's who may work in the project (ADR-0331), and
steering (terminal input, Continue, merge, model, subscription) is the owner's. Taking control
(SPEC-510, ADR-0365) is for steering a task; what this part adds is for one answer.

- **ANS-001 Naming.** `needs_you` takes `who`: one to ten GitHub logins it waits for besides the
  owner. Each must be a person of the instance with write access to the project, checked when the
  request is raised; one that is not refuses the whole request, saying which and why. The owner
  named is dropped from the list (always may answer). Stored with the request
  (`NeedsYouRecord.who`), shown in its view (`who`) and in `tasks_list`/`task_get`.
- **ANS-002 Who answers a named request.** While a request names people, its owner and the
  people still named answer it; anyone else with write access is refused ("task #N asks @a and
  @b, not you"). A request that names nobody keeps ADR-0370's rule (any writer). A question
  stays answerable while it is open, also once its task parked (PRK-002): the answer resumes the
  task in its own session with the answer as its first prompt, as a message does (PRK-005). A
  task whose run holds its owner's personal secrets or MCP servers names nobody: nobody else
  could answer it.
- **ANS-003 Through `task_message`.** A message from someone who is not the task's owner, while a
  request is open that they may answer (ANS-002), is that answer: it goes the way of the web's
  answer (same label, guards and audit) and `delivered` says `answered`. With no such request it
  is refused as today (the owner steers).
- **ANS-004 Closed, replaced or withdrawn.** An answer names the request it answers
  (`asked_at_ms`, the request's `at_ms`; the page sends the one Reply opened on, and after a
  refusal the one it shows). One sent after the request ended is refused ("…asks nobody anything
  now"), after a new request replaced it ("the question you answered was replaced by a new one:
  read it and answer again"), and after the owner withdrew the person ("@owner withdrew you from
  this question"). An answer ends only the request it answered: one asked while it was typed
  stays open.
- **ANS-005 The owner sees and withdraws.** The task's page shows "Asks @a, @b" on the request;
  its owner withdraws a person there (`POST /api/tasks/{tid}/needs-you/withdraw {login}`, owner
  only, a browser session or an operate token), audited `task.needs_you.withdrawn`. Withdrawing
  the last name leaves the request to its owner alone.
- **ANS-006 Nothing else moves.** A named person who answers does not get control: the task acts
  on GitHub as its owner; terminal input, Continue (start), and `delivery_merge`/`task_merge`
  stay the owner's (tested for the named person); stop follows ADR-0331 as today.

### Part 3: the end-of-day digest (#1413)

Today (read on `dev` at 5cc48169e): parking (part 1) ends idle sessions of tasks that wait for a
person or are Done after `YULIUS_PARK_AFTER_MINUTES`; nothing gathers what waits for a person. The
browser's notification center is built from the event stream while a tab is open (kept in that
browser); the Telegram notifier (ADR-0351) sends each linked person's chosen kinds, redacted with
`redact_for_sandbox` (TGC-023). Instance settings live in columns of `instance`.

- **DIG-001 The setting.** `GET`/`PUT /api/admin/digest` (superadmins and instance
  administrators): `enabled` (default false), `at` (`HH:MM`, UTC, default `18:00`) and
  `park_idle_hours` (1 to 720, or null: the digest parks nothing; default null). Stored in
  `instance.digest`, audited `instance.digest.set`, shown in Settings, Instance. Turning it on
  never sends at once: the first digest is the next `at`. Off, nothing below runs.
- **DIG-002 Once a day.** The background tick runs the digest when it is on, today's `at` has passed
  and it has not run since; the run is taken with one conditional write of `digest_ran_at`, so a
  restart or a second tick never sends it twice. A server down at `at` sends it when it is back
  the same day.
- **DIG-003 It parks what waits for nobody.** With `park_idle_hours` set, the run first parks each
  running task whose session has been quiet that long, whose agent waits for its turn (not Done),
  that has no open `needs_you` and that every other guard of PRK-002 lets go (no turn,
  background work, Yulius wait, held prompt, browser request, pause, keyboard, onboarding), as
  part 1 parks: it resumes when someone writes to it. Done tasks and tasks waiting for a person
  are never parked by the digest; part 1's own grace period is unchanged.
- **DIG-004 What it says.** One digest per enabled person, about the tasks they own and the
  requests that name them, in this order:
  1. *Waiting for you*: their running or parked tasks with an open `needs_you` or a question in
     the terminal, and other people's tasks whose open request names them (ADR-0379): project,
     number, title, the request, the link.
  2. *Parked*: their tasks parked since the last digest (by it or by part 1; the first digest
     looks back 24 hours) and not listed above: title, how it ended (finished, or waiting for
     their turn), what the agent's last message left (its end, at most 300 characters), the link.
  3. *Still working*: how many, and up to ten one per line.
  A person with nothing in any section gets no digest.
- **DIG-005 Where it goes.** The digest is stored for the person (the last seven kept),
  published to their open pages, and the bell shows it, also when the page opens later
  (`GET /api/me/digest`); the entry opens `/digest`, which shows the latest digest with its links.
  A linked Telegram chat gets it as one message unless the person turned digests off there
  (`digest` among the Telegram choices, on by default once linked).
- **DIG-006 Redaction.** Every text taken from a task (title, request, last message) passes the
  same redaction as that sandbox's transcripts before it is stored or sent.

### Part 4: the personal Telegram channel's intake (#1107)

Today (read on `dev` at 9427240e9): part 1 of #1107 is merged (pull request #1362, ADR-0351,
SPEC-498): an instance administrator sets the bot (sealed, checked with `getMe`, removable), a
person links their chat with a single-use 10-minute code from a browser session and unlinks in
Settings or with `/stop` (audited), notifications reach the chat redacted under a rate bound, and
the server long-polls with backoff. A linked chat's other messages get one help reply ("Replies
and new tasks are not supported yet"). What #1107's acceptance still asks is the intake (its
recommended option 1, part 4): replies and new tasks, off by default, on per person.

- **TGI-001 The switch.** Each person's linked chat has `intake` (off by default) and a default
  project, set in Settings → Notifications from a browser session only (as linking is) with
  `PUT /api/me/telegram/intake` `{"enabled","sandbox_id"}`; the project must be one they may work
  in; turning it on without one is refused. Audited `telegram.intake` (on or off, the project).
  Unlinking ends it. Off, a linked chat's messages create and send nothing; the help reply says
  how to turn it on.
- **TGI-002 A reply reaches its task.** The bot remembers which of its messages were about which
  task (the last 500 per chat). With intake on, a reply to one is the person's `task_message` to
  that task, under the owner rule and its guards as from their own agent (ADR-0331, ADR-0379: a
  person a request names may answer it; a task holding the owner's personal reach accepts only
  its owner; a parked task resumes). The bot answers "Sent to #n" or the refusal's reason.
- **TGI-003 A new message is a task.** With intake on, any other message (text, a caption, a
  photo, a document, a voice note, an audio or video file; an album is one message) becomes a
  task in the default project as that person, with the task bound, the onboarding gate and access
  checks of `task_create`. Its files, each at most 20 MB (Telegram's bot download bound) and at
  most 10, become the task's attachments in the person's drafts folder, named in its prompt. The
  task is labelled "from Telegram" (origin `telegram`) and audited. The bot answers with its
  number and link.
- **TGI-004 Commands.** `/status` lists the person's tasks that wait for them (as the digest's
  first section: an open request, a question in the terminal, a request naming them), at most 20,
  with links; `/task <project> <text>` starts the task in another project they may work in
  (named by its repository name or slug); `/help` says what the chat does.
- **TGI-005 Redaction.** The bot's answers that carry a task's text (`/status`, a refusal naming a
  task) pass the same redaction as transcripts.

### Part 5

Written when it starts (see Context): HOF (#1106), from its issue's acceptance criteria.

## Plan

- [x] Spec SPEC-512 and ADR-0367 for parked sessions written
- [x] A parked status: transitions, store, list group on the server and the web
- [x] Idle sessions park after the grace period, decided under the launch lock
- [x] Writing to a parked task resumes it: task_message and the web's Resume
- [x] A queued task says what it waits for
- [x] The Capacity card shows working, parked, idle and queued sessions
- [x] Docs for parking: product, architecture, api, operations, changes
- [x] Review of the #1412 pull request answered test-first
- [x] needs_you names who it waits for, refused for logins without write access
- [x] A named person answers an open needs_you on the web and with task_message
- [x] The owner sees who may answer and withdraws it; answers attributed in transcript and audit
- [x] Owner-only refusals tested for the named person; ADR and docs for answers
- [x] Review of the #1418 pull request answered test-first
- [x] Digest setting: on or off, time and idle threshold, off by default
- [x] The digest is built from task state and last messages
- [x] The digest parks idle tasks past the threshold and goes to the browser and Telegram
- [x] ADR and docs for the digest
- [x] Review of the #1413 pull request answered test-first
- [x] Telegram intake switch and default project per person, off by default
- [x] A reply to a Telegram notification reaches its task under the owner rule
- [x] A new Telegram message or file becomes a task with attachments
- [x] Telegram commands /status and /task; ADR and docs for intake
- [x] Pull request #1130 answered and closed as superseded
- [ ] Review of the #1107 pull request answered test-first
- [ ] Handoff setting off, ask or automatic and a per-project opt-out
- [ ] The sandbox_handoff tool with its checks and bounds
- [ ] Ask mode: a stored request the owner approves in a browser session
- [ ] Provenance on the target, the outcome sent back, task.handoff audited on both
- [ ] ADR amending ADR-0227 and docs for handoffs
- [ ] Review of the #1106 pull request answered test-first

## Acceptance criteria

### #1412

- [x] A task with no turn running and no background work does not count toward `max_sessions`
  after the grace period (PRK-002, PRK-003, PRK-006).
- [x] Writing to such a task resumes it in its own session, with its history (PRK-005).
- [x] A queued task shows why it has not started and what it waits for (PRK-007).
- [x] The Capacity card shows working sessions and parked sessions separately (PRK-008).

### #1418

- [x] A task can name who its `needs_you` waits for; logins without write access are refused when
  the `needs_you` is raised (ANS-001).
- [x] A named non-owner can answer that `needs_you` (web and `task_message`) while it is open, and
  is refused once it is cleared or replaced, with a message that says so (ANS-002 to ANS-004).
- [x] The answer appears in the transcript and the audit log as that person's, and the task keeps
  acting on GitHub as its owner (ADR-0370, ANS-006).
- [x] The owner sees who else may answer and can withdraw it (ANS-005).
- [x] Everything else on the task stays owner-only (GIT-004), with tests for terminal input,
  start, stop and merge by the named person (ANS-006).

### #1413

- [x] An instance setting turns the digest on and sets its time and the idle threshold, both off
  by default (DIG-001).
- [x] The digest goes to every notification channel the person has on: the browser and Telegram
  (DIG-005).
- [x] Tasks waiting for the person are listed first and are never parked by it (DIG-003, DIG-004).
- [x] Idle tasks past the threshold are parked, listed with links and what their last message
  left, and resume when someone writes to them (DIG-003, DIG-004).
- [x] A digest with nothing to report is not sent (DIG-004).

### #1107 (part 2: intake)

- [x] Off by default per person; turned on in Settings with a default project, from a browser
  session only, audited (TGI-001).
- [x] With intake on, a reply to a notification reaches that task as a `task_message`, under the
  owner rule; a refusal says why (TGI-002).
- [x] A new message, or one with a photo or document, becomes a task with attachments in the
  default project; `/task` targets another project (TGI-003, TGI-004).
- [x] With intake off, neither happens (TGI-001).
- [x] What the second person still cannot do through a reply: start, stop, merge, terminal input
  (TGI-002).
- [x] Docs: `product.md`, `api.md`, `changes.md` and an ADR (TGI-001..005).

## Test plan

### #1412

- Domain: `TaskStatus` transitions of `parked` (every allowed and refused event), its wire name,
  that it is neither live nor resting, and the group rule for a parked task (Needs you, Done,
  never Working by a live overlay).
- App, with the fake agent (`crates/yulius/tests`): an idle task parks after the grace period
  and its slot frees a queued task; each exclusion of PRK-002 keeps a task live (background
  shells, `yulius wait`, armed merge, needs_attention, a keystroke, a held prompt, grace 0); a
  turn that starts between the decision and the end wins; `task_message` resumes a parked task
  in the same provider session (`--resume` with the recorded session id) and ends its
  `needs_you`; the web's continue resumes it and keeps `needs_you`; at the limit the resume
  queues with its prompt; archive closes a parked task; a stop stops it.
- `queue_reason` at the session limit and during an update; the Capacity view's `sessions`.
- Web unit tests (`status.js`): label and group of a parked task, `taskActions` offers Resume,
  the queued line; an e2e test for the Sessions tile and a parked task's row.

### #1413

- Unit: the digest's park rule (each guard keeps a session; Done and Needs you never park); the
  digest's text (sections in order, cut last message, nothing to report → none); the day's run
  decision (before and after `at`, ran already, turned on after `at`).
- Store: `digest_ran_at` is taken once; digests kept per person, the last seven.
- Integration (`crates/yulius/tests/telegram.rs` and `parking.rs`, the digest tests): off by default (no digest, no park); a run parks
  an idle Your-turn task past the threshold and not a Needs-you or Done one, and a message
  resumes it; the person's digest lists the waiting task first, the parked one with its last
  message redacted, the working count; Telegram gets it (fake bot) and not with `digest` off;
  nothing to report sends nothing; two ticks send one.
- e2e (`digest.e2e.ts`): the setting in Settings; the bell's digest entry opens `/digest`.

## Decisions

- **A new status, not a flag on `closed` nor a `running` task without a process.** `closed` means
  a person ended the task (ADR-0111 hides its request, the list shows it Ended, chains read it as
  ended); a `running` task without a run breaks the restart reconciliation (ADR-0050), which
  takes a running task with no live run for a lost one. `parked` says what happened and keeps
  both rules true. ADR-0367.
- **Parked tasks are neither "live" nor "resting".** `is_resting` means ended in most of its
  callers (CI hygiene of worktrees, `needs_you`, chains); `is_live` means a process holds a slot.
  A parked task's claims count (`TaskStatus::holds_claims`), it is Done for chains when its idle
  run would be, and the few places that continue or archive a resting task accept `parked` by
  name. A supertask does not wait for a parked task, as it does not wait for an idle Done one.
- **A park looks at the session afresh.** The list's shell and `yulius wait` counters are a
  probe's last answer; before ending a session the park probes it again and treats no answer as
  work running, as a move does (SPEC-483 SWT-001), and asks Claude Code's queue (SPEC-471). A
  park whose session cannot be ended gives up and the session goes on.
- **A withdrawal splits the request's wait span.** The spans trigger (migration 0028) starts a
  new `needs_you` span on any change of the stored request; a withdrawal changes its `who`, so
  the Pipeline shows the wait as two spans from the same start (the first ended "replaced").
  Kept: a migration of the trigger for this alone is not worth its risk now; the wait's total is
  unchanged.
- **Known limits.** The park's intent lives in the server's memory: a restart between the
  decision and the session's end records the end as a clean exit (`closed`) or a crash. A person
  who only reads the terminal does not hold a park; a keystroke or draft does. On the web,
  "writing to it" is Resume followed by typing; `task_message` resumes it with the message.
- **The grace period is a unit setting, default 30 minutes.** Long enough that a person who reads
  the answer and replies within the half hour finds the live session; short enough that a night
  of finished tasks frees its memory. Settings → limits is for host-sized numbers; this one is not
  sized from the host. 0 turns parking off.
- **A web Resume types nothing and keeps `needs_you`.** The person who presses Resume writes into
  the terminal next; their keystrokes end the request as today. A `task_message` is the answer
  itself, so it ends the request as a typed message does (TMS-006).
- **Yulius's own news does not wake a parked task.** A parked task has no armed merge and no
  pending news by PRK-002; news that arrives later waits in its watch like any news for a
  session that takes none, and is told when the task resumes.

- **The digest is built from task state and the transcript, without a model** (#1413's
  recommended option). The last message's end (300 characters) stands for "the action it left":
  an agent ends its turn with what it needs; a model to pick actions out stays a later option.
  ADR-0380.
- **The digest's parks are narrower than part 1's.** The issue says tasks waiting for the person
  and Done tasks are never parked by the digest; part 1 parks both after its grace period. Both
  hold: the digest adds a park for quiet Your-turn tasks past its own threshold (an instance that
  turned part 1 off or set it long), and part 1 is unchanged.
- **Stored per person, read by the bell.** The notification center lives in the browser from the
  event stream; a digest sent while no page is open would be lost, so the server keeps the last
  seven and the bell reads the latest when a page opens. Telegram is one message per digest,
  outside the per-person rate bound (one a day).
- **Who gets one.** Every enabled person with something in a section; there is no per-person
  browser switch (the instance setting is the switch, as the issue's acceptance says), and
  Telegram digests can be turned off among the chat's choices.

- **A Telegram reply goes through `task_message`, labelled `Telegram`.** The person typed it,
  not their agent; the label says so (`Channel::Telegram`), and every guard is the one their own
  agent meets. A reply to a notice of a request carries that request's `at`, so it never answers
  a later one (ANS-004).
- **Intake runs off the poll.** Files take up to two minutes each; the poll goes on, so other
  chats' `/stop` and link codes are not held, and the next poll confirms the update at once.
- **Known limits.** An album split by Telegram across two answers of `getUpdates` (more than 20
  updates waiting) becomes two tasks. Files are fetched before `create_task_as` decides, so a
  task it refuses (the onboarding gate) leaves its files in the person's drafts folder, removed
  with the other drafts after a week.

## Evidence

### Part 4 (#1107, intake)

| Acceptance criterion | Test |
|---|---|
| Off by default per person; on in Settings with a project they may work in, audited | `telegram::telegram_intake_is_off_by_default_and_set_with_a_project` (defaults, 422 without a project, refused for a project without access, audit `on s1`); store `intake_and_the_bots_messages_follow_the_chat`; e2e `telegram.e2e.ts` "a person turns their chat's intake on with a project" |
| With intake off, nothing happens | `telegram::telegram_intake_is_off_by_default_and_set_with_a_project` (a message starts no task; the help says how to turn it on) |
| A reply reaches its task as a `task_message` under the owner rule | `telegram::a_reply_reaches_its_task_under_the_owner_rule`: the owner's reply is handled as her `task_message` (here "task #1 has no live session": no agent runs in that harness; delivery to a live or parked session is `task_message`'s, tested in `task_message` and `parking`); a teammate's reply after the request ended is refused by the owner rule, nothing audited; the bot's messages are matched per chat (store test) |
| A new message, or one with a photo or document, becomes a task with attachments; `/task` targets another project | `telegram::with_intake_a_message_starts_a_task_and_a_photo_is_its_attachment` (origin `telegram`, owner the person, the photo's bytes at the path named in the prompt, `/task app …`, unknown project refused); adapter `replies_captions_files_and_albums_are_read`, `a_file_is_fetched_within_its_bound`; `telegram::tests::an_album_is_one_message` |
| What the second person still cannot do through a reply | a reply is only ever a `task_message`; the owner rule refuses a teammate's (test above); there is no Telegram path to start, stop, merge or type into a terminal |
| `/status`, redaction | `telegram::a_reply_reaches_its_task_under_the_owner_rule` (`/status` lists the waiting task; the refusal carries no agent secret); `notify::tests::intake_commands_are_read` |
| Docs and ADR | ADR-0392, `docs/product.md`, `docs/api.md`, `docs/architecture.md`, `docs/changes.md` |

PR #1130 was commented on and closed as superseded by #1362 (part 1), naming this part 2.

### Part 3 (#1413)

| Acceptance criterion | Test |
|---|---|
| An instance setting turns the digest on, sets its time and idle threshold, both off by default | `telegram::the_digest_is_off_by_default_and_set_by_an_instance_administrator` (defaults, instance administrators only, 422 for a bad time or threshold, audit); `digest::tests::a_time_is_hours_and_minutes`; e2e `digest.e2e.ts` "an instance administrator turns the digest on" and "a member sees no digest setting" |
| It runs once a day after its time, never twice, never while off | `digest::tests::the_digest_is_due_once_a_day_after_its_time`; `telegram::the_digest_runs_once_a_day_after_its_time` (controlled clock); store `digest::tests::the_days_run_is_taken_once` |
| It goes to every notification channel the person has on (browser and Telegram) | `telegram::a_digest_lists_what_waits_first_redacted_and_reaches_the_chat` (fake bot; not sent with `digest` off, still kept for the browser); e2e "a digest made while no page was open reaches the bell and opens /digest" and "the day's digest arrives while the page is open"; store `a_person_keeps_the_last_seven_digests` |
| Tasks waiting for the person are listed first and never parked | `parking::the_digest_parks_a_quiet_task_that_asks_nobody_and_lists_its_last_message` (run says parked 0 while it asks: fails when the rule ignores the request, checked by mutation); `parking::tests::the_digest_parks_only_what_waits_for_nobody` (Done never); `digest::tests::the_message_lists_what_waits_first` |
| Idle tasks past the threshold are parked, listed with links and their last message, and resume when written to | `parking::the_digest_parks_a_quiet_task_that_asks_nobody_and_lists_its_last_message` (real processes, part 1 off: parked, audited `task.parked`, "Last message: ECHO: …", `task_message` resumes it) |
| Text that leaves the instance is redacted | `telegram::a_digest_lists_what_waits_first_redacted_and_reaches_the_chat` (an agent secret in the request is absent from the stored digest and the Telegram text) |
| A digest with nothing to report is not sent | `telegram::a_digest_lists_what_waits_first_redacted_and_reaches_the_chat` (bob, with no task, gets none: `null` and no message) |

Review round 1 (head 3d6fb8768, CHANGES REQUESTED), answered test-first:

| Finding | Change | Test |
|---|---|---|
| The digest reached people who cannot see the project (owner who lost access, named person without access) | each recipient is checked with `can_view` per project, cached per run | `telegram::a_digest_reaches_only_people_who_can_see_the_project` (fails without the check: checked by mutation) |
| A task the digest parked was listed again the next day | the listing has its own mark (`digest_listed_at`), its value taken once the run's parks settled (no longer live) and before the build, written after it; "parked since" is after the last mark and up to this one | `parking::the_digest_parks_…` (a second run sends nothing) and `telegram::a_digest_lists_…` (a second digest lists nothing parked); both fail when the window ignores the mark (mutation) |
| A cleared bell entry came back on every load | the last digest heard is remembered per person apart from the list; the entry carries the digest's time | e2e "a cleared digest entry does not come back on the next load" (fails without the memory: mutation) |
| No tests for named, withdrawn or disabled people, or for redacting a title or a last message | added | `telegram::a_digest_reaches_only_people_who_can_see_the_project` (named, withdrawn, disabled owner), `telegram::a_digest_lists_…` (secret in a title), `parking::the_digest_parks_…` (agent secret in the last message: `[secret:DIGEST_KEY]`) |
| A save shortened the window; moving the time later sent a second digest that day | a save no longer touches the listing mark (the first digest looks back 24 hours); no digest when one already went out today | store `the_days_run_is_taken_once`, `digest::tests::the_digest_is_due_once_a_day_after_its_time`, `telegram::a_digest_lists_…` (saved before the first run, the parked task is still listed) |
| Transcripts read inside the tick | the run is spawned off the tick; its claim keeps a second tick out | by reading (`background.rs`) |
| One unreadable task cancelled every digest | a task that fails is logged and left out | by reading (`digest_drafts`) |
| Guests were refused `/api/me/digest` | allowed for guests (their own digest; visibility is checked per project) | `guest_may_reach` |
| Nits | `notify::truncate`; quiet time checked before a park's probe; "Finished" follows the list's group; `waiting_capacity` counts as working; digits-only times; `/digest` follows `digest.ready` | `a_time_is_hours_and_minutes`; parking test reads "Finished" as the list does |

Review round 2 (head 809c5b974, CHANGES REQUESTED), answered test-first:

| Finding | Change | Test |
|---|---|---|
| With two tabs open, only one tab's bell showed a live digest | only the read at page open asks the remembered id; a live `digest.ready` always adds its entry (the key dedups) | e2e "a digest arriving live reaches every open tab, also one whose browser already saw it" (failed before the change) |
| A run crossing midnight UTC skipped the next day's digest | the same-day rule reads `digest_sent_at`, written only by a run's claim (with `digest_ran_at`), not the listing's end | `digest::tests::the_digest_is_due_once_a_day_after_its_time` (claim at 23:59:30), store `the_days_run_is_taken_once` (the claim writes it, a save leaves it) |
| The listing window had no upper bound | "parked since" is after the last mark and up to this listing's mark | the parking and Telegram tests' second runs list nothing parked |
| Wording in architecture, spec and ADR-0380 | corrected (the mark's value is taken before the build and written after it; the same-day rule and the settle wait in the ADR) | by reading |
| CI at 3d6fb8768: the guest page test saw the mock refuse `/api/me/digest` | the mock's guest allowlist matches the server's | e2e `playground-guests.e2e.ts` "a guest sees their playground and nothing else" |

Review round 3 (head 35aacf65f): APPROVED, nothing blocking. Its one test gap was closed after the
approval (tests only): `telegram::a_digest_whose_listing_ends_after_midnight_keeps_the_next_days`
fails when the tick reads the listing mark instead of the claim (checked by mutation). Nits left
as they are: a digest park that takes over a minute to end is listed on two days; a page opened
in the same instant as a live digest can add the previous digest's entry once more.

Known limit (not changed): a stored digest is not redacted again when read, so a secret registered after a
digest was made stays in up to seven stored digests of its person until they rotate out.

Before and after: before, nothing gathered what waits for a person; they opened each project. After
(with the digest on), one bell entry a day, "Your end-of-day digest: 1 waits for you · 1 parked ·
2 still working", opens `/digest` with the sections and links, and the same text reaches the
linked Telegram chat. An instance that changes nothing sees no difference.

### Part 2 (#1418)

| Acceptance criterion | Held by |
| --- | --- |
| A request names whom it waits for; logins without write access are refused when it is raised | `task_ownership::a_needs_you_names_who_it_waits_for` (a reader and an unknown login refuse the whole request and record nothing; `@Bob`, the owner and a repeat clean to `["bob"]`) |
| A named non-owner answers on the web and with `task_message` while it is open; refused once cleared, replaced or withdrawn, saying so | `task_ownership::a_named_request_is_answered_by_whom_it_names` (Bob and the owner answer, Dave is refused naming @bob, an unnamed request stays any writer's, Bob's `task_message` is his answer, Dave's is refused, with nothing asked a message stays the owner's); `task_ownership::an_answer_to_a_replaced_or_withdrawn_request_is_refused` ("replaced", "withdrew"); ADR-0370's "nothing to answer" test still holds |
| The answer is that person's in the transcript and the audit; the task acts as its owner | `task_message::a_named_person_answers_through_their_own_agent` (real processes: "Answer from bob" on the agent's screen, request ended, owner unchanged, audited as bob) |
| The owner sees who may answer and withdraws it | `task_ownership::an_answer_to_a_replaced_or_withdrawn_request_is_refused` (only the owner withdraws, `task.needs_you.withdrawn` audited); e2e `reply.e2e.ts` ("Asks @bob, @carol", Withdraw @bob) |
| Everything else stays owner-only, tested for the named person | `task_ownership::a_named_person_steers_nothing_else` (terminal input refused, Continue and merge 403; a stop succeeds, as ADR-0331 lets anyone who may work there stop a task: the issue's "stop" predates ADR-0331) |

What a person sees. Before: a request on someone else's task offered every teammate Reply, and
their own agent's `task_message` was refused ("only they steer it"). After: the request says
"Asks @bob"; Bob gets Reply (and his agent's message is his answer), others get none; the owner
sees "Withdraw @bob". Refusals say "task #43 asks @alice and @bob, not you", "the question you
answered on task #43 was replaced by a new one: read it and answer again", "@alice withdrew you
from task #43's question".

Commands (2026-11-19): `cargo test -p yulius --test main -- task_ownership:: task_message::
telegram:: docker_trouble_notices:: agent_hooks:: cockpit:: parking::` → 221 passed;
`cargo test -p yulius-app --lib` → 626 passed; e2e `reply`, `parked`, `attention`, `ownership`,
`watching`, `notifications` → 79 passed; `scripts/dev quick`, `lint`, `cargo doc` passed.

Review round 3 of #1459 (request changes at c6430ace1), answered test-first:

| Finding | Answer | Test that fails without it |
| --- | --- | --- |
| A named person could not answer once the task parked | an allowed answer to a parked task resumes it in its own session with the answer as its first prompt (PRK-005; ANS-002 and ADR-0379 say so); the page offers Reply on a parked task | `parking::a_named_persons_answer_resumes_a_parked_task` (failed before: "not running (parked)"); e2e "a parked task's question is answered by whom it names" |
| The field re-keyed after any failed send | it no longer re-keys after a failure; before sending, a question the page shows as replaced is said ("…replaced by a new one: read it and answer again") and the field names it from the next send | e2e "an answer typed while the question is replaced…" |
| `tasks_list`/`task_get` text named nobody | "(answered by its owner and @bob, @carol)" / "(answered by its owner only)"; api.md lists `at_ms`, `who`, `named` | `mcp_text::tests::every_reason_a_task_gives_is_in_its_line` |
| Duplicated, untested shared-cause block | one helper (`answer_shared_cause`) for both ends | `docker_trouble_notices::a_page_answer_to_a_shared_docker_question_answers_the_others` |
| Sign-in-missing branch untested | test of a person with no stored GitHub sign-in | `task_ownership::names_that_could_never_answer_are_refused_with_why` |
| Naming people on a task holding the owner's personal reach | refused when raised: only the owner could answer | same test |
| Nits | `task_message`'s answer branch only for a running or parked task (an ended task's stale request gets the owner rule); its note and api.md say a question asked meanwhile stays | — |
| `task_message` names no request | kept: an agent answers the question open now; the page names the one Reply opened on | — |
| Pre-existing: refresh failure reads as sign-in missing (`access.rs`) | unchanged | — |

Review round 2 of #1459 (request changes at 23f21095a), answered test-first:

| Finding | Answer | Test that fails without it |
| --- | --- | --- |
| After a "replaced" refusal the open field kept naming the old question | after a refused send the field names the question it now shows | e2e "an answer typed while the question is replaced goes to no other question" (the second Enter answers the new question) |
| The answer-end guard had no test | `end_task_needs_you_at`: one conditional statement by the request's `at` (`end_needs_you_asked_at` also answers a shared cause's others only when it ended one) | store `an_answer_ends_only_the_request_asked_at_its_time`; `task_message::an_answer_leaves_a_question_asked_while_it_was_typed` (race point `AnswerTyped`; an unconditional end fails it: checked by mutation) |
| Stale `asked_at` in docs | api.md, architecture.md, ANS-004 and the e2e README name `asked_at_ms`/`at_ms` | — |
| The `who` access check's error wording | a missing GitHub sign-in is a refusal saying the person must sign in again; other errors say "could not be checked now" with the public message | `task_ownership::a_name_whose_access_cannot_be_checked_is_said_so` |
| Store test differed in both `at` and action | a same-action request asked later is refused too | store `a_request_is_replaced_only_while_it_is_the_one_read` |
| `?` on the read after typing | gone: the end is one statement, its failure logged | — |
| Round-1 nits | kept, because: the toast reads "Couldn't withdraw task #N" with the server's reason after it, which says what failed; people are matched by GitHub login, as `who` is given and shown, and a login names one person of the instance; after a take of control the new owner answers as owner and a name equal to them is harmless, while the former owner answers only if named (the request is the new owner's); requests sharing a `cause` end together by SPEC-494's rule, which names nobody | — |
| Notifier reads the task after the event (pre-existing) | unchanged; a start failing within the handling lag behaves as on `dev` | — |

`task_message::the_last_wake_after_an_outage_tells_the_person_of_their_draft` failed once under
a parallel run of five suites here and passed 3 of 3 alone (load-sensitive; it does not touch
this change's paths).

Review round 1 of #1459 (request changes at dd7b25b38), answered test-first:

| Finding | Answer | Test that fails without it |
| --- | --- | --- |
| Web sent the `at` of the question shown at Enter, not the one Reply opened on | the page keeps the question's `at_ms` from when Reply opened and sends it (`asked_at_ms`) | e2e `reply.e2e.ts` "an answer typed while the question is replaced goes to no other question" (fails when the page reads the request at send time: checked by mutation; the first Enter sends nothing: `failedPosts` stays 0 with every answer POST set to fail) |
| Withdraw read-modify-write could overwrite a newer request | `replace_task_needs_you`: one conditional statement on the request's `at`, action and `who`; 409 "changed meanwhile" otherwise | `needs_you::tests::a_request_is_replaced_only_while_it_is_the_one_read` (store) |
| A start hid a repeated failure from Telegram | a start forgets a told failure (`failed:…`) and keeps the rest, so a parked task's resume stays no news | `telegram::a_failure_after_a_restart_is_told_again`; `telegram::a_parked_tasks_resume_is_no_news` still holds |
| `asked_at` compared as a second-precision string | compared as the stored milliseconds (`at_ms` in the view, `asked_at_ms` in the answer) | `task_ownership::an_answer_to_a_replaced_or_withdrawn_request_is_refused` |
| `task_message`'s answered reply and audit | returns the task's brief like every other outcome and is audited `task.messaged` with its channel too; `docs/api.md` describes it | `task_message::a_named_person_answers_through_their_own_agent` |
| Untested lines | eleven names, a disabled person, the withdraw 409s, a withdrawn person's own agent; a request with all names withdrawn offers no Reply | `task_ownership::naming_and_withdrawing_refuse_what_cannot_hold`; e2e "a request whose names were all withdrawn offers a teammate no Reply" |
| ADR-0379 "builds on" 0370 | it says it amends 0370's rejected alternative | — |
| Spans split by a withdrawal | recorded in Decisions (kept) | — |
| Pre-existing: an answer ended a request asked while it was typed | the answer ends the request only while it is still the one it answered | covered by the replaced-request tests' paths |
| Nits | GitHub errors while checking `who` say so instead of "no write access"; the mock checks in the server's order with its words; CSS gap for the names; the Evidence quotes the server's words | — |

Also in this pull request, the two follow-ups the review of part 1 left: a parked task's Resume
is no Telegram news (`telegram::a_parked_tasks_resume_is_no_news`, failed before: the notice was
sent twice), and a parked status with no activity reads "Parked" on the Dashboard's Outcomes
(e2e `parked.e2e.ts`).

### Part 1 (#1412)

| Acceptance criterion | Held by |
| --- | --- |
| A task with no turn and no background work stops counting after the grace period | `parking::an_answered_task_parks_and_its_slot_starts_a_queued_task` (limit 1: the parked task's slot starts the queued one; activity kept, run ended `parked`, audited `task.parked` by `yulius`); `yulius_app::parking::tests::*` (every exclusion of PRK-002); `parking::a_task_parks_only_after_its_grace_period_and_never_with_parking_off` (grace 0, a long grace, a keystroke) |
| Writing to it resumes it in its own session with its history | `parking::a_message_resumes_a_parked_task_in_its_own_session` (`task_message` → `delivered: "resumed"`, second run `LaunchKind::Resume` with the same provider session id, `needs_you` ended); `parking::resume_keeps_the_request_and_archive_or_stop_end_a_parked_task` (the web's Resume keeps the request; archive → `closed`; Stop → `stopped`); `parking::a_message_during_a_park_is_refused_and_resumes_the_task_once_parked` (race point `ParkDecided`) |
| A queued task says what it waits for | `parking::an_answered_task_parks_and_its_slot_starts_a_queued_task` (`queue_reason` "Waiting for a free session: 1 of 1 in use"); `yulius_app::parking::tests::a_queued_task_says_what_it_waits_for`; e2e `parked.e2e.ts` "a queued task says what it waits for" |
| The Capacity card shows working and parked sessions apart | `parking::an_answered_task_parks_and_its_slot_starts_a_queued_task` (`sessions` = working 1, limit 1, parked 1); e2e `parked.e2e.ts` "the Capacity card shows working and parked sessions apart" |

The status itself: `yulius_domain::task::tests::a_parked_task_resumes_closes_or_stops` and
`a_parked_task_stays_in_its_group`; the web rule: e2e `parked.e2e.ts` "a parked task reads as it
did…" (labels, groups, actions, the ended terminal's Resume).

Commands (2026-11-19, after merging `origin/dev` at 4b9045b4c):

- `cargo test -p yulius --test main parking::` → 5 passed (`target/evidence/parking-9.log`).
- `cargo test -p yulius-domain --lib` → 573 passed; `cargo test -p yulius-app --lib` → 614 passed.
- `npx e2e run tests/parked.e2e.ts` → 3 passed; `subscriptions`, `loading-shapes`, `task-done`,
  `attention`, `home`, `archive`, `pause` → 112 passed.
- `scripts/dev quick` → passed; `scripts/dev lint` → passed.

What a person sees. Before: a finished task stays "Running · Your turn" or "Done" with a live
process; a queued task reads "Queued" and nothing else; the Capacity card has no sessions. After:
the same row reads "Your turn · parked" (or "Done · parked"), its second line "Parked to free its
session · resumes when you write to it", its terminal "Yulius parked this idle session to free its
place and memory. Resume it, or write to it, and it goes on in the same session with its history."
with a Resume button; a queued row reads "Waiting for a free session: 8 of 8 in use"; the Capacity
card's Sessions tile reads "8 / 8 working" over "13 parked · 2 idle · 3 queued" and "Idle sessions
park after 30 min".

Review round 1 (Fable 5.2, request changes at 74c6b7076), answered test-first:

| Finding | Answer | Test that fails without it |
| --- | --- | --- |
| B1 `cargo doc`: private intra-doc links in `parking.rs` | plain backticks | `RUSTDOCFLAGS="-D warnings" cargo doc --workspace --all-features --no-deps` passes |
| M1 a stop during the park's end left the task `stopping` | `settle_parked` rests a stopped or paused task as its run's end does | `parking::a_stop_during_the_parks_end_stops_the_task` (race point `RunEndTaken`) |
| M2 a parked task listed Working by its pull request | `AttentionFacts::parked_group`; `status.js` mirrors it | `a_parked_task_stays_in_its_group` (every `PullSignal`, plan open or not); e2e `parked.e2e.ts` |
| M3 a parked Done task never Done for chains and rules | `task_is_done` counts an idle parked task | `parking::a_task_chained_after_a_parked_done_task_starts` |
| M4 a parked task's claims ignored | `TaskStatus::holds_claims` in the claim readers, overlaps and the briefing | `parking::a_parked_task_keeps_its_claims` |
| M5 park trusted cached counters | fresh `background_or_wait_now` (no answer = work) and Claude Code's queue | `parking::a_look_that_gives_no_answer_keeps_the_session` (gated host) |
| M6 test gaps | added the tests in this table and below | `parking::a_question_in_the_terminal_or_a_pause_keeps_the_session`, `parking::a_resume_at_the_session_limit_waits_with_its_message` |
| F10 a park overtaking a switch closed the task | it parks; the notice says "the idle session was parked" (`SwitchEnd::Parked`) | covered by the fallback order of M1; no race test of its own |
| F11 a park whose session cannot end blocked messages | the park gives up and says so | `parking::a_park_that_cannot_end_its_session_gives_up` |
| F12 park intent in memory only | recorded as a known limit (Decisions) | — |
| F4 `task_restart`/`task_merge` on a parked task | resume through `continue_task`; merge advice says resume | the same `continue_task` as `resume_keeps_the_request…` |
| F7 Move offered, Stop not | no Move, Stop offered | e2e `parked.e2e.ts` |
| m7 park before the outage wake | no park in the first 3 minutes after a start (asserted at compile time > `WAKE_GRACE`) | — |
| m8 reason only on hover | the strip and the empty terminal show it | e2e "a parked task shows where it was…" |
| m9 analytics | parked + Done counts `done` | `analytics::tests` |
| m10, nits | rollback note in operations, the third `queue_reason` line documented, PRK-002 and ADR-0367 name the onboarding task | — |
| `queue_reason_of` per summary | kept: only queued tasks compute it, at most `queue_max` (32) | — |
| needs_you shared cause (pre-existing condition) | unchanged: answering a parked task resumes it; the others are answered by their own messages | — |

Review round 5 approved head 75c95f40e (2026-11-19).

Review round 4 (request changes at e68acb5e2), answered test-first:

| Finding | Answer | Test that fails without it |
| --- | --- | --- |
| G1 the parked goal's mark had no test | test with a goal the fake agent leaves failed, parked, then read by a second `App` on the same data (the restart) | `parking::a_parked_unfinished_goal_stays_your_turn_across_a_restart` (removing the `mark_parked_goal` call fails it: checked) |
| nit: the mark written after the status change | written before it, and removed when the goal is not unfinished | same test |
| nit: queue test's doc and grace | doc says only what it holds; waits past the grace before its check | — |
| minor: Resume re-tell | left as said in round 3 (not reproduced); not blocking for the reviewer | — |
| minor: `goal_unfinished_of` reads the attempts and stats a file per parked task | kept: it runs only for parked tasks, the list reads at most the sandbox's | — |

Commands after round 4: `parking::` and the queue test → 21 passed; `scripts/dev quick`, `lint`
passed.

Review round 3 (request changes at 87a654b6c), answered test-first:

| Finding | Answer | Test that fails without it |
| --- | --- | --- |
| W1 a parked task's summary dropped `goal_unfinished`/`spec_unfinished` | sent for an idle parked task as for an idle running one; the goal's state is kept in the parked run's directory (`parked-goal-unfinished`), which outlives a restart, and read for a parked task (summary, group, resume) | `parking::a_parked_tasks_summary_keeps_its_unfinished_work`; e2e `parked.e2e.ts` (`specLeft`, `goalLeft`) |
| L1 the reader took the park's intent and the escalation's loss was recorded first: `stopped_lost` | the reader leaves `park_taken` (the run's id) when it takes a park's intent; `mark_lost` parks when it finds no intent and that mark | `parking::a_loss_recorded_while_the_reader_holds_the_park_still_parks` (the session exits, its reader holds at `RunEndTaken` until the escalation recorded the loss; `exit_reason` `lost`, parked) |
| T1 the deaf test pinned no order | it accepts either recorder; the order where the loss comes first is pinned by the L1 test | 8 suite runs in a row, 19 tests each, all passed |
| T2 one pass per park in the same millisecond as the last state change | `parked()` runs a pass at each look until parked; negative checks first wait past the grace (`past_the_grace`); the chained test sets Done on the already parked task | same 8 runs |
| Q1 the queue guard untested | test `claude_code_moves::moves::a_prompt_in_the_queue_keeps_an_idle_session_from_parking`; with the fake agent a prompt in the queue also reads as a turn in progress (`turn`), so removing the queue guard alone does not fail it (checked by mutation): the guard is a second line for a queue the transcript shows while no turn does | (behaviour held, guard not isolable) |
| minor: Resume re-tells the same request on Telegram | not reproduced: in the Telegram harness park → starting → idle with the same request sent nothing new; left unchanged | — |
| nits | e2e README row, ADR index line, the unreachable arm, `parked_activity` written under the intent's lock and cleared when a park gives up | — |
| pre-existing: a close in the same recorder order ends an answered task `stopped_lost` | issue #1434 | — |

Two `claude_code_moves` timing tests (`a_prompt_that_just_went_on_keeps_the_session`,
`a_prompt_that_goes_on_while_a_turns_end_looks_keeps_the_session`) fail on this machine on the
`dev` merge base 4b9045b4c too (outside this change; they passed in Yulius CI at 87a654b6c).

Commands after round 3: `cargo test -p yulius --test main parking::` 19 passed, 8 runs in a row;
`telegram:: scope_overlaps::` and the queue test passed; domain 573, app 614;
`npx e2e run tests/parked.e2e.ts` 4 passed; `scripts/dev quick`, `lint`, `cargo doc` passed.

Review round 2 (Fable 5.2, request changes at e17ffaef3), answered test-first:

| Finding | Answer | Test that fails without it |
| --- | --- | --- |
| N1 a parked task with an achieved goal and work left read Needs you (web: Done) | `parked_group` is the idle running rule with the live overlays off, Working read as Needs you, as `status.js` does; checked before a pause | `a_parked_task_stays_in_its_group` asserts that equality over activity × pull × plan × spec × goal |
| M6 a park overtaking a switch | test of the race | `parking::a_switch_overtaken_by_a_park_parks_with_its_activity` (`ParkDecided` → `switch_task_now`) |
| minor: parked from `switching` lost its activity | the activity the park was decided on is kept in the run's runtime and used when the stored one was cleared | same test (`activity == waiting`) |
| M6 `mark_lost` with the park's intent | test | `parking::a_session_deaf_to_the_park_is_parked_by_the_escalation` (`exit_reason` `lost`, parked, audited) |
| M6 single-guard exclusions | tests for the held prompt and the onboarding task | `parking::a_held_prompt_keeps_the_session`, `parking::the_onboarding_task_is_not_parked` |
| M6 owner rule | test | `parking::only_its_owner_resumes_a_parked_task` (another writer's `/continue` 403, `task_message` refused naming the owner, still parked) |
| nits: scopes doc, a parked holder is not told | module doc says both | — |

Not tested, with why (round 2; the queue guard got a test in round 3): a probe that answers "work
running" (it takes the same `!= Some(false)` branch as the tested no-answer case); a request in
the sandbox browser (no browser runs in the tests); an armed merge (its guard is the same
`yulius` fact as `yulius wait`, read from `merge_is_armed`); the three-minute start gate (a
compile-time assertion keeps it past `WAKE_GRACE`).

Commands after round 2: `cargo test -p yulius --test main parking::` → 17 passed; domain 573,
app 614 passed; `npx e2e run tests/parked.e2e.ts` → 4 passed; `scripts/dev quick`, `lint`,
`cargo doc` → passed.

Commands after round 1: `cargo test -p yulius --test main parking::` → 12 passed;
`scope_overlaps:: task_message::` → 26 passed; `cargo test -p yulius-domain -p yulius-app --lib`
→ 573 + 614 passed; `npx e2e run tests/parked.e2e.ts` → 4 passed, four runs in a row;
`scripts/dev quick`, `scripts/dev lint`, `cargo doc` → passed.
