---
yulius-task: 0ab40d0e-824c-4561-8f61-a0f683354103
status: in-progress
---

# SPEC-191: Live task lists, and Paused as a state an update and a person both set

Status: in progress
Date: 2026-10-16
Owner: Yulius task #176
Scope authorization: task #176, requested by MrFishPL: "Tasks list doesnt refresh automatically,
there is no different status when it's paused by upgreader. Moreover, I'd like to have "paused"
status in general and it should be able be done by a user".

## Goal

The Tasks page and the sidebar always show what tasks do now, without a reload. A task an update
has paused reads **Paused** (for the update), not Working. And a person can pause a running task
and resume it later: the agent stops where it is, is not told, and goes on from there.

## Context

- **Live updates exist** (SPEC-002 §7.8): the Tasks page (`pages/tasks.js`) and the sidebar
  (`layout.js`) read `store.tasksOf(sid)`, loaded by `GET /api/sandboxes/{sid}/tasks` and kept
  current by `task.updated` events on `WS /api/events` (`store.js` `startEvents`); a reconnect
  emits `resync`, which reloads the lists. The server publishes every task change
  (`App::publish_task`).
- **Root cause of "doesn't refresh".** The event stream has no heartbeat in either direction:
  `routes/events.rs` `pump` sends only events and no ping, and the browser's `ReconnectingSocket`
  only reconnects on a `close` event. A connection that dies without a close reaching the browser
  (a laptop asleep, a network change, a proxy or NAT that drops an idle connection, a mobile tab
  in the background) stays `open` in the page for ever: no event arrives, no reconnect, no
  `resync`, and nothing says live updates stopped (the "live updates connected" dot stays on;
  `retryIfDown` on visibility skips an `open` socket). The lists freeze until a reload. Also, the
  backoff resets only on a message, so on a quiet instance every idle reconnect counts as a
  failed attempt and, after 360 of them, live updates stop for good.
- **An update's pause is invisible** (SPEC-160, ADR-0131): when an update's build is done every
  live task is frozen with the cgroup v2 freezer (or held at its turn boundary), and "task states
  are untouched": `TaskSummary` carries nothing about it, so a frozen task reads Working.
  `App::held_for_update(task)` knows it, in memory (`updates.rs`); only the launch-goal watch reads
  it.
- **Paused today** means only `waiting_capacity` (ROT-010): `status.js` shows `st-paused`,
  "Paused", "Paused: no subscription has capacity left …". There is no pause a person can ask for:
  the routes are stop, continue, switch, start-now, schedule (`routes/mod.rs`).
- **The freezer is the right tool for a person's pause too**: it stops the agent mid-token with no
  signal (ADR-0131), and the server already owns the delegated cgroup subtree. But every update
  thaw path (`thaw_run`, `thaw_frozen_runs`, and `sweep_frozen_runs` → `SessionHost::thaw_all`,
  which thaws every frozen `run-*` group, also at every start) would wake a task a person paused.
- Related tasks: #175 (request-control animation) and #174 (deploy feedback) touch other views.

## Requirements

### Live lists (LIV)

- **LIV-001 Heartbeat.** `WS /api/events` sends `{"type":"heartbeat"}` every
  `events_heartbeat_ms` (default 20 s), whatever else it sends.
- **LIV-002 Watchdog.** The browser's event stream counts as dead when nothing (event or
  heartbeat) arrived for 2.5 × the heartbeat (50 s): it closes the socket and reconnects, and the
  reconnect resyncs the lists (existing `resync`). When the tab is shown again or the browser goes
  back online, a stream silent for longer than one heartbeat is replaced at once. The watchdog is
  an option of `ReconnectingSocket` that only the event stream sets: terminals and the browser
  stream get no heartbeat and keep their behaviour.
- **LIV-003 Healthy means heard.** A connection that delivered a heartbeat resets the backoff, so
  idle reconnects never exhaust it (no heartbeat on open: a connection that opens and drops at
  once keeps backing off).

### A pause people can see (PAU)

- **PAU-001 The summary says why a task is paused.** `TaskSummary.pause` is null or
  `{by, user, since, pending}`: `by` is `update` (an update holds the task: the hold is on and the
  task not released yet) or `person` (a person paused it; `user` is who, `since` when); `pending`
  is true while the pause has not taken hold yet (the run cannot be frozen and its turn goes on).
  Only a task with a live run (`running`, `switching`) carries a pause; a person's pause wins over
  an update's.
