// Application state store and the UI event stream (docs/api.md "UI event stream").
// Topics: sandboxes, tasks (sid), task (TaskSummary), task-removed (tid), steps (tid),
// message ({task_id, message}), browser-action (event), automation-run (event),
// account (Account), update ({phase}), resync, events-status (status), pulls (sandbox id),
// secrets ({sandbox_id}: an agent asked about an agent secret, SPEC-209).
//
// The server sends `resync` (not `sandbox.updated`) after sandbox changes, and sandbox
// counters (running / needs you / queued) are not pushed at all: task events and a changed
// pull request schedule one debounced reload of /api/sandboxes so the tab badges stay current.

import { enc } from './api.js';
import { ReconnectingSocket } from './termsocket.js';

const ORDER_KEY = 'yulius.sandboxOrder';
/** `yulius.pulls.<user id>.<sandbox id>`: the pull requests the last Git overview read said (SPEC-104). */
const PULLS_PREFIX = 'yulius.pulls.';
/** Delay before task events refresh the sandbox counters (one reload per burst). */
const COUNTS_REFRESH_MS = 1500;
// How often the server's event stream beats (`events_heartbeat_ms`, SPEC-191 LIV-001).
const EVENTS_HEARTBEAT_MS = 20000;

function readJson(key, fallback) {
  try {
    const raw = window.localStorage.getItem(key);
    if (!raw) return fallback;
    const value = JSON.parse(raw);
    // A stored value of the wrong shape (edited by hand, an older version) falls back instead of
    // breaking the page: arrays stay arrays, objects stay objects, and so on.
    if (fallback !== null && fallback !== undefined) {
      if (Array.isArray(fallback) !== Array.isArray(value) || typeof value !== typeof fallback || value === null) return fallback;
    }
    return value;
  } catch {
    return fallback;
  }
}

function writeJson(key, value) {
  try { window.localStorage.setItem(key, JSON.stringify(value)); } catch { /* storage unavailable */ }
}

/** The real clock of pull request reads; the mock can give its own (SPEC-144, ADR-0118). */
const REAL_CLOCK = {
  now: () => Date.now(),
  setTimeout: (fn, ms) => setTimeout(fn, ms),
  clearTimeout: (id) => clearTimeout(id),
  setInterval: (fn, ms) => setInterval(fn, ms),
  clearInterval: (id) => clearInterval(id),
};

/**
 * Create the store bound to an API client. `pullGapMs`: the least time between the starts of a
 * sandbox's pull request reads that an announced change brings (SPEC-108). `clock`: the time and
 * timers those reads are planned with (tests drive a manual one, SPEC-144).
 */
