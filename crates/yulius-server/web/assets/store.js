// Application state store and the UI event stream (docs/api.md "UI event stream").
// Topics: sandboxes, tasks (sid), task (TaskSummary), task-removed (tid), steps (tid),
// message ({task_id, message}), browser-action (event), automation-run (event),
// account (Account), resync, events-status (status).
//
// The server sends `resync` (not `sandbox.updated`) after sandbox changes, and sandbox
// counters (running / needs attention / queued) are not pushed at all: task events
// schedule one debounced reload of /api/sandboxes so the tab badges stay current.

import { enc } from './api.js';
import { ReconnectingSocket } from './termsocket.js';

const ORDER_KEY = 'yulius.sandboxOrder';
/** Delay before task events refresh the sandbox counters (one reload per burst). */
const COUNTS_REFRESH_MS = 1500;

function readJson(key, fallback) {
  try {
    const raw = window.localStorage.getItem(key);
    return raw ? JSON.parse(raw) : fallback;
  } catch {
    return fallback;
  }
}

function writeJson(key, value) {
  try { window.localStorage.setItem(key, JSON.stringify(value)); } catch { /* storage unavailable */ }
}

/** Create the store bound to an API client. */
export function createStore(api) {
  const subscribers = new Map();
  const state = {
    me: null,
    sandboxes: [],
    tasks: new Map(), // sandbox id → Map(task id → TaskSummary)
    actionRequests: new Map(), // sandbox id → Map(tab id → {reason, task_id})
    eventsStatus: 'closed',
  };
  let events = null;
  let countsTimer = null;
  let countsInFlight = false;

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
    const list = await api.get('/api/sandboxes');
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

  async function loadTasks(sandboxId) {
    const list = await api.get(`/api/sandboxes/${enc(sandboxId)}/tasks?archived=false`);
    const map = new Map();
    for (const task of Array.isArray(list) ? list : []) if (!task.archived) map.set(task.id, task);
    state.tasks.set(sandboxId, map);
    emit('tasks', sandboxId);
    return map;
  }

  function upsertTask(task) {
    if (!task || !task.id) return;
    let map = state.tasks.get(task.sandbox_id);
    if (!map) {
      map = new Map();
      state.tasks.set(task.sandbox_id, map);
    }
    if (task.archived) map.delete(task.id);
    else map.set(task.id, task);
    emit('tasks', task.sandbox_id);
    emit('task', task);
  }

  function removeTask(taskId) {
    for (const [sandboxId, map] of state.tasks) {
      if (map.delete(taskId)) emit('tasks', sandboxId);
    }
    emit('task-removed', taskId);
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
      if (countsInFlight || !state.me) return;
      countsInFlight = true;
      try { await loadSandboxes(); } catch { /* the next event or resync retries */ } finally { countsInFlight = false; }
    }, COUNTS_REFRESH_MS);
  }

  function handleEvent(event) {
    switch (event.type) {
      case 'task.updated': upsertTask(event.task); scheduleCountsRefresh(); break;
      case 'task.removed': removeTask(event.task_id); scheduleCountsRefresh(); break;
      case 'steps.updated': emit('steps', event.task_id); break;
      case 'message': emit('message', { task_id: event.task_id, message: event.message }); break;
      case 'sandbox.updated': upsertSandbox(event.sandbox); break;
      case 'browser.action_requested': addActionRequest(event); break;
      case 'automation.run': emit('automation-run', event); break;
      case 'account.updated': emit('account', event.account); break;
      case 'resync': emit('resync'); break;
      default: break;
    }
  }

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
    });
    events.backoff.maxAttempts = 360;
    events.start();
  }

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
    findTask,
    clearActionRequest,
    handleEvent,
    startEvents,
    stopEvents,
    sandboxBySlug: (slug) => state.sandboxes.find((s) => s.slug === slug) || null,
    sandboxById: (id) => state.sandboxes.find((s) => s.id === id) || null,
    retryEvents: () => events && events.retry(),
  };
}

export { readJson, writeJson };