- **PAU-002 An update publishes its pause.** Turning the hold on or off, a task's run confirmed
  frozen and a task released while resuming publish `task.updated` for the tasks concerned, so the
  lists change without a reload.
- **PAU-003 Paused reads Paused.** A paused task shows the `st-paused` icon and "Paused" in the
  Tasks page, the sidebar and its view, with the line "Paused for a Yulius update. It continues by
  itself." (while pending: "Pausing for a Yulius update at the end of its turn") or "Paused by
  @user" (while pending: "· pausing at the end of its turn"). It counts in the Working group, as a capacity pause does.

### A person's pause (PER)

- **PER-001 Pause.** `POST /api/tasks/{tid}/pause` (who may work on the task: its owner, as Stop)
  pauses a `running` task: the pause is stored (who, when, the live attempt) and the run is
  frozen as an update freezes it, under the update's freeze lock. A freeze the kernel has not
  confirmed is asked again every 2 s, 5 times, then thawed. A run that cannot be frozen (or did not
  settle) pauses at its turn boundary instead: its `Stop` and `UserPromptSubmit` hooks are held,
  after the update's hold and before any other hook work (so the drain still sees its pause point
  and no rotation runs), `pending` until held. Pausing a paused task changes nothing; any other
  state is a conflict ("Only a running task can be paused"). A pause acts only on the attempt it
  was taken on.
- **PER-002 Resume.** `POST /api/tasks/{tid}/resume` clears the pause and thaws the run (the held
  hooks are answered); resuming a task that is not paused changes nothing. While an update holds
  the task, resume only clears the pause: the update thaws and releases it with the others.
- **PER-003 An update never wakes it.** No update thaw (per task, all at once, the sweeps at the
  end of a run and at start) thaws the run of a task a person paused; a held hook stays held after
  the update releases it. A server that starts with a person's pause on a live run freezes it again
  (idempotent); if it cannot read the pauses it thaws nothing at start.
- **PER-004 Stopping and ending.** Every way Yulius ends a run (stop, close, a switch's restart,
  archive or delete: the shared shutdown path) thaws a paused run and clears the pause before it
  interrupts the agent, so the agent ends as it would unpaused. An ended task never shows a pause,
  and a pause left on an older attempt does nothing.
- **PER-005 Time does not run against it.** While a person's pause holds, the launch-goal watch
  does not count (as for an update), and a switch is refused ("Resume the task first").
