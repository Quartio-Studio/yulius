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
  const failedWatchOrder = new WeakMap();
  const rowOrder = new WeakMap();
  const verifiedGroup = new WeakMap(); // last verifier answer, before a failed read inferred Needs you
  const overviewOrder = new WeakMap();
  const prOrder = new WeakMap();

  // Events have second-resolution timestamps. Confirm a lifecycle change seen
  // during an explicit list read once, then preserve any changes during confirmation.
  const activeLists = new Set();
  const activeDetails = new Set();
  const terminalLifecycleRows = new WeakSet();

  function forgetDetailRow(taskId) {
    for (const read of activeDetails) if (read.row?.id === taskId) read.row = null;
  }

  async function loadTasks(sandboxId, watching = false) {
    const atStart = state.tasks.get(sandboxId);
    const order = ++nextListRead;
    const read = { sandboxId, order, changes: new Map(), beforeConfirmation: null, transitions: new Map(), lifecycle: new Set(), deleted: new Set(), failedWatchOrder: 0 };
    let snapshotOrder = order;
    let confirming = false;
    const path = `/api/sandboxes/${enc(sandboxId)}/tasks?archived=false`;
    activeLists.add(read);
    try {
      let list = await api.get(path);
      for (const task of Array.isArray(list) ? list : []) {
        if (task.status === 'parked' && read.changes.has(task.id)) read.lifecycle.add(task.id);
      }
      if ([...read.changes].some(([id, task]) => read.lifecycle.has(id) && (!watching || task.archived || task.status !== 'parked'))) {
        read.beforeConfirmation = read.changes;
        read.changes = new Map();
        confirming = true;
        const confirmationOrder = ++nextListRead;
        try {
          list = await api.get(path);
          snapshotOrder = confirmationOrder;
        } catch {
          read.changes = new Map([...read.beforeConfirmation, ...read.changes]);
        } finally {
          read.beforeConfirmation = null;
        }
      }
      const current = state.tasks.get(sandboxId);
      if (listOrder.get(current) > snapshotOrder) return current;
      const missedChange = pullWatch.get(sandboxId)?.changeBeforeParkedRows > snapshotOrder;
      const watchFailure = Math.max(read.failedWatchOrder, failedWatchOrder.get(current) ?? 0);
      const failureOrder = Math.max(watchFailure, pullWatch.get(sandboxId)?.changeBeforeParkedRows ?? 0);
      const map = new Map();
      if (watchFailure > snapshotOrder) failedWatchOrder.set(map, watchFailure);
      for (const task of Array.isArray(list) ? list : []) {
        if (task.archived) continue;
        map.set(task.id, orderedListRow(task, current?.get(task.id), snapshotOrder, failureOrder, read.transitions.has(task.id)));
      }
      for (const [id, task] of read.changes) {
        const listed = map.get(id);
        if (!watching && !confirming && !read.lifecycle.has(id) && !read.transitions.has(id) && listed && listed.status !== 'parked') continue;
        if (task.archived) {
          map.delete(id);
          continue;
        }
        const prior = current?.get(id);
        // A detail can supersede an event after the list saved it for replay.
        const latest = rowOrder.get(prior) > Math.max(snapshotOrder, rowOrder.get(task) ?? 0) ? prior : task;
        const fallbackOrder = Math.max(failureOrder, unverified.get(latest) ?? 0);
        const uncertain = latest.status === 'parked' && fallbackOrder >= rowOrder.get(latest);
        map.set(id, uncertain ? awaitingVerification(latest, listed ?? prior, fallbackOrder) : latest);
      }
      // A detail can discover a lifecycle change while this list is reading,
      // even for a row the older snapshot omitted. Later events and confirmed
      // lists still take precedence. Permanent removal wins in either order.
      for (const [id, transition] of read.transitions) {
        if (rowOrder.get(transition) <= snapshotOrder || read.changes.has(id)) continue;
        if (transition.archived) {
          map.delete(id);
          continue;
        }
        const latest = current?.get(id) ?? transition;
        // For two parked observations the list loop has already reconciled
        // the PR and verification, keeping the newer lifecycle fields.
        if (latest.status !== 'parked' || map.get(id)?.status !== 'parked') map.set(id, latest);
      }
      for (const id of read.deleted) map.delete(id);
      state.taskErrors.delete(sandboxId);
      listOrder.set(map, snapshotOrder);
      state.tasks.set(sandboxId, map);
      let changed = false;
      if (current) {
        for (const task of map.values()) {
          const prior = current.get(task.id);
          // Notify the terminal when a newer list supersedes its lifecycle
          // observation or restores a row that disappeared in an archive.
          const tracked = terminalLifecycleRows.has(prior);
          if (prior && task.status !== 'parked' && prior.status !== 'parked' && !tracked) continue;
          if (tracked && task.status !== 'parked') terminalLifecycleRows.add(task);
          if (!prior || JSON.stringify(task) !== JSON.stringify(prior) || pullForStatus(task) !== pullForStatus(prior)) {
            emit('task', task);
            changed = true;
          }
          // Only the open task view subscribes to its step list. The watched
          // summary can learn that its plan changed even if no step event arrived.
          if (prior?.status === 'parked' && task.status === 'parked' && task.steps &&
              (task.steps.done !== prior.steps?.done || task.steps.total !== prior.steps?.total ||
               (task.steps.current ?? null) !== (prior.steps?.current ?? null))) emit('steps', task.id);
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
        // Preserve the watch order across overlapping list and detail reads.
        if (current) failedWatchOrder.set(current, Math.max(order, failedWatchOrder.get(current) ?? 0));
        for (const older of activeLists) {
          if (older.sandboxId === sandboxId && older.order < order) older.failedWatchOrder = Math.max(older.failedWatchOrder, order);
        }
        let changed = false;
        for (const [id, task] of current || []) {
          if (task.status !== 'parked' || read.changes.has(id) || Math.max(rowOrder.get(task) ?? 0, unverified.get(task) ?? 0) > order) continue;
          const updated = awaitingVerification(task, undefined, order);
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

  // Parked status comes from the same verifier in the list and on the detail route.
  // Keep detail-only data (runs, transcripts, etc.) out of the shared task list.
  function parkedFacts(task) {
    return {
      status: task.status, archived: task.archived, activity: task.activity, steps: task.steps, spec_unfinished: task.spec_unfinished,
      goal_unfinished: task.goal_unfinished, held_prompt: task.held_prompt,
      needs_you: task.needs_you, needs_you_notice: task.needs_you_notice, staging_wait: task.staging_wait, staging_outcome: task.staging_outcome,
      parked_needs_you: task.parked_needs_you, parked_pr: task.parked_pr,
    };
  }

  function recordVerification(task, order) {
    if (typeof task.parked_needs_you === 'boolean') {
      verifiedGroup.set(task, { order, needsYou: task.parked_needs_you });
    }
    if (task.parked_pr) prOrder.set(task.parked_pr, order);
  }

  function latestVerification(task, other) {
    const first = verifiedGroup.get(task);
    const second = verifiedGroup.get(other);
    return (second?.order ?? 0) > (first?.order ?? 0) ? second : first;
  }

  function orderedListRow(listed, prior, snapshotOrder, failureOrder, keepLifecycleFields) {
    // A detail or lifecycle event that started later supplies the parked state.
    // The list can still carry a new title or discover an open PR.
    recordVerification(listed, snapshotOrder);
    const priorOrder = rowOrder.get(prior) ?? 0;
    const keepPrior = priorOrder > snapshotOrder && prior?.status === 'parked' && listed.status === 'parked';
    const order = keepPrior ? priorOrder : snapshotOrder;
    let row = listed;
    if (keepPrior) {
      row = keepLifecycleFields ? { ...prior } : { ...listed, ...parkedFacts(prior) };
      verifiedGroup.set(row, latestVerification(prior, listed));
      if (unverified.has(prior)) unverified.set(row, unverified.get(prior));
    }
    const fallbackOrder = Math.max(failureOrder, unverified.get(prior) ?? 0);
    if (row.status === 'parked' && fallbackOrder >= order) {
      row = awaitingVerification(row, keepPrior ? listed : prior, fallbackOrder);
    } else {
      unverified.delete(row);
    }
    rowOrder.set(row, order);
    verifiedGroup.set(row, latestVerification(row, prior));
    return row;
  }

  function publishParkedRow(map, row, updated) {
    if (updated.archived) {
      // Concurrent terminal reads still need the archive's observation order:
      // an older detail cannot restore it, and a newer detail can discover Undo.
      for (const pending of activeDetails) {
        if (pending.row?.id === row.id) pending.row = updated;
      }
      const removed = map.delete(row.id);
      // A list can omit the row before the terminal learns why it vanished.
      if (removed || !row.archived) emit('task', updated);
      if (removed) {
        emit('tasks', row.sandbox_id);
        scheduleCountsRefresh();
      }
      return updated;
    }
    if (updated !== row) {
      const restored = !map.has(row.id);
      map.set(row.id, updated);
      if (restored || JSON.stringify(updated) !== JSON.stringify(row) || pullForStatus(updated) !== pullForStatus(row)) {
        emit('task', updated);
        emit('tasks', row.sandbox_id);
        scheduleCountsRefresh();
      }
    }
    return updated;
  }

  // TaskDetail flattens TaskSummary together with private run data. Publish
  // the public summary on lifecycle changes, including a newly attached account.
  function lifecycleSummary(detail, prior) {
    // Fork links are absent when empty (SPEC-628): the detail's, never an older summary's.
    const summary = { ...parkedFacts(detail), account_id: detail.account_id, forked_from: detail.forked_from, forks: detail.forks };
    for (const key of ['id', 'sandbox_id', 'number', 'title', 'harness', 'harness_choice', 'branch', 'origin',
      'created_by', 'owner', 'control_request', 'control_from', 'owner_asks_first', 'created_at', 'updated_at',
      'state_since', 'working_since', 'account_label', 'mode', 'resumes_at', 'waits_for_account',
      'background_shells', 'background_shells_since', 'yulius_wait', 'merge_armed', 'after', 'next', 'attention',
      'auto', 'supertask', 'held_by', 'queue_reason', 'start_at', 'not_started', 'awaits_start_onboarding',
      'pause', 'pr_stalled', 'overlaps']) {
      if (key in detail) summary[key] = detail[key];
      else if (key in prior) summary[key] = prior[key];
    }
    return summary;
  }

  function rememberTransition(row, discovered = false) {
    if (!row.archived && row.status !== 'parked') terminalLifecycleRows.add(row);
    const order = rowOrder.get(row);
    for (const read of activeLists) {
      if (read.sandboxId !== row.sandbox_id || read.order >= order) continue;
      // An older detail may retain a newer event. Keep that event available
      // for confirmation and replay, including any PR the detail supplied.
      for (const events of [read.beforeConfirmation, read.changes]) {
        if (!events?.has(row.id)) continue;
        if (rowOrder.get(events.get(row.id)) === order) events.set(row.id, row);
        else events.delete(row.id);
      }
      if (discovered || row.status === 'parked') read.lifecycle.add(row.id);
      read.transitions.set(row.id, row);
    }
  }

  // Resting tasks can move into and out of the archive without changing
  // process status (TaskStatus::is_resting). Their detail verifies membership.
  function restingStatus(status) {
    return status === 'closed' || status?.startsWith('stopped_') || status?.startsWith('failed_');
  }

  function beginParkedDetail(startingRow) {
    const order = ++nextListRead;
    // The active list can lose a row before the mounted terminal learns
    // it was archived. Keep its identity until this detail read settles.
    const read = { row: state.tasks.get(startingRow?.sandbox_id)?.get(startingRow?.id) ?? startingRow };
    if (read.row) activeDetails.add(read);
    const release = () => activeDetails.delete(read);
    const publish = (detail, view = {}) => {
      release();
      const map = state.tasks.get(detail.sandbox_id);
      if (!map || read.row === null) return view;
      // Retain the observed task across an older list omission. A removal
      // event invalidates the reference before any pending detail can publish.
      const retained = read.row?.id === detail.id ? read.row : null;
      const listed = map.get(detail.id);
      const listVersion = listOrder.get(map) ?? 0;
      const laterEvent = rowOrder.get(retained) > Math.max(listVersion, rowOrder.get(listed) ?? 0);
      const crossedPark = startingRow && startingRow.status !== detail.status &&
        (startingRow.status === 'parked' || detail.status === 'parked');
      const undo = startingRow?.archived && !detail.archived;
      const verifiesMembership = detail.archived || startingRow?.archived || retained?.archived ||
        restingStatus(detail.status) || restingStatus(startingRow?.status);
      // An empty active list agrees with an archive. The detail still tells
      // the mounted terminal why its row disappeared.
      const confirmsAbsence = detail.archived && (rowOrder.get(retained) ?? 0) <= order;
      const retainOmitted = confirmsAbsence || ((crossedPark || verifiesMembership) && listVersion < order);
      const row = laterEvent ? retained : listed ?? (retainOmitted ? retained : null);
      if (!row) return undo ? { ...view, ...lifecycleSummary(startingRow, startingRow) } : view;
      if (!verifiesMembership && detail.status !== 'parked' && row.status !== 'parked' && startingRow?.status !== 'parked') return view;
      const transition = verifiesMembership || crossedPark || row.status !== detail.status;
      recordVerification(detail, order);
      if (rowOrder.get(row) > order) {
        // Preserve the newer event's work and decisions. An older detail may
        // still identify the open PR that an unverified update could not name.
        const supplement = detail.status === 'parked' && !detail.archived && row.status === 'parked' &&
          unverified.has(row) && (verifiedGroup.get(row)?.order ?? 0) < order;
        const latest = supplement ? awaitingVerification(row, detail) : row;
        if (transition) rememberTransition(latest);
        publishParkedRow(map, listed ?? row, latest);
        return { ...view, ...(transition ? lifecycleSummary(latest, latest) : parkedFacts(latest)) };
      }
      let updated = transition ? lifecycleSummary(detail, row) : { ...row, ...parkedFacts(detail) };
      const hintOrder = pullWatch.get(detail.sandbox_id)?.changeBeforeParkedRows ?? 0;
      verifiedGroup.set(updated, latestVerification(detail, row));
      const fallbackOrder = Math.max(hintOrder, unverified.get(row) ?? 0, failedWatchOrder.get(map) ?? 0);
      if (!detail.archived && detail.status === 'parked' && (fallbackOrder > order || typeof detail.parked_needs_you !== 'boolean')) {
        updated = awaitingVerification(updated, row, Math.max(order, fallbackOrder));
      }
      rowOrder.set(updated, order);
      if (transition) rememberTransition(updated, true);
      const missedHint = !detail.archived && detail.status === 'parked' && hintOrder > order && !hasParkedRows(map);
      publishParkedRow(map, row, updated);
      if (missedHint) requestRead(detail.sandbox_id, refreshParkedGroups, true, true);
      return { ...view, ...(transition ? updated : parkedFacts(updated)) };
    };
    publish.release = release;
    return publish;
  }

  function observedPr(task, snapshot) {
    // An unverified group may inherit its PR from a different read. Compare
    // the PR observations themselves, independently of grouping and chip text.
    const candidates = [task.parked_pr, snapshot?.parked_pr].filter(Boolean);
    const overview = state.pulls.get(task.sandbox_id);
    const git = overview?.get(task.id);
    if (git && ['open', 'closed', 'merged'].includes(git.state)) {
      const pr = { number: git.number, open: git.state === 'open' };
      prOrder.set(pr, overviewOrder.get(overview) ?? 0);
      candidates.push(pr);
    }
    candidates.sort((a, b) => (prOrder.get(b) ?? 0) - (prOrder.get(a) ?? 0));
    // A newer closure rules out an older observation of the same open PR.
    return candidates.find((pr, i) => pr.open && !candidates.slice(0, i).some(newer => newer.number === pr.number)) ?? candidates[0];
  }

  function upsertTask(task) {
    if (!task?.id) return;
    if (task.archived) forgetDetailRow(task.id);
    const map = state.tasks.get(task.sandbox_id);
    const prior = map?.get(task.id);
    const pending = [...activeLists].filter(read => read.sandboxId === task.sandbox_id);
    if (pending.some(read => read.deleted.has(task.id))) return;
    const recheck = task.status === 'parked' && !task.archived && typeof task.parked_needs_you !== 'boolean';
    const order = ++nextListRead;
    recordVerification(task, order);
    if (recheck) task = awaitingVerification(task, prior, order);
    rowOrder.set(task, order);
    if (task.status !== 'parked' && terminalLifecycleRows.has(prior)) terminalLifecycleRows.add(task);
    // A live list can replace a newer live event before a detail reveals that
    // the task crossed a park. Keep that event with the outstanding read.
    for (const read of activeDetails) if (read.row?.id === task.id) read.row = task;
    for (const read of pending) {
      if (task.archived || task.status === 'parked' || prior?.status === 'parked') read.lifecycle.add(task.id);
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
    forgetDetailRow(taskId);
    for (const read of activeLists) {
      if (!archived) read.deleted.add(taskId);
      else if (state.tasks.get(read.sandboxId)?.has(taskId)) {
        read.lifecycle.add(taskId);
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
      case 'commands.updated': emit('commands', event); break;
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
  const unverified = new WeakMap();
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
    // Mounted terminals add detail-only fields to a copy. An identical
    // list can verify the same row again without emitting a field update.
    const row = rowOrder.has(task) ? task : state.tasks.get(task.sandbox_id)?.get(task.id);
    if (pr?.state === 'open' && task.parked_needs_you === false && task.parked_pr?.number === pr.number &&
        row?.parked_needs_you === false && row.parked_pr?.number === pr.number &&
        ((!unverified.has(row) && rowOrder.get(row) > (overviewOrder.get(pulls) ?? 0)) ||
         (row.parked_pr.open === false && prOrder.get(row.parked_pr) > (overviewOrder.get(pulls) ?? 0))) &&
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

  function awaitingVerification(task, snapshot, order = unverified.get(task) ?? 0) {
    const pr = observedPr(task, snapshot);
    const verification = latestVerification(task, snapshot);
    // A verification can time out before naming the blocker. Keep that
    // unresolved decision until a later verifier answers it.
    const unnamedBlocker = [task, snapshot].some(row => row?.parked_needs_you === true && !row.parked_pr?.open &&
      (unverified.get(row) ?? 0) > (verification?.order ?? 0));
    const needsYou = pr?.open || unnamedBlocker || verification?.needsYou;
    // A fallback to an open PR is temporary. A later verification can close
    // that PR without overwriting a rename, plan or decision received meanwhile.
    let updated = task;
    if (needsYou !== task.parked_needs_you || pr?.open !== task.parked_pr?.open || pr?.number !== task.parked_pr?.number) {
      updated = { ...task, parked_needs_you: needsYou, parked_pr: pr };
      if (rowOrder.has(task)) rowOrder.set(updated, rowOrder.get(task));
    } else if (pr && task.parked_pr) {
      prOrder.set(task.parked_pr, prOrder.get(pr) ?? 0);
    }
    verifiedGroup.set(updated, verification);
    unverified.set(updated, order);
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
      // An overview can discover a replacement or a closure after the summary read failed.
      const tasks = state.tasks.get(sandboxId);
      let changed = false;
      for (const [id, pr] of pulls) {
        const prior = tasks?.get(id);
        if (!['open', 'closed', 'merged'].includes(pr.state) || !unverified.has(prior)) continue;
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
    beginParkedDetail,
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