export function createStore(api, { pullGapMs = 3000, clock = REAL_CLOCK } = {}) {
  const subscribers = new Map();
  const state = {
    me: null,
    sandboxes: [],
    tasks: new Map(), // sandbox id → Map(task id → TaskSummary)
    taskErrors: new Map(), // sandbox id → the error of its last failed list load
    sandboxesError: null, // the error of the last failed sandbox list load
    actionRequests: new Map(), // sandbox id → Map(tab id → {reason, task_id})
    pulls: new Map(), // sandbox id → Map(task id → pull request), from the sandbox's Git overview
    gitRows: new Map(), // sandbox id → Map(task id → overview row: files_changed, insertions, deletions, ahead, behind)
    eventsStatus: 'closed',
  };
  let events = null;
  let countsTimer = null;
  let countsInFlight = false;
  let countsAgain = false; // asked for while a reload ran: run once more after it

  function on(topic, fn) {
    if (!subscribers.has(topic)) subscribers.set(topic, new Set());
    subscribers.get(topic).add(fn);
    return () => subscribers.get(topic).delete(fn);
  }

  function emit(topic, payload) {
    for (const fn of [...(subscribers.get(topic) || [])]) {
      try { fn(payload); } catch (error) { console.error(`store listener for ${topic} failed`, error); }
    }
  }

  // User-defined order (drag and drop) first; sandboxes never ordered keep the server's order.
  function orderedSandboxes(list) {
    const order = readJson(ORDER_KEY, []);
    const rank = new Map((Array.isArray(order) ? order : []).map((id, i) => [id, i]));
    const position = new Map(list.map((s, i) => [s.id, i]));
    return [...list].sort((a, b) => {
      const ra = rank.has(a.id) ? rank.get(a.id) : 1e6;
      const rb = rank.has(b.id) ? rank.get(b.id) : 1e6;
      return ra - rb || position.get(a.id) - position.get(b.id);
    });
  }

  async function loadSandboxes() {
    let list;
    try {
      list = await api.get('/api/sandboxes');
    } catch (error) {
      state.sandboxesError = error; // views say so instead of "no sandboxes" (CODE-12)
      throw error;
    }
    state.sandboxesError = null;
    state.sandboxes = orderedSandboxes(Array.isArray(list) ? list : []);
    emit('sandboxes');
    return state.sandboxes;
  }

  /** Persist a user-defined tab order (drag and drop in the sandbox tab row). */
  function setSandboxOrder(ids) {
    writeJson(ORDER_KEY, ids);
    state.sandboxes = orderedSandboxes(state.sandboxes);
    emit('sandboxes');
  }

  function upsertSandbox(sandbox) {
    const index = state.sandboxes.findIndex((s) => s.id === sandbox.id);
    if (index >= 0) state.sandboxes[index] = sandbox;
    else state.sandboxes = orderedSandboxes([...state.sandboxes, sandbox]);
    emit('sandboxes');
  }

  function removeSandbox(id) {
    state.sandboxes = state.sandboxes.filter((s) => s.id !== id);
    state.tasks.delete(id);
    emit('sandboxes');
  }

  // Cold navigation and resync can both load the list. Keep the newest successful
  // snapshot even when the other request fails.
  let nextListRead = 0;
  const listOrder = new WeakMap();
  const parkedRowOrder = new WeakMap();
  const overviewOrder = new WeakMap();

  // Events have second-resolution timestamps. Confirm a lifecycle change seen
  // during an explicit list read once, then preserve any changes during confirmation.
  const activeLists = new Set();

  async function loadTasks(sandboxId, watching = false) {
    const atStart = state.tasks.get(sandboxId);
    const order = ++nextListRead;
    const read = { sandboxId, order, changes: new Map(), parked: new Set(), deleted: new Set(), failedWatchOrder: 0 };
    let snapshotOrder = order;
    let confirming = false;
    const path = `/api/sandboxes/${enc(sandboxId)}/tasks?archived=false`;
    activeLists.add(read);
    try {
      let list = await api.get(path);
      for (const task of Array.isArray(list) ? list : []) {
        if (task.status === 'parked' && read.changes.has(task.id)) read.parked.add(task.id);
      }
      if ([...read.changes].some(([id, task]) => read.parked.has(id) && (!watching || task.archived || task.status !== 'parked'))) {
        const before = read.changes;
        read.changes = new Map();
        confirming = true;
        const confirmationOrder = ++nextListRead;
        try {
          list = await api.get(path);
          snapshotOrder = confirmationOrder;
        } catch {
          read.changes = new Map([...before, ...read.changes]);
        }
      }
      const current = state.tasks.get(sandboxId);
      if (listOrder.get(current) > snapshotOrder) return current;
      const missedChange = pullWatch.get(sandboxId)?.changeBeforeParkedRows > snapshotOrder;
      const needsVerification = missedChange || read.failedWatchOrder > snapshotOrder;
      const map = new Map();
      for (const task of Array.isArray(list) ? list : []) {
        if (task.archived) continue;
        unverified.delete(task);
        if (task.status === 'parked' && task.parked_needs_you === false) parkedRowOrder.set(task, snapshotOrder);
        map.set(task.id, needsVerification && task.status === 'parked' ? awaitingVerification(task) : task);
      }
      for (const [id, task] of read.changes) {
        const listed = map.get(id);
        if (!watching && !confirming && !read.parked.has(id) && listed && listed.status !== 'parked') continue;
        if (task.archived) map.delete(id);
        else if (unverified.has(task)) map.set(id, awaitingVerification(task, map.get(id)));
        else map.set(id, task);
      }
      for (const id of read.deleted) map.delete(id);
      state.taskErrors.delete(sandboxId);
      listOrder.set(map, snapshotOrder);
      state.tasks.set(sandboxId, map);
      let changed = false;
      if (current) {
        for (const task of map.values()) {
          const prior = current.get(task.id);
          if (task.status !== 'parked' && prior?.status !== 'parked') continue;
          if (JSON.stringify(task) !== JSON.stringify(prior)) { emit('task', task); changed = true; }
        }
      }
      emit('tasks', sandboxId);
      if (watching && changed) scheduleCountsRefresh();
      if (missedChange && !hasParkedRows(current) && hasParkedRows(map)) {
        requestRead(sandboxId, refreshParkedGroups, true, true);
      }
      return map;
    } catch (error) {
      const current = state.tasks.get(sandboxId);
      if (listOrder.get(current) > order) return current;
      if (watching) {
        for (const older of activeLists) {
          if (older.sandboxId === sandboxId && older.order < order) older.failedWatchOrder = Math.max(older.failedWatchOrder, order);
        }
        let changed = false;
        for (const [id, task] of current || []) {
          if (task.status !== 'parked' || read.changes.has(id)) continue;
          const updated = awaitingVerification(task);
          if (updated !== task) {
            current.set(id, updated);
            emit('task', updated);
            changed = true;
          }
        }
        if (changed) { emit('tasks', sandboxId); scheduleCountsRefresh(); }
        return current;
      }
      if (current === atStart) {
        state.taskErrors.set(sandboxId, error);
        emit('tasks', sandboxId);
      }
      throw error;
    } finally {
      activeLists.delete(read);
    }
  }

  function knownOpenPr(prior, sandboxId, id) {
    const pr = state.pulls.get(sandboxId)?.get(id);
    if (pr?.state === 'open') return { number: pr.number, open: true };
    return prior?.parked_pr?.open ? prior.parked_pr : null;
  }

  function upsertTask(task) {
    if (!task?.id) return;
    const map = state.tasks.get(task.sandbox_id);
    const prior = map?.get(task.id);
    const pending = [...activeLists].filter(read => read.sandboxId === task.sandbox_id);
    if (pending.some(read => read.deleted.has(task.id))) return;
    const recheck = task.status === 'parked' && !task.archived && typeof task.parked_needs_you !== 'boolean';
    if (recheck) {
      const openPr = knownOpenPr(prior, task.sandbox_id, task.id);
      if (openPr) task = { ...task, parked_pr: openPr, parked_needs_you: true };
      unverified.add(task);
    }
    for (const read of pending) {
      if (task.status === 'parked' || prior?.status === 'parked') read.parked.add(task.id);
      read.changes.set(task.id, task);
    }
    if (map) {
      if (task.archived) map.delete(task.id);
      else map.set(task.id, task);
      emit('tasks', task.sandbox_id);
    }
    emit('task', task);
    if (recheck) requestRead(task.sandbox_id, refreshParkedGroups, true, true);
  }

  /** Archive can be undone. A permanent deletion wins even over a delayed response. */
  function removeTask(taskId, { byUser = false, archived = false } = {}) {
    for (const read of activeLists) {
      if (!archived) read.deleted.add(taskId);
      else if (state.tasks.get(read.sandboxId)?.has(taskId)) {
        if (state.tasks.get(read.sandboxId).get(taskId).status === 'parked') read.parked.add(taskId);
        read.changes.set(taskId, { archived: true });
      }
    }
    for (const [sandboxId, map] of state.tasks) {
      if (map.delete(taskId)) emit('tasks', sandboxId);
    }
    emit('task-removed', { taskId, byUser });
  }

  /** Non-archived tasks of a sandbox, most recently updated first. */
  function tasksOf(sandboxId) {
    const map = state.tasks.get(sandboxId);
    if (!map) return null;
    return [...map.values()].sort((a, b) => String(b.updated_at).localeCompare(String(a.updated_at)) || b.number - a.number);
  }

  function findTask(taskId) {
    for (const map of state.tasks.values()) if (map.has(taskId)) return map.get(taskId);
    return null;
  }

  function addActionRequest(event) {
    let map = state.actionRequests.get(event.sandbox_id);
    if (!map) {
      map = new Map();
      state.actionRequests.set(event.sandbox_id, map);
    }
    map.set(event.tab_id, { reason: event.reason, task_id: event.task_id });
    emit('browser-action', event);
  }

  function clearActionRequest(sandboxId, tabId) {
    const map = state.actionRequests.get(sandboxId);
    if (map && map.delete(tabId)) emit('browser-action', { sandbox_id: sandboxId, tab_id: tabId, cleared: true });
  }

  /** Reload /api/sandboxes once after a burst of task events (bounded: one timer, one request). */
  function scheduleCountsRefresh() {
    if (countsTimer || !state.me) return;
    countsTimer = setTimeout(async () => {
      countsTimer = null;
      if (!state.me) return;
      // A reload already running may have read the server before the change: one more after it.
      if (countsInFlight) { countsAgain = true; return; }
      countsInFlight = true;
      try { await loadSandboxes(); } catch { /* the next event or resync retries */ } finally {
        countsInFlight = false;
        if (countsAgain) { countsAgain = false; scheduleCountsRefresh(); }
      }
    }, COUNTS_REFRESH_MS);
  }

  function handleEvent(event) {
    switch (event.type) {
      case 'task.updated': upsertTask(event.task); scheduleCountsRefresh(); break;
      case 'task.removed': removeTask(event.task_id); scheduleCountsRefresh(); break;
      case 'task.control_taken_back': emit('control-taken-back', event); break;
      case 'task.control_taken': emit('control-taken', event); break;
      case 'steps.updated': emit('steps', event.task_id); break;
      case 'browser.action_requested': addActionRequest(event); break;
      case 'browser.action_cleared': clearActionRequest(event.sandbox_id, event.tab_id); break;
      case 'automation.run': emit('automation-run', event); break;
      case 'account.updated': emit('account', event.account); break;
      case 'update.updated': emit('update', event); break;
      case 'disk.updated': emit('disk', event); break;
      case 'digest.ready': emit('digest', event.digest); break;
      case 'handoffs.updated': emit('handoffs', event.task_id); break;
      case 'connections.updated': emit('connections', event); break;
      case 'note.updated': emit('note', event); break;
      case 'note.presence': emit('note-presence', event); break;
      case 'file.presence': emit('file-presence', event); break;
      case 'file.live': emit('file-live', event); break;
      case 'pulls.updated': pullsChanged(event.sandbox_id); break;
      case 'secrets.updated': emit('secrets', event); break;
      case 'superagent.requests': emit('superagent-requests', event); break;
      case 'superagent.link': emit('superagent-link', event); break;
      case 'resync': emit('resync'); break;
      default: break;
    }
  }

  const eventsHeartbeatMs = () => (api.transport && api.transport.eventsHeartbeatMs) || EVENTS_HEARTBEAT_MS;

  /** Connect the event stream; a reconnect triggers a resync (events may have been missed). */
  function startEvents() {
    if (events) return;
    events = new ReconnectingSocket({
      transport: api.transport,
      path: '/api/events',
      // The event stream should outlive long outages; cap the delay at 10 s but keep trying for ~1 h.
      onOpen: (_socket, reconnect) => { if (reconnect) emit('resync'); },
      onMessage: (data) => {
        if (typeof data !== 'string') return;
        let event;
        try { event = JSON.parse(data); } catch { return; }
        if (event && typeof event.type === 'string') handleEvent(event);
      },
      onStatus: (status) => { state.eventsStatus = status; emit('events-status', status); },
      // The server beats every 20 s (SPEC-191 LIV-001): a stream silent for 2.5 beats is dead.
      staleMs: 2.5 * eventsHeartbeatMs(),
    });
    events.backoff.maxAttempts = 360;
    events.start();
  }

  // Pull requests per sandbox, for the sidebar and the Tasks page: one read of the Git overview
  // shared by every view watching the sandbox, refreshed every minute while the tab is in view and
  // online and when it is shown again (at most every 15 s) — never while hidden (PERF-12). The
  // server says when they changed (`pulls.updated`, SPEC-108): read while in view, at least
  // `pullGapMs` after the last read started (a push announces several times), or as soon as the
  // tab is shown again. Reads of a sandbox never overlap, so an older one cannot answer last, and a
  // change announced during a read brings one more read after it.
  const pullWatch = new Map(); // sandbox id → view count, poll timer and each source's read schedule
  const sources = [refreshParkedGroups, loadGitOverview];
  const inFlight = new Map(sources.map(source => [source, new Map()]));
  // A failed read may need a conservative display when Git later discovers an open PR.
  const unverified = new WeakSet();
  const pollPaused = () => document.hidden || navigator.onLine === false;
  // The overview takes seconds (GitHub and Git for every task), so this browser keeps what the
  // last read said, per user, and a page load shows it at once until its own read replaces it
  // (SPEC-104). Only for a watched sandbox, whose read is on its way: elsewhere (a notification
  // about another sandbox) an old read would stay unreplaced. A stored value of the wrong shape is
  // ignored.
  const pullsKey = (sandboxId) => (state.me && state.me.user && state.me.user.id ? `${PULLS_PREFIX}${state.me.user.id}.${sandboxId}` : null);
  const isPullRow = (row) => Array.isArray(row) && typeof row[0] === 'string' && row[1] !== null && typeof row[1] === 'object'
    && Number.isInteger(row[1].number) && typeof row[1].state === 'string';
  const seeded = new Set(); // sandbox ids whose pull requests came from storage, not a read yet
  function pullsOf(sandboxId) {
    if (!state.pulls.has(sandboxId) && pullWatch.has(sandboxId)) {
      const key = pullsKey(sandboxId);
      if (key) {
        state.pulls.set(sandboxId, new Map(readJson(key, []).filter(isPullRow)));
        seeded.add(sandboxId);
      }
    }
    return state.pulls.get(sandboxId) || new Map();
  }
  // Keep the real chip visible. Only disregard a blocking overview for grouping
  // when a later list verified this very PR. A new overview gets its own order.
  function pullForStatus(task) {
    const pulls = pullsOf(task.sandbox_id);
    const pr = pulls.get(task.id);
    if (pr?.state === 'open' && task.parked_needs_you === false && task.parked_pr?.number === pr.number &&
        parkedRowOrder.get(task) > (overviewOrder.get(pulls) ?? 0) &&
        (pr.conflicts || !['success', 'none'].includes(pr.checks))) return null;
    return pr;
  }

  /** Sign-out or an expired session: the pull requests (titles included) do not stay in this browser. */
  function forgetPulls() {
    state.pulls.clear();
    seeded.clear();
    try {
      for (const key of Object.keys(window.localStorage)) if (key.startsWith(PULLS_PREFIX)) window.localStorage.removeItem(key);
    } catch { /* storage unavailable: nothing kept */ }
  }
  // A rerun on the same open PR changes its checks without changing the task's
  // delivery stage. Refresh the server's parked groups alongside the watched
  // overview. An SSE update or list replacement during this read takes precedence.
  function hasParkedRows(tasks) {
    return tasks && [...tasks.values()].some(task => task.status === 'parked');
  }

  async function refreshParkedGroups(sandboxId) {
    if (hasParkedRows(state.tasks.get(sandboxId))) await loadTasks(sandboxId, true);
  }

  function awaitingVerification(task, snapshot) {
    const prior = task.parked_pr?.open ? task : snapshot ?? task;
    const pr = knownOpenPr(prior, task.sandbox_id, task.id);
    const alreadyWaiting = task.parked_needs_you && task.parked_pr?.open && task.parked_pr.number === pr?.number;
    const updated = pr && !alreadyWaiting ? { ...task, parked_needs_you: true, parked_pr: pr } : task;
    unverified.add(updated);
    return updated;
  }

  async function loadGitOverview(sandboxId) {
    const order = ++nextListRead;
    try {
      const git = await api.get(`/api/sandboxes/${enc(sandboxId)}/git`);
      const rows = Array.isArray(git.tasks) ? git.tasks : [];
      const pulls = rows.filter((t) => t.pr).map((t) => [t.task_id, t.pr]);
      const overview = new Map(pulls);
      overviewOrder.set(overview, order);
      state.pulls.set(sandboxId, overview);
      // A replacement PR can become known after the independent summary read failed.
      const tasks = state.tasks.get(sandboxId);
      let changed = false;
      for (const [id, pr] of pulls) {
        const prior = tasks?.get(id);
        if (pr.state !== 'open' || !unverified.has(prior)) continue;
        const updated = awaitingVerification(prior);
        if (updated === prior) continue;
        tasks.set(id, updated);
        emit('task', updated);
        changed = true;
      }
      if (changed) emit('tasks', sandboxId);
      seeded.delete(sandboxId);
      const key = pullsKey(sandboxId);
      if (key) writeJson(key, pulls);
      // The server counts "needs you" from what this read saw (a tested or failing pull request),
      // and after a restart it had seen nothing yet: reload the counters after every read, so the
      // tab and Home agree with the sidebar (#147). One debounced request per burst.
      scheduleCountsRefresh();
      state.gitRows.set(sandboxId, new Map(rows.map((t) => [t.task_id, t])));
      emit('pulls', sandboxId);
    } catch {
      // The chips of an earlier read on this page stay; stored ones of any age do not pass for
      // current when this page could not read (SPEC-104), shown already or not (an empty entry
      // keeps pullsOf from reading them later): the lists work without them.
      if (seeded.delete(sandboxId) || !state.pulls.has(sandboxId)) {
        state.pulls.set(sandboxId, new Map());
        emit('pulls', sandboxId);
      }
    }
  }
  // Each source has its own minimum gap and one in-flight request per sandbox.
  // A fresh hint during a request survives until that source can read again.
  function requestRead(id, source, fresh = false, paced = false) {
    const schedule = pullWatch.get(id)?.schedules.get(source);
    if (!schedule) return;
    if (pollPaused()) { schedule.due ||= fresh; return; }
    const delay = schedule.at + pullGapMs - clock.now();
    if (paced && delay > 0) {
      if (schedule.soon == null) schedule.soon = clock.setTimeout(() => {
        schedule.soon = null;
        requestRead(id, source, true, true);
      }, delay);
      return;
    }
    const reads = inFlight.get(source);
    const running = reads.get(id);
    if (running) { running.again ||= fresh; return; }
    clock.clearTimeout(schedule.soon);
    Object.assign(schedule, { at: clock.now(), due: false, soon: null });
    const read = { again: false };
    reads.set(id, read);
    void source(id).finally(() => {
      reads.delete(id);
      if (read.again) requestRead(id, source, true, true);
    });
  }

  function pullsChanged(id) {
    const watch = pullWatch.get(id);
    if (watch && !hasParkedRows(state.tasks.get(id))) watch.changeBeforeParkedRows = ++nextListRead;
    for (const source of sources) requestRead(id, source, true, true);
  }

  function watchPulls(id) {
    let entry = pullWatch.get(id);
    if (!entry) {
      const schedules = new Map(sources.map(source => [source, { at: 0, due: false, soon: null }]));
      const timer = clock.setInterval(() => {
        for (const source of sources) requestRead(id, source);
      }, 60_000);
      entry = { views: 0, schedules, timer };
      pullWatch.set(id, entry);
      // A read from the previous watch can predate announcements nobody received.
      for (const reads of inFlight.values()) {
        const running = reads.get(id);
        if (running) running.again = true;
      }
    }
    entry.views++;
    for (const [source, schedule] of entry.schedules) {
      if (clock.now() - schedule.at > 15_000) requestRead(id, source);
    }
    let done = false;
    return () => {
      if (done) return;
      done = true;
      if (--entry.views === 0) {
        clock.clearInterval(entry.timer);
        for (const schedule of entry.schedules.values()) clock.clearTimeout(schedule.soon);
        pullWatch.delete(id);
      }
    };
  }
  const refreshPulls = () => {
    if (pollPaused()) return;
    for (const [id, entry] of pullWatch) {
      for (const [source, schedule] of entry.schedules) {
        if (schedule.due || clock.now() - schedule.at > 15_000) requestRead(id, source, schedule.due);
      }
    }
  };
  document.addEventListener('visibilitychange', refreshPulls);
  window.addEventListener('focus', refreshPulls);
  // After lost events or a restart, read the watched pull requests again at once: the server's
  // "needs you" counts start from what such a read sees (#147).
  on('resync', () => { for (const id of pullWatch.keys()) for (const source of sources) requestRead(id, source, true); });

  function stopEvents() {
    if (events) events.stop();
    events = null;
    clearTimeout(countsTimer);
    countsTimer = null;
  }

  return {
    state,
    on,
    emit,
    loadSandboxes,
    setSandboxOrder,
    upsertSandbox,
    removeSandbox,
    loadTasks,
    upsertTask,
    removeTask,
    tasksOf,
    tasksError: (sandboxId) => state.taskErrors.get(sandboxId) || null,
    findTask,
    clearActionRequest,
    handleEvent,
    startEvents,
    stopEvents,
    sandboxBySlug: (slug) => state.sandboxes.find((s) => s.slug === slug) || null,
    sandboxById: (id) => state.sandboxes.find((s) => s.id === id) || null,
    retryEvents: () => events && events.retry(),
    /**
     * Replace a stream that looks open but missed its beat (the tab shown again, online): silent
     * for 1.5 beats, so a tab shown just as a beat is due does not reconnect for nothing.
     */
    checkEvents: () => Boolean(events && events.checkAlive(1.5 * eventsHeartbeatMs())),
    /** Keeps the sandbox's pull requests fresh while a view shows them; returns a stop function. */
    watchPulls,
    /** The sandbox's pull requests by task id: the last overview read, or the stored one before it. */
    pullsOf,
    pullForStatus,
    forgetPulls,
    /** A task's row of its sandbox's Git overview (changed files and lines), or null. */
    gitRowOf: (sandboxId, taskId) => (state.gitRows.get(sandboxId) || new Map()).get(taskId) || null,
  };
}

export { readJson, writeJson };
