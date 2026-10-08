// Notification center (SPEC-001 UI-020): what happened while the user looked elsewhere, in one
// list behind the bell in the tab row — tasks waiting for input, needing attention or failing,
// browser action requests, agent messages, automation runs, account problems and instance
// notices. Built from the event stream the UI already receives; toasts still show. Kept per
// user in this browser and bounded, so a reload keeps the history.

import { h, icon, setChildren, popoverFocus } from './dom.js';
import { relShort, dateTime } from './format.js';
import { statusInfo } from './status.js';
import { readJson, writeJson } from './store.js';

const MAX_ITEMS = 100;
const KINDS = new Set(['info', 'ok', 'warn', 'error']);
/** Task tones that notify when a task enters them, and the notification kind. */
const TASK_TONES = { waiting: 'info', attention: 'warn', failed: 'error', 'stopped-abnormal': 'error' };
const ACCOUNT_PROBLEMS = { relogin: 'needs to sign in again', limited: 'reached its usage limit', error: 'reports an error' };

function sanitize(list) {
  if (!Array.isArray(list)) return [];
  return list
    .filter((n) => n && typeof n.id === 'string' && typeof n.title === 'string' && typeof n.at === 'string')
    .map((n) => ({ ...n, kind: KINDS.has(n.kind) ? n.kind : 'info', route: typeof n.route === 'string' && n.route.startsWith('/') ? n.route : null }))
    .slice(0, MAX_ITEMS);
}