- **PER-006 Buttons.** The task menu (the task view's Task actions, the sidebar's and the Tasks
  list's row menu) has Pause for a running task (unavailable otherwise, saying why) and Resume in
  its place while a person's pause holds it. A paused task's view says why above the terminal
  ("Paused by @user. What you type waits until it resumes.") with Resume (primary). Both call the
  routes and show the server's error as a toast. What a person types into a paused task's terminal
  waits in it until the task resumes.

## Plan

- [ ] Read the product context and related work, then write and validate the spec
- [ ] Live updates never go silently stale: the event stream has a heartbeat and a watchdog
- [ ] A task an update pauses reads Paused in the list, the sidebar and its view
- [ ] A person pauses and resumes a running task (stored, frozen, thawed)
- [ ] An update or a restart never wakes a task a person paused; ending or stopping it clears the pause
- [ ] Pause and Resume buttons in the task view and the Tasks list, with e2e tests
- [ ] Independent review in a fresh session, findings fixed test-first
- [ ] Every gate green, docs and the changes log updated

## Acceptance criteria

- **AC-LIV-1.** Rust: a client of `/api/events` receives a heartbeat first and another within the
  configured interval with no event published.
- **AC-LIV-2.** e2e (mock, short heartbeat): the mock stream goes silent without closing (as a dead
  connection); the page reconnects by itself and a task changed meanwhile shows its new state in
  the Tasks page and the sidebar without a reload. Fails before the change (the old state stays).
- **AC-LIV-3.** unit (`node --test` of `termsocket.js` or e2e): a socket that heard a message
  resets its backoff; one silent past its limit is closed and reconnected.
- **AC-PAU-1.** Rust: while an update holds tasks, the summary of a live task has
  `pause.by = "update"`, a `task.updated` is published when the hold goes on and off, and the
  pause is gone after the release.
- **AC-PAU-2.** e2e: a task whose summary has `pause.by = "update"` reads Paused with the update
  line in the Tasks page and the sidebar.
- **AC-PER-1.** Rust: pause freezes the live run (the fake host records it), stores who and when,
  publishes the pause; resume thaws it and clears it; pause of a queued task is a conflict; a
  watcher who may not work on the task is refused.
- **AC-PER-2.** Rust: with a person's pause, an update's resume and the start sweep leave its run
  frozen (the fake host's `thaw`/`thaw_all` never touch it); a server start freezes it again.
- **AC-PER-3.** Rust: a run that cannot be frozen is paused at its turn boundary (the Stop hook is
  held, `pending` until then), and resume releases it.
- **AC-PER-4.** Rust: stop of a paused task thaws then stops it; a pause of an older attempt is
  not shown and freezes nothing; resume while an update holds the task thaws nothing.
- **AC-PER-5.** e2e: Pause in the task view makes it Paused (Resume shown), Resume makes it Working;
  the row menu does the same.

## Test plan

- Integration tests (one binary, ADR-0028): `tests/task_pause.rs` (new: AC-PAU-1, AC-PER-1..4),
  the events test file for AC-LIV-1.
- `crates/yulius-sandbox/src/freeze.rs` unit test: `thaw_all` spares the kept groups.
- e2e: `e2e/tests/live-updates.e2e.ts` (new: AC-LIV-2), `e2e/tests/pause.e2e.ts` (new: AC-PAU-2,
  AC-PER-5); the mock gets the heartbeat, a silence switch, and the pause routes.
- Gates: `scripts/dev ci`, `scripts/dev security`, the full e2e suite.

## Decisions

- **Q1: why does the list not refresh?** No heartbeat on the event stream (Context). Chosen: an
  application heartbeat from the server and a watchdog in the page. Alternatives: WebSocket
  protocol pings alone (keep proxies awake but JavaScript cannot see them, so a dead connection
  still looks open); polling the lists every N seconds (load on every open tab, and still stale for
  N seconds); reconnecting on every visibility change (needless reconnects and resyncs).
- **Q2: one ADR (0154): Paused is an overlay on a live run, not a new `TaskStatus`.** A paused task
  keeps `running` and gains `pause`; the update's pause was invisible because it changed no state,
  and a person's pause has the same shape (the process lives, frozen). A new status would have to
  be threaded through every place that reads `running` (hooks, activity, chains, capacity,
  rotation, the supervisor's watches) and through the state machine's transitions, for no
  behaviour the overlay lacks. `waiting_capacity` stays the capacity pause it is.
- **Q3: how a person's pause stops the agent.** The cgroup freezer, as the update (ADR-0131),
  falling back to the turn-boundary hold. Alternatives: holding the hooks only (a `/goal` turn
  lasts hours, so Pause would do nothing for hours); interrupting the agent (it notices and loses
  its turn); `SIGSTOP` (ADR-0131 rejected it).
- **Q4: where the pause is stored.** Three columns on `tasks` (`paused_by`, `paused_at`,
  `paused_attempt`, migration 0033), as the control request (0018): a pause survives a
  restart, and the capacity `pauses` table stays capacity's (its resume loop and the Capacity card's hours read it). Assumed
  (reversible): a pause does not expire; the person resumes it.
- **Q5: who may pause.** Whoever may stop the task (`Action::Work`, its owner): pausing is a
  lighter stop. Assumed (reversible).
- **Q6: what a frozen run costs.** It keeps its session slot and memory, as an update's pause; the
  provider stream open at the freeze is cut by the proxy after its idle timeout and Claude Code
  retries after the thaw (ADR-0131 consequence). Wall-clock timers keep running while frozen: a
  tool call in flight longer than its timeout fails after the resume, which the agent sees and
  handles like any timeout. Assumed (reversible): no limit on how long a task stays paused; a
  person who wants the slot back stops it.
- **Q7: scope.** The three asks ship together: the update and the person's pause share the
  `pause` contract and the UI, and the refresh fix is what makes either visible. The split would be
  three small pull requests touching the same files in sequence. The spec review suggested
  shipping LIV first; kept together because the person's pause is what was asked for "in general"
  and LIV alone is about 60 lines.
