// Notification center (SPEC-001 UI-020): what happened while the user looked elsewhere, in one
// list behind the bell in the tab row — tasks waiting for input, done (TSK-014), needing attention or failing,
// browser action requests, automation runs, account problems and instance
// notices. Built from the event stream the UI already receives; toasts still show. Kept per
// user in this browser and bounded, so a reload keeps the history. With desktop notifications
// on (Settings), entries also reach the operating system while the tab is in the background.
// Every entry has a key and an expiry (SPEC-624): a newer entry with its key replaces it, and
// an expired one goes away by itself. Notices the server keeps (`/api/me/notices`) join it.

import { h, icon, iconButton, setChildren, popoverFocus, keepFocus, toast } from './dom.js';
import { relShort, dateTime } from './format.js';
import { taskInfo } from './status.js';
import { readJson, writeJson } from './store.js';
import { digestSummary } from './digest.js';

const MAX_ITEMS = 100;
const DAY_MS = 24 * 60 * 60 * 1000;
/** Default expiries of the bell's entries (SPEC-624, the spec's table), by kind. */
export const EXPIRES_MS = { info: DAY_MS, warn: DAY_MS, ok: 3 * DAY_MS, error: 7 * DAY_MS };

/** When an entry of `kind` made at `at` expires by default (ISO). */
function defaultExpires(kind, at) {
  const start = Date.parse(at);
  return new Date((Number.isFinite(start) ? start : Date.now()) + (EXPIRES_MS[kind] || DAY_MS)).toISOString();
}

/** Whether an entry has expired (`expires` null: never). */
function expired(entry, now = Date.now()) {
  return typeof entry.expires === 'string' && Date.parse(entry.expires) <= now;
}
const KINDS = new Set(['info', 'ok', 'warn', 'error']);
/** Task tones that notify when a task enters them, and the notification kind. */
const TASK_TONES = { waiting: 'info', done: 'ok', attention: 'warn', failed: 'error', 'stopped-abnormal': 'error' };
/** Settings → Preferences: also notify through the operating system while the tab is hidden. */
export const DESKTOP_KEY = 'yulius.desktopNotifications';
const FILTER_KEY = 'yulius.notifications.filter';

/**
 * The stored history, checked: an entry kept before entries had expiries gets its kind's
 * default, counted from its own time, and one already past its expiry is dropped (SPEC-624).
 */
function sanitize(list) {
  if (!Array.isArray(list)) return [];
  return list
    .filter((n) => n && typeof n.id === 'string' && typeof n.title === 'string' && typeof n.at === 'string')
    .map((n) => {
      const kind = KINDS.has(n.kind) ? n.kind : 'info';
      const expires = n.expires === null || typeof n.expires === 'string' ? n.expires : defaultExpires(kind, n.at);
      return { ...n, kind, expires, route: typeof n.route === 'string' && n.route.startsWith('/') && !n.route.startsWith('//') ? n.route : null };
    })
    .filter((n) => !expired(n))
    .slice(0, MAX_ITEMS);
}