/** The bell, its panel and the notification list. `add` records one; events are watched. */
export function createNotificationCenter(ctx) {
  const user = (ctx.store.state.me && ctx.store.state.me.user) || {};
  const storageKey = `yulius.notifications.${user.id || user.login || 'anonymous'}`;
  let items = sanitize(readJson(storageKey, []));
  let seq = 0;
  const taskTones = new Map(); // task id → last tone seen this session
  const accountStatus = new Map(); // account id → last status seen this session

  const badge = h('span', { class: 'notif-badge', hidden: true });
  const button = h('button', { type: 'button', class: 'btn-icon notif-btn', title: 'Notifications', 'aria-label': 'Notifications', 'aria-haspopup': 'dialog', 'aria-expanded': 'false' }, icon('bell'), badge);
  let pop = null;
  let onDoc = null;
  let offFocus = null;
  let onKey = null;

  function save() {
    writeJson(storageKey, items);
  }

  function unread() {
    return items.filter((n) => !n.read).length;
  }

  function render() {
    const count = unread();
    badge.hidden = count === 0;
    badge.textContent = count > 99 ? '99+' : String(count);
    button.title = count ? `Notifications (${count} unread)` : 'Notifications';
    button.setAttribute('aria-label', button.title);
    if (pop) renderPanel();
  }

  /**
   * Records a notification. A `key` keeps one entry per subject (a task, a browser tab): a new
   * one replaces it and is unread again; with `once`, an existing entry is left as it is.
   */
  function add({ kind = 'info', title, detail = null, route = null, key = null, once = false }) {
    if (key) {
      const index = items.findIndex((n) => n.key === key);
      if (index >= 0) {
        if (once) return;
        items.splice(index, 1);
      }
    }
    seq += 1;
    items.unshift({ id: `${Date.now().toString(36)}-${seq}`, kind, title, detail, route, key, at: new Date().toISOString(), read: false });
    if (items.length > MAX_ITEMS) items.length = MAX_ITEMS;
    save();
    render();
  }

  function markRead(predicate) {
    let changed = false;
    for (const n of items) if (!n.read && predicate(n)) { n.read = true; changed = true; }
    if (changed) { save(); render(); }
  }

  // ------------------------------------------------------------ panel

  function renderPanel() {
    const head = h('div', { class: 'notif-head' },
      h('h2', null, 'Notifications'),
      h('span', { class: 'spacer' }),
      h('button', { type: 'button', class: 'btn btn-sm', disabled: unread() === 0, onClick: () => markRead(() => true) }, 'Mark all read'),
      h('button', { type: 'button', class: 'btn btn-sm', disabled: items.length === 0, onClick: () => { items = []; save(); render(); } }, 'Clear'));
    const rows = items.map((n) => h(n.route ? 'button' : 'div', {
      type: n.route ? 'button' : null,
      class: ['notif-row', `notif-${n.kind}`, !n.read && 'unread'],
      onClick: n.route ? () => { markRead((m) => m.id === n.id); close(); ctx.navigate(n.route); } : null,
    },
    h('span', { class: 'notif-mark', 'aria-hidden': 'true' }),
    h('span', { class: 'notif-text' },
      h('span', { class: 'notif-title' }, n.title),
      n.detail ? h('span', { class: 'notif-detail' }, n.detail) : null),
    h('span', { class: 'notif-time', title: dateTime(n.at) }, relShort(n.at))));
    setChildren(pop, head, items.length
      ? h('div', { class: 'notif-list' }, rows)
      : h('div', { class: 'notif-empty' }, 'Nothing yet. Tasks that wait for you, agent messages and browser requests show up here.'));
  }

  function open() {
    pop = h('div', { class: 'status-pop notif-pop', role: 'dialog', 'aria-label': 'Notifications' });
    document.body.append(pop);
    button.setAttribute('aria-expanded', 'true');
    renderPanel();
    offFocus = popoverFocus(pop, button, close);
    onDoc = (event) => { if (pop && !pop.contains(event.target) && !button.contains(event.target)) close(); };
    onKey = (event) => { if (event.key === 'Escape') { event.preventDefault(); event.stopPropagation(); close(); button.focus(); } };
    setTimeout(() => document.addEventListener('pointerdown', onDoc, true), 0);
    document.addEventListener('keydown', onKey, true); // before the drawer's Escape: innermost layer first
  }

  function close() {
    if (offFocus) offFocus();
    offFocus = null;
    document.removeEventListener('pointerdown', onDoc, true);
    document.removeEventListener('keydown', onKey, true);
    if (pop) pop.remove();
    pop = null;
    button.setAttribute('aria-expanded', 'false');
  }

  button.addEventListener('click', () => (pop ? close() : open()));

  // ------------------------------------------------------------ sources

  const offs = [];
  const watch = (topic, fn) => offs.push(ctx.store.on(topic, fn));

  function sandboxOf(id) {
    return ctx.store.sandboxById(id);
  }

  function taskRoute(task) {
    const sandbox = task && sandboxOf(task.sandbox_id);
    return sandbox ? `/s/${encodeURIComponent(sandbox.slug)}/t/${task.number}` : null;
  }

  /** The user is looking at this task right now: no need to tell them about it. */
  function watching(taskId) {
    const current = ctx.currentTask();
    return Boolean(current && current.id === taskId && document.visibilityState === 'visible' && document.hasFocus());
  }

  watch('task', (task) => {
    const info = statusInfo(task);
    const previous = taskTones.get(task.id);
    taskTones.set(task.id, info.tone);
    if (previous === info.tone || !(info.tone in TASK_TONES) || watching(task.id)) return;
    const sandbox = sandboxOf(task.sandbox_id);
    add({
      kind: TASK_TONES[info.tone],
      title: `#${task.number} ${task.title || 'Task'}`,
      detail: `${info.label}${sandbox ? ` · ${sandbox.name}` : ''}`,
      route: taskRoute(task),
      key: `task:${task.id}`,
    });
  });

  watch('browser-action', (event) => {
    const key = `browser:${event.sandbox_id}:${event.tab_id}`;
    if (event.cleared) { markRead((n) => n.key === key); return; }
    const sandbox = sandboxOf(event.sandbox_id);
    if (!sandbox) return;
    const task = event.task_id ? ctx.store.findTask(event.task_id) : null;
    add({
      kind: 'warn',
      title: `${task ? `#${task.number} ${task.title}` : 'An agent'} needs your action in the browser`,
      detail: [event.reason, sandbox.name].filter(Boolean).join(' · '),
      route: `/s/${encodeURIComponent(sandbox.slug)}/browser?tab=${encodeURIComponent(event.tab_id || '')}`,
      key,
    });
  });

  watch('message', ({ task_id: taskId, message }) => {
    if (!message || (message.from && message.from.login && message.from.login === user.login) || watching(taskId)) return;
    const task = ctx.store.findTask(taskId);
    add({
      kind: message.kind === 'question' || message.kind === 'request' ? 'warn' : 'info',
      title: `Message from ${(message.from && message.from.title) || (task ? `#${task.number} ${task.title}` : 'an agent')}`,
      detail: message.subject || null,
      route: taskRoute(task),
    });
  });

  watch('automation-run', (event) => {
    if (event.state !== 'failed' && event.state !== 'task_created') return;
    const task = event.task_id ? ctx.store.findTask(event.task_id) : null;
    add({
      kind: event.state === 'failed' ? 'error' : 'info',
      title: event.state === 'failed' ? 'An automation run failed' : `An automation started ${task ? `#${task.number} ${task.title}` : 'a task'}`,
      route: taskRoute(task),
    });
  });

  watch('account', (account) => {
    if (!account || !account.id) return;
    const previous = accountStatus.get(account.id);
    accountStatus.set(account.id, account.status);
    if (previous === account.status || !(account.status in ACCOUNT_PROBLEMS)) return;
    add({ kind: 'warn', title: `${account.label || 'An account'} ${ACCOUNT_PROBLEMS[account.status]}`, detail: account.last_error || null, route: '/admin/accounts', key: `account:${account.id}` });
  });

  render();
  return {
    button,
    add,
    /** Entries kept in this browser. */
    count: () => items.length,
    /** Forgets every entry. */
    clear() { items = []; save(); render(); },
    /** Stops watching the store (sign-out): a later shell builds a new center. */
    destroy() { close(); offs.splice(0).forEach((off) => off()); },
  };
}