/** The bell, its panel and the notification list. `add` records one; events are watched. */
export function createNotificationCenter(ctx) {
  const user = (ctx.store.state.me && ctx.store.state.me.user) || {};
  const storageKey = `yulius.notifications.${user.id || user.login || 'anonymous'}`;
  let items = sanitize(readJson(storageKey, []));
  writeJson(storageKey, items);
  let seq = 0;
  let expiryTimer = null;
  const taskTones = new Map(); // task id → last tone seen this session
  const taskEpisodes = new Map(); // task id → last Needs You episode seen this session

  const badge = h('span', { class: 'notif-badge', hidden: true });
  const button = h('button', { type: 'button', class: 'btn-icon notif-btn', title: 'Notifications', 'aria-label': 'Notifications', 'aria-haspopup': 'dialog', 'aria-expanded': 'false' }, icon('bell'), badge);
  let pop = null;
  let onDoc = null;
  let offFocus = null;
  let onKey = null;
  let filter = readJson(FILTER_KEY, 'all') === 'unread' ? 'unread' : 'all';

  function save() {
    writeJson(storageKey, items);
  }

  function unread() {
    return items.filter((n) => !n.read).length;
  }

  /** Drops expired entries and wakes at the next expiry (a minute at most, so a page whose
   * clock moved catches up). */
  function prune() {
    const now = Date.now();
    const kept = items.filter((n) => !expired(n, now));
    if (kept.length !== items.length) { items = kept; save(); }
    clearTimeout(expiryTimer);
    const next = Math.min(...items.filter((n) => typeof n.expires === 'string').map((n) => Date.parse(n.expires) - now), 60000);
    expiryTimer = setTimeout(() => render(), Math.max(1000, next));
  }

  function render() {
    prune();
    const count = unread();
    badge.hidden = count === 0;
    badge.textContent = count > 99 ? '99+' : String(count);
    button.title = count ? `Notifications (${count} unread)` : 'Notifications';
    button.setAttribute('aria-label', button.title);
    if (pop) keepFocus(pop, renderPanel);
  }

  /** An OS notification for an entry, only while the user is not looking at the app. */
  function desktop(entry) {
    if (readJson(DESKTOP_KEY, false) !== true || typeof Notification === 'undefined' || Notification.permission !== 'granted') return;
    if (document.visibilityState === 'visible' && document.hasFocus()) return;
    try {
      const note = new Notification(entry.title, { body: entry.detail || '', tag: entry.key || entry.id, icon: document.querySelector('link[rel="icon"]')?.href });
      note.onclick = () => {
        window.focus();
        markRead((n) => n.id === entry.id);
        if (entry.route) ctx.navigate(entry.route);
        note.close();
      };
    } catch {
      // Some mobile browsers only allow notifications from a service worker; the bell still has it.
    }
  }

  /**
   * Records a notification. A `key` keeps one entry per subject (a task, a browser tab): a new
   * one replaces it and is unread again; with `once`, an existing entry of the same `episode`
   * (any, without one) is left as it is. `expires` is when it goes away (ISO; null: never, for
   * what waits for a decision); left out, its kind's default (SPEC-624).
   */
  function add({ kind = 'info', title, detail = null, route = null, key = null, once = false, at = null, expires, episode = null, notice = null }) {
    if (key) {
      const index = items.findIndex((n) => n.key === key);
      if (index >= 0) {
        if (once && (episode === null || items[index].episode === episode)) return;
        items.splice(index, 1);
      }
    }
    seq += 1;
    const when = at || new Date().toISOString();
    const entry = { id: `${Date.now().toString(36)}-${seq}`, kind, title, detail, route, key, at: when, read: false, expires: expires === undefined ? defaultExpires(kind, when) : expires, episode, notice };
    if (expired(entry)) return;
    items.unshift(entry);
    if (items.length > MAX_ITEMS) items.length = MAX_ITEMS;
    save();
    render();
    desktop(items[0]);
  }

  function markRead(predicate) {
    let changed = false;
    for (const n of items) if (!n.read && predicate(n)) { n.read = true; changed = true; }
    if (changed) { save(); render(); }
  }

  /** An entry that waited for a decision that is now taken goes away a day later. */
  function settle(predicate) {
    const until = new Date(Date.now() + DAY_MS).toISOString();
    let changed = false;
    for (const n of items) if (predicate(n) && (n.expires === null || !n.read)) { n.read = true; n.expires = until; changed = true; }
    if (changed) { save(); render(); }
  }

  /** Clears the list; notices the server keeps are dismissed there too, so a reload does not
   * bring them back. */
  function clearAll() {
    for (const n of items) if (n.notice && ctx.api) ctx.api.del(`/api/me/notices/${encodeURIComponent(n.notice)}`).catch(() => {});
    items = [];
    save();
    render();
  }

  // ------------------------------------------------------------ panel

  function renderPanel() {
    const count = unread();
    const choice = (value, label) => h('label', null, h('input', { type: 'radio', name: 'notif-filter', value, checked: filter === value, onChange: () => { filter = value; writeJson(FILTER_KEY, value); keepFocus(pop, renderPanel); } }), label);
    const head = h('div', { class: 'notif-head' },
      h('h2', null, 'Notifications'),
      h('span', { class: 'spacer' }),
      h('button', { type: 'button', class: 'btn btn-sm', disabled: count === 0, onClick: () => markRead(() => true) }, 'Mark all read'),
      h('button', { type: 'button', class: 'btn btn-sm', disabled: items.length === 0, onClick: () => clearAll() }, 'Clear'),
      h('div', { class: 'seg notif-filter', role: 'radiogroup', 'aria-label': 'Show' }, choice('all', 'All'), choice('unread', count ? `Unread (${count})` : 'Unread')));
    const shown = filter === 'unread' ? items.filter((n) => !n.read) : items;
    // One row = the entry (a button when it leads somewhere) plus its own "Mark read" button,
    // side by side rather than nested.
    const rows = shown.map((n) => h('div', { class: ['notif-row', `notif-${n.kind}`, !n.read && 'unread'] },
      h(n.route ? 'button' : 'div', {
        type: n.route ? 'button' : null,
        class: 'notif-open',
        onClick: n.route ? () => { markRead((m) => m.id === n.id); close(); ctx.navigate(n.route); } : null,
      },
      h('span', { class: 'notif-mark', 'aria-hidden': 'true' }),
      h('span', { class: 'notif-text' },
        h('span', { class: 'notif-title' }, n.read ? null : h('span', { class: 'sr-only' }, 'Unread: '), n.title),
        n.detail ? h('span', { class: 'notif-detail' }, n.detail) : null),
      h('span', { class: 'notif-time', title: dateTime(n.at) }, relShort(n.at))),
      n.read ? h('span', { class: 'notif-read-space', 'aria-hidden': 'true' }) : iconButton('check', `Mark “${n.title}” read`, () => markRead((m) => m.id === n.id), { class: 'notif-read' })));
    const empty = filter === 'unread' && items.length
      ? 'No unread notifications.'
      : 'Nothing yet. Tasks that wait for you and browser requests show up here.';
    setChildren(pop, head, shown.length ? h('div', { class: 'notif-list' }, rows) : h('div', { class: 'notif-empty' }, empty));
  }

  function open() {
    prune();
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

  // A request for control of one of the viewer's tasks (SPEC-490 OWN-008): heard wherever they
  // are, as an entry under the bell (once per request, also across a reload, and for one that
  // waited before the page loaded) and, when it arrives while they are here, a toast that opens
  // the task, where they hand it over or decline. An answered request's entry is read.
  function heardRequest(task, live) {
    const me = ctx.store.state.me?.user?.login;
    const request = task.control_request;
    if (!me || !task.owner || task.owner.login !== me) return;
    const key = `control:${task.id}`;
    if (!request) { settle((n) => n.key === key && n.expires === null); return; }
    const episode = `${request.by.login}:${request.at}`;
    if (items.some((n) => n.key === key && n.episode === episode) || watching(task.id)) return;
    const title = `@${request.by.login} asks for control of #${task.number} ${task.title || 'Task'}`;
    const sandbox = sandboxOf(task.sandbox_id);
    const route = taskRoute(task);
    add({ kind: 'warn', title, detail: `It would act on GitHub as them${sandbox ? ` · ${sandbox.name}` : ''}`, route, key, episode, once: true, expires: null });
    if (live) toast(title, { kind: 'warn', timeout: 10000, action: route ? { label: 'Open', run: () => ctx.navigate(route) } : undefined });
  }
  watch('task', (task) => heardRequest(task, true));

  // The end-of-day digest (SPEC-512 DIG-005): one entry per digest that opens /digest, also for
  // one made while no page was open (read when the page opens). The last digest heard is
  // remembered apart from the list, so a cleared or dropped entry does not come back on the next
  // load (review round 1); the entry carries the digest's own time. Only the read at page open
  // asks that memory: a digest arriving live reaches every open tab, whichever heard it first
  // (review round 2).
  const digestSeenKey = `yulius.digest.seen.${user.id || user.login || 'anonymous'}`;
  function heardDigest(digest, live) {
    if (!digest || typeof digest.id !== 'string') return;
    if (!live && readJson(digestSeenKey, null) === digest.id) return;
    writeJson(digestSeenKey, digest.id);
    // Today's digest replaces yesterday's (SPEC-624).
    add({ kind: 'info', title: 'Your end-of-day digest', detail: digestSummary(digest), route: '/digest', key: 'digest', episode: digest.id, once: true, at: digest.created_at, expires: defaultExpires('ok', digest.created_at) });
  }
  watch('digest', (digest) => heardDigest(digest, true));
  if (ctx.api) ctx.api.get('/api/me/digest').then((digest) => heardDigest(digest, false), () => {});
  // A sandbox's tasks as first loaded: requests that waited before the page did. Later list
  // changes come with their own task event.
  const loaded = new Set();
  watch('tasks', (sandboxId) => {
    // A failed load has no list yet: the sandbox is scanned once one arrives (review round 3).
    const list = ctx.store.tasksOf(sandboxId);
    if (!list || loaded.has(sandboxId)) return;
    loaded.add(sandboxId);
    list.forEach((task) => heardRequest(task, false));
  });

  // The person who handed a task to the viewer took it back (SPEC-490 OWN-010): the server tells
  // the person it was taken from, wherever they are.
  watch('control-taken-back', (event) => {
    const task = ctx.store.findTask(event.task_id) || { id: event.task_id, sandbox_id: event.sandbox_id, number: event.number };
    const title = `@${event.by} took back control of #${event.number} ${event.title || 'Task'}`;
    const route = taskRoute(task);
    add({ kind: 'info', title, detail: 'It acts on GitHub as them again', route, key: `control:${event.task_id}`, expires: defaultExpires('ok', new Date().toISOString()) });
    toast(title, { kind: 'info', action: route ? { label: 'Open', run: () => ctx.navigate(route) } : undefined });
  });

  // Someone took control of the viewer's task without asking (SPEC-510 TKC-002): told wherever
  // they are, as above.
  watch('control-taken', (event) => {
    const task = ctx.store.findTask(event.task_id) || { id: event.task_id, sandbox_id: event.sandbox_id, number: event.number };
    const title = `@${event.by} took control of #${event.number} ${event.title || 'Task'}`;
    const route = taskRoute(task);
    add({ kind: 'info', title, detail: 'It acts on GitHub as them now; you can take it back', route, key: `control:${event.task_id}`, expires: defaultExpires('ok', new Date().toISOString()) });
    toast(title, { kind: 'info', action: route ? { label: 'Open', run: () => ctx.navigate(route) } : undefined });
  });

  // The sandbox-level problems a Needs you may name (SPEC-494): their tasks share one entry.
  const SHARED_CAUSES = { docker: 'its Docker needs you' };

  watch('task', (task) => {
    // The rule every view uses (status.js taskInfo): a tested pull request is Done, failing checks
    // need the user.
    const info = taskInfo(task, ctx.store.state.actionRequests, ctx.store.pullsOf(task.sandbox_id).get(task.id));
    // A switch is under the hood: the session goes on with another subscription, or restarts on
    // a newer Claude Code, and the task reads "Continuing" meanwhile. What it read before is
    // remembered through it, so a task that reads afterwards as it did (an idle session that
    // got nothing typed) is no news; one that reads differently is (SPEC-468 CCF-008).
    if (info.tone === 'switching') return;
    const previous = taskTones.get(task.id);
    taskTones.set(task.id, info.tone);
    const notice = info.group === 'needs' ? task.needs_you_notice : null;
    const episode = notice ? (notice.episode || (task.needs_you && task.needs_you.at) || '') : null;
    const prevEpisode = taskEpisodes.get(task.id);
    if (notice) taskEpisodes.set(task.id, episode);
    else taskEpisodes.delete(task.id);
    const sameEpisode = Boolean(notice && episode && prevEpisode === episode);
    const sameState = notice ? (previous === info.tone && (sameEpisode || !episode)) : previous === info.tone;
    // An entry that waited for the person goes a day after the task no longer does, and a
    // shared problem's once no task of its project asks about it (SPEC-624).
    if (!notice) settle((n) => n.key === `task:${task.id}` && n.expires === null);
    for (const shared of Object.keys(SHARED_CAUSES)) {
      const asked = (ctx.store.tasksOf(task.sandbox_id) || []).some((t) => t.needs_you && t.needs_you.cause === shared);
      if (!asked) settle((n) => n.key === `needs-you:${task.sandbox_id}:${shared}` && n.expires === null);
    }
    if (sameState || !(info.tone in TASK_TONES) || watching(task.id)) return;
    const sandbox = sandboxOf(task.sandbox_id);
    // A problem of the whole sandbox several tasks ask about (SPEC-494): one entry per episode
    // (the server names it by its oldest open request), not one per task, wherever the person is.
    const cause = task.needs_you && Object.hasOwn(SHARED_CAUSES, task.needs_you.cause || '') ? task.needs_you.cause : null;
    if (cause && info.tone === 'attention' && task.needs_you.episode) {
      add({
        kind: TASK_TONES[info.tone],
        title: `${sandbox ? sandbox.name : 'A project'}: ${SHARED_CAUSES[cause]}`,
        detail: `Needs you, asked by #${task.number} and any other task of it that shares the problem`,
        route: taskRoute(task),
        key: `needs-you:${task.sandbox_id}:${cause}`,
        episode: String(task.needs_you.episode),
        once: true,
        expires: null,
      });
      return;
    }
    let route = taskRoute(task);
    if (notice && typeof notice.link === 'string' && notice.link.startsWith(location.origin)) {
      try {
        const parsed = new URL(notice.link);
        route = `${parsed.pathname}${parsed.search}`;
      } catch { /* keep taskRoute */ }
    }
    const detail = notice
      ? [notice.happened, notice.action, sandbox ? sandbox.name : null].filter(Boolean).join(' · ').replaceAll('`', '')
      : `${info.label}${sandbox ? ` · ${sandbox.name}` : ''}`;
    add({
      kind: TASK_TONES[info.tone],
      title: `#${task.number} ${task.title || 'Task'}`,
      detail,
      route,
      // One entry per task: its newer state, or a newer red run of its pull request, replaces
      // the older one; while it waits for the person it never expires (SPEC-624).
      key: `task:${task.id}`,
      episode,
      at: (notice && notice.at) || null,
      expires: info.group === 'needs' ? null : undefined,
    });
  });

  watch('browser-action', (event) => {
    const key = `browser:${event.sandbox_id}:${event.tab_id}`;
    if (event.cleared) { settle((n) => n.key === key); return; }
    const sandbox = sandboxOf(event.sandbox_id);
    if (!sandbox) return;
    const task = event.task_id ? ctx.store.findTask(event.task_id) : null;
    add({
      kind: 'warn',
      title: `${task ? `#${task.number} ${task.title}` : 'An agent'} needs your action in the browser`,
      detail: [event.reason, sandbox.name].filter(Boolean).join(' · '),
      route: `/s/${encodeURIComponent(sandbox.slug)}/browser?tab=${encodeURIComponent(event.tab_id || '')}`,
      key,
      expires: null,
    });
  });

  watch('automation-run', (event) => {
    if (event.state !== 'failed' && event.state !== 'task_created') return;
    const task = event.task_id ? ctx.store.findTask(event.task_id) : null;
    add({
      kind: event.state === 'failed' ? 'error' : 'info',
      title: event.state === 'failed' ? 'An automation run failed' : `An automation started ${task ? `#${task.number} ${task.title}` : 'a task'}`,
      route: taskRoute(task),
      key: `automation:${event.automation_id || event.task_id || ''}`,
      expires: defaultExpires('ok', new Date().toISOString()),
    });
  });

  // Notices the server keeps (SPEC-624): subscription problems, integration failures nobody
  // repairs, script watches. One entry per project and key; the server's own expiry; removed
  // when the server removes it. Each is told once, also across a reload.
  const noticeKey = (n) => `notice:${n.sandbox_id || '-'}:${n.personal ? 'me' : '-'}:${n.key}`;
  function heardNotice(notice) {
    if (!notice || typeof notice.id !== 'string' || typeof notice.title !== 'string') return;
    const key = noticeKey(notice);
    if (items.some((n) => n.key === key && n.notice === notice.id)) return;
    const sandbox = notice.sandbox_id ? sandboxOf(notice.sandbox_id) : null;
    add({
      kind: KINDS.has(notice.tone) ? notice.tone : 'info',
      title: notice.title,
      detail: [notice.detail, sandbox ? sandbox.name : null].filter(Boolean).join(' · ') || null,
      route: typeof notice.link === 'string' && notice.link.startsWith('/') && !notice.link.startsWith('//') ? notice.link : null,
      key,
      at: notice.created_at,
      expires: notice.expires_at || null,
      notice: notice.id,
    });
  }
  watch('notice', heardNotice);
  watch('notice-removed', (event) => {
    const kept = items.filter((n) => n.notice !== event.id);
    if (kept.length !== items.length) { items = kept; save(); render(); }
  });
  if (ctx.api) ctx.api.get('/api/me/notices').then((list) => { if (Array.isArray(list)) list.slice().reverse().forEach(heardNotice); }, () => {});

  render();
  return {
    button,
    add,
    /** Entries kept in this browser. */
    count: () => items.length,
    /** Forgets every entry. */
    clear() { items = []; save(); render(); },
    /** Stops watching the store (sign-out): a later shell builds a new center. */
    destroy() { close(); clearTimeout(expiryTimer); offs.splice(0).forEach((off) => off()); },
  };
}
