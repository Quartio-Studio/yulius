// Tasks overview (SPEC-001 §6.2, UI-024) and Archived tasks (UI-006, AC-ARC-1).
//
// The overview is a grouped list, not a table: Needs you → Working → Stopped → Closed, with the
// sidebar's exact rules (status.js taskInfo). Each row reads in words, with its pull request and
// checks, author and age on the right. Filters (state tabs with counts, Author and Origin
// multi-select, search) live in the URL so a filtered view can be shared as a link.

import { h, icon, setChildren, iconButton, badge, toast, errorToast, statusMark, openMenu, loadError, singleFlight, keepFocus } from '../dom.js';
import { relShort, dateTime, plural } from '../format.js';
import { statusInfo, taskInfo, activityLine, allows, taskActions } from '../status.js';
import { openTaskMenu, restoreTask, archiveTask } from '../actions.js';
import { href, currentLocation, replaceQuietly } from '../router.js';
import { readJson, writeJson } from '../store.js';
import { enc } from '../api.js';
import { initials } from '../layout.js';
import { pageFrame, dataTable, loader } from './common.js';

function statusCell(task) {
  const info = statusInfo(task);
  return h('span', { class: 'row nowrap' }, statusMark(info), info.label);
}

function taskLink(sandbox, task) {
  const path = `/s/${encodeURIComponent(sandbox.slug)}/t/${task.number}`;
  return h('a', { href: href(path), dataset: { route: path } }, task.title || `Task #${task.number}`);
}

const GROUPS = [
  { id: 'needs', name: 'Needs you', icon: 'bell' },
  { id: 'working', name: 'Working', icon: 'pulse' },
  { id: 'stopped', name: 'Stopped', icon: 'st-stopped' },
  { id: 'closed', name: 'Closed', icon: 'st-closed' },
];
const ORIGINS = { ui: 'Started here', automation: 'Automation', import: 'Imported', onboarding: 'Onboarding', agent: 'Another task' };
const CHECKS = {
  success: { icon: 'check', tone: 'ok', label: 'checks passed' },
  failure: { icon: 'x', tone: 'danger', label: 'checks failed' },
  pending: { icon: 'clock', tone: 'warn', label: 'checks running' },
};
const CLOSED_PREVIEW = 5;
const COLLAPSED_KEY = 'yulius.tasksCollapsed';

/** Which list group a task is in: the sidebar's groups, with Closed apart from Stopped. */
function groupOf(task, info) {
  if (info.group !== 'ended') return info.group;
  return task.status === 'closed' ? 'closed' : 'stopped';
}

/** The second line: what is happening or what to do, in words (status.js activityLine). */
const detailLine = (task, info) => activityLine(task, info);

/** Pull request chip: number and checks, opening GitHub. Empty (not "—") when there is none. */
function prChip(pr) {
  if (!pr) return h('span', { class: 'tl-pr' });
  const merged = pr.state === 'merged';
  const checks = merged || pr.state === 'closed' ? null : CHECKS[pr.checks];
  const label = `Pull request #${pr.number} · ${pr.state}${checks ? ` · ${checks.label}` : ''}`;
  return h('span', { class: 'tl-pr' }, h('a', { class: ['tl-pr-chip', merged && 'merged', pr.state === 'closed' && 'closed'], href: pr.url, target: '_blank', rel: 'noopener noreferrer', title: label, 'aria-label': label },
    icon('branch', { size: 12 }), `#${pr.number}`, checks ? h('span', { class: `tone-${checks.tone}` }, icon(checks.icon, { size: 12 })) : null));
}

/** Tasks of one sandbox: grouped list with filters. */
export function mountTasksPage(ctx, sandbox) {
  const base = `/s/${encodeURIComponent(sandbox.slug)}/tasks`;
  const query = currentLocation().query;
  const filters = {
    state: GROUPS.some((g) => g.id === query.get('state')) ? query.get('state') : 'all',
    authors: new Set((query.get('author') || '').split(',').filter(Boolean)),
    origins: new Set((query.get('origin') || '').split(',').filter(Boolean)),
  };
  let alive = true;
  const stored = readJson(COLLAPSED_KEY, []);
  const collapsed = new Set(Array.isArray(stored) ? stored : []);
  let closedExpanded = false;
  let pulls = new Map();

  const frame = pageFrame({ title: 'Tasks', sub: sandbox.name, actions: [
    h('button', { type: 'button', class: 'btn btn-primary', onClick: () => ctx.openNewTask && ctx.openNewTask() }, icon('plus', { size: 14 }), 'New task', h('span', { class: 'kbd kbd-on-accent' }, '⌘⇧O')),
  ] });
  const tabs = h('div', { class: 'tl-tabs', role: 'radiogroup', 'aria-label': 'Task state' });
  const chips = h('div', { class: 'tl-chips' });
  const summary = h('span', { class: 'tl-summary', 'aria-live': 'polite' });
  const search = h('input', { class: 'input tl-search', type: 'search', placeholder: 'Search tasks', value: query.get('q') || '', 'aria-label': 'Search tasks (press / to focus)', 'aria-keyshortcuts': '/' });
  const searchBox = h('label', { class: 'tl-search-box' }, icon('search', { size: 14 }), search, h('span', { class: 'kbd tl-search-key', 'aria-hidden': 'true' }, '/'));
  const listHost = h('div', { class: 'tl' });
  const footer = h('div', { class: 'tl-footer' }, h('a', { href: href('/archived'), dataset: { route: '/archived' } }, 'View archived tasks', icon('right', { size: 12 })));
  // Search sits in the header next to New task, so the filter row stays on one line.
  frame.actions.prepend(searchBox);
  frame.body.append(h('div', { class: 'tl-toolbar' }, tabs, chips, summary), listHost, footer);

  function syncUrl() {
    const params = new URLSearchParams();
    if (filters.state !== 'all') params.set('state', filters.state);
    if (filters.authors.size) params.set('author', [...filters.authors].join(','));
    if (filters.origins.size) params.set('origin', [...filters.origins].join(','));
    if (search.value.trim()) params.set('q', search.value.trim());
    const qs = params.toString();
    if (alive) replaceQuietly(qs ? `${base}?${qs}` : base);
  }

  const myLogin = () => (ctx.store.state.me && ctx.store.state.me.user ? ctx.store.state.me.user.login : null);
  const filtered = () => filters.state !== 'all' || filters.authors.size || filters.origins.size || search.value.trim();
  function clearFilters() {
    filters.state = 'all';
    filters.authors.clear();
    filters.origins.clear();
    search.value = '';
    render();
  }

  // A filter chip is built once and updated in place, so its open menu stays anchored while choices toggle.
  function filterChip(name, set, labelOf) {
    let values = [];
    const label = h('span', { class: 'chip-label' });
    const button = h('button', { type: 'button', class: ['chip-btn', 'tl-chip'], 'aria-haspopup': 'menu', onClick: () => {
      openMenu(button, [
        ...values.map(([id, count]) => ({ label: labelOf(id), hint: String(count), checked: set.has(id), keepOpen: true, onSelect: () => { if (set.has(id)) set.delete(id); else set.add(id); render(); } })),
        { separator: true },
        { label: `Clear ${name.toLowerCase()} filter`, onSelect: () => { set.clear(); render(); } },
      ]);
    } }, label, icon('down', { size: 12 }));
    const clear = iconButton('x', `Clear ${name.toLowerCase()} filter`, () => { set.clear(); render(); button.focus(); }, { class: 'tl-chip-clear' });
    const el = h('span', { class: 'tl-chip-group', hidden: true }, button, clear); // shown once tasks are known
    return {
      el,
      update(next) {
        values = next;
        el.hidden = values.length < 2 && !set.size;
        button.classList.toggle('active', set.size > 0);
        label.textContent = set.size ? `${name}: ${set.size === 1 ? labelOf([...set][0]) : set.size}` : name;
        clear.hidden = !set.size;
      },
    };
  }
  const authorChip = filterChip('Author', filters.authors, (login) => (login === myLogin() ? `Me (${login})` : login));
  const originChip = filterChip('Origin', filters.origins, (id) => ORIGINS[id] || id);
  chips.append(authorChip.el, originChip.el);

  function render() {
    if (!alive) return;
    syncUrl();
    const tasks = ctx.store.tasksOf(sandbox.id);
    if (tasks === null) {
      const error = ctx.store.tasksError(sandbox.id);
      setChildren(listHost, error
        ? loadError(`Couldn’t load the tasks of ${sandbox.name}: ${error.message}`, () => ctx.store.loadTasks(sandbox.id).catch(() => {}))
        : h('div', { class: 'placeholder placeholder-loading' }, 'Loading…'));
      return;
    }
    const me = myLogin();
    const q = search.value.trim().toLowerCase();
    const items = tasks.map((task) => { const info = taskInfo(task, ctx.store.state.actionRequests); return { task, info, group: groupOf(task, info) }; });

    // Author and origin choices come from all tasks, so a choice never disappears while selected.
    const count = (key) => { const m = new Map(); for (const { task } of items) { const k = key(task); if (k) m.set(k, (m.get(k) || 0) + 1); } return m; };
    const authorCounts = count((t) => t.created_by && t.created_by.login);
    const originCounts = count((t) => t.origin);
    const authors = [...authorCounts].sort((a, b) => (b[0] === me) - (a[0] === me) || b[1] - a[1] || a[0].localeCompare(b[0]));
    const origins = [...originCounts].sort((a, b) => b[1] - a[1]);

    const matches = items.filter(({ task }) => (!filters.authors.size || filters.authors.has(task.created_by && task.created_by.login))
      && (!filters.origins.size || filters.origins.has(task.origin))
      && (!q || `#${task.number} ${task.title} ${task.branch || ''} ${(task.created_by && task.created_by.login) || ''}`.toLowerCase().includes(q)));
    const byGroup = new Map(GROUPS.map((g) => [g.id, matches.filter((item) => item.group === g.id)]));
    const visible = filters.state === 'all' ? matches : byGroup.get(filters.state);

    // Live updates rebuild the tabs and groups; keyboard focus stays where it was (keepFocus).
    keepFocus(tabs, () => setChildren(tabs, [{ id: 'all', name: 'All', icon: 'tasks', size: matches.length }, ...GROUPS.map((g) => ({ ...g, size: byGroup.get(g.id).length }))].map((tab) => {
      const on = filters.state === tab.id;
      return h('button', { type: 'button', role: 'radio', 'aria-checked': String(on), tabindex: on ? '0' : '-1', class: ['tl-tab', on && 'active', tab.id === 'needs' && tab.size > 0 && 'urgent'], onClick: () => { filters.state = tab.id; render(); },
        onKeydown: (event) => {
          // A radio group moves with all four arrows (APG radio group).
          const step = { ArrowRight: 1, ArrowDown: 1, ArrowLeft: -1, ArrowUp: -1 }[event.key];
          if (!step) return;
          event.preventDefault();
          const all = [...tabs.children];
          const next = all[(all.indexOf(event.currentTarget) + step + all.length) % all.length];
          next.click();
          tabs.querySelector('[aria-checked="true"]').focus();
        } }, h('span', { class: `tl-tab-icon tab-${tab.id}` }, icon(tab.icon, { size: 14 })), tab.name, h('span', { class: 'tl-tab-count' }, String(tab.size)));
    })));
    authorChip.update(authors);
    originChip.update(origins);
    setChildren(summary, filtered()
      ? [`Showing ${visible.length} of ${tasks.length}`, ' · ', h('button', { type: 'button', class: 'link-btn', onClick: clearFilters }, 'Clear filters')]
      : plural(tasks.length, 'task'));
    frame.sub.textContent = sandbox.name;

    const focusedId = listHost.contains(document.activeElement) ? document.activeElement.closest('[data-task-id]')?.dataset.taskId : null;
    if (!tasks.length) {
      setChildren(listHost, h('div', { class: 'tl-empty' },
        h('p', { class: 'tl-empty-title' }, `No tasks in ${sandbox.name} yet`),
        h('p', { class: 'muted' }, 'Each task is a Claude Code session on its own branch.'),
        h('button', { type: 'button', class: 'btn btn-primary', onClick: () => ctx.openNewTask && ctx.openNewTask() }, icon('plus', { size: 14 }), 'New task')));
      return;
    }
    if (!visible.length) {
      setChildren(listHost, h('div', { class: 'tl-empty' },
        h('p', { class: 'tl-empty-title' }, q ? `No tasks match “${search.value.trim()}”` : 'No tasks match these filters'),
        h('p', { class: 'muted' }, 'The counts on the tabs show where the matches are.'),
        h('button', { type: 'button', class: 'btn', onClick: clearFilters }, 'Clear filters')));
      return;
    }

    const sections = [];
    for (const group of GROUPS) {
      const list = filters.state === 'all' ? byGroup.get(group.id) : (filters.state === group.id ? visible : []);
      if (!list.length) continue;
      const open = filters.state === group.id || !collapsed.has(group.id);
      const shown = group.id === 'closed' && !closedExpanded && filters.state === 'all' && !q ? list.slice(0, CLOSED_PREVIEW) : list;
      const headId = `tl-head-${group.id}`;
      sections.push(h('section', { class: ['tl-group', `group-${group.id}`], 'aria-labelledby': headId },
        h('h2', { class: 'tl-group-title' }, h('button', { type: 'button', id: headId, class: 'tl-group-head', 'aria-expanded': String(open), onClick: () => {
          if (open) collapsed.add(group.id); else collapsed.delete(group.id);
          writeJson(COLLAPSED_KEY, [...collapsed]);
          render();
        } }, icon(open ? 'down' : 'right', { size: 12 }), icon(group.icon, { size: 14 }), h('span', { class: 'tl-group-name' }, group.name), h('span', { class: 'tl-group-count' }, String(list.length)))),
        open ? h('ul', { class: 'tl-rows' }, shown.map((item) => rowView(item))) : null,
        open && shown.length < list.length ? h('button', { type: 'button', class: 'tl-more', onClick: () => { closedExpanded = true; render(); } }, `Show ${list.length - shown.length} more`) : null));
    }
    if (focusedId) setChildren(listHost, sections);
    else keepFocus(listHost, () => setChildren(listHost, sections));
    if (focusedId) listHost.querySelector(`[data-task-id="${CSS.escape(focusedId)}"] .tl-link`)?.focus({ preventScroll: true });
  }

  function rowView({ task, info, group }) {
    const path = `/s/${encodeURIComponent(sandbox.slug)}/t/${task.number}`;
    const login = task.created_by && task.created_by.login;
    const actions = taskActions(task, sandbox.access);
    const kebab = iconButton('more', `Actions for ${task.title || `task #${task.number}`}`, () => openTaskMenu(ctx, kebab, task), { class: 'tl-action', 'aria-haspopup': 'menu' });
    const row = h('li', { class: ['tl-row', `group-${group}`], dataset: { taskId: task.id, tone: info.tone } },
      h('a', { class: 'tl-link', href: href(path), dataset: { route: path }, title: `#${task.number} ${task.title}${task.branch ? ` · ${task.branch}` : ''}` },
        h('span', { class: 'tl-mark' }, statusMark(info, 18)),
        h('span', { class: 'tl-text' },
          h('span', { class: 'tl-line' },
            h('span', { class: 'tl-title' }, task.title || `Task #${task.number}`),
            task.unread_messages ? h('span', { class: 'task-unread', title: `${task.unread_messages} unread messages` }, String(task.unread_messages), h('span', { class: 'sr-only' }, ' unread')) : null,
            task.origin && task.origin !== 'ui' ? h('span', { class: 'tl-origin' }, ORIGINS[task.origin] || task.origin) : null),
          // One sentence only: the number and branch stay in the tooltip and in search.
          h('span', { class: 'tl-sub' },
            h('span', { class: 'tl-what' }, detailLine(task, info)),
            h('span', { class: 'tl-sub-time' }, ` · ${relShort(task.updated_at)}`)))),
      prChip(pulls.get(task.id)),
      // Only someone else's task shows its author; your own would repeat your initials on every row.
      login && login !== myLogin() ? h('span', { class: 'avatar tl-avatar', role: 'img', title: `Started by ${login}`, 'aria-label': `Started by ${login}` }, initials(login)) : h('span', { class: 'tl-avatar' }),
      h('span', { class: 'tl-time', title: `Updated ${dateTime(task.updated_at)} · created ${dateTime(task.created_at)}` }, relShort(task.updated_at)),
      h('span', { class: 'tl-actions' },
        actions.archive ? iconButton('archive', 'Archive', () => archiveTask(ctx, task), { class: 'tl-action' }) : null,
        kebab));
    row.addEventListener('contextmenu', (event) => {
      if (event.target.closest('a.tl-pr-chip')) return;
      event.preventDefault();
      openTaskMenu(ctx, { x: event.clientX, y: event.clientY }, task);
    });
    return row;
  }

  // Pull requests and checks: one read per sandbox, refreshed every minute and on focus.
  async function loadPulls() {
    try {
      const git = await ctx.api.get(`/api/sandboxes/${enc(sandbox.id)}/git`);
      if (!alive) return; // the page was left while the read was in flight
      pulls = new Map((git.tasks || []).filter((t) => t.pr).map((t) => [t.task_id, t.pr]));
      render();
    } catch { /* PR chips stay empty; the list works without them */ }
  }

  // Keyboard: "/" focuses search, j/k move between tasks, "." opens the focused task's menu.
  const typing = (target) => target.closest('input, textarea, select, [contenteditable="true"], .xterm');
  function onKey(event) {
    if (event.metaKey || event.ctrlKey || event.altKey || event.defaultPrevented || !frame.el.isConnected) return;
    if (readJson('yulius.singleKeyShortcuts', true) === false) return; // turned off in Settings (WCAG 2.1.4)
    // Single-key shortcuts only while focus is on this page, not in a tool panel (WCAG 2.1.4).
    if (event.target !== document.body && !frame.el.contains(event.target)) return;
    if (event.key === '/' && !typing(event.target)) { event.preventDefault(); search.focus(); search.select(); return; }
    if (typing(event.target)) return;
    const links = [...listHost.querySelectorAll('.tl-link')];
    const index = links.indexOf(document.activeElement);
    const move = { j: 1, ArrowDown: 1, k: -1, ArrowUp: -1 }[event.key];
    if (move && (index >= 0 || event.key === 'j' || event.key === 'k')) {
      event.preventDefault();
      const next = links[index < 0 ? 0 : Math.max(0, Math.min(links.length - 1, index + move))];
      if (next) { next.focus(); next.scrollIntoView({ block: 'nearest' }); }
    } else if (event.key === '.' && index >= 0) {
      event.preventDefault();
      links[index].closest('.tl-row').querySelector('.tl-actions .tl-action:last-child')?.click();
    }
  }
  search.addEventListener('input', render);
  search.addEventListener('keydown', (event) => {
    if (event.key !== 'Escape') return;
    event.preventDefault();
    if (search.value) { search.value = ''; render(); } else search.blur();
  });
  document.addEventListener('keydown', onKey);
  const onFocus = () => loadPulls();
  window.addEventListener('focus', onFocus);
  const timer = setInterval(loadPulls, 60_000);
  const offs = [
    ctx.store.on('tasks', (sid) => { if (sid === sandbox.id) render(); }),
    ctx.store.on('browser-action', (event) => { if (event.sandbox_id === sandbox.id) render(); }),
  ];
  render();
  loadPulls();
  // A link to this page with other filters (the nav link, a shared URL) updates it in place.
  function update(route) {
    const next = route.query;
    filters.state = GROUPS.some((g) => g.id === next.get('state')) ? next.get('state') : 'all';
    filters.authors.clear(); (next.get('author') || '').split(',').filter(Boolean).forEach((a) => filters.authors.add(a));
    filters.origins.clear(); (next.get('origin') || '').split(',').filter(Boolean).forEach((o) => filters.origins.add(o));
    search.value = next.get('q') || '';
    render();
  }
  return { el: frame.el, key: `tasks:${sandbox.id}`, update, destroy: () => {
    alive = false;
    offs.forEach((off) => off());
    document.removeEventListener('keydown', onKey);
    window.removeEventListener('focus', onFocus);
    clearInterval(timer);
  } };
}

/** Archived tasks across visible sandboxes, with restore. */
export function mountArchivedPage(ctx) {
  const frame = pageFrame({ title: 'Archived tasks', sub: 'across your sandboxes' });
  const host = h('div');
  frame.body.append(host);
  const reload = loader(host, () => ctx.api.get('/api/archived-tasks'), (tasks) => {
    const list = Array.isArray(tasks) ? tasks : [];
    frame.sub.textContent = `${list.length} archived across your sandboxes`;
    return dataTable([
      { label: 'Sandbox', render: (t) => { const sb = ctx.store.sandboxById(t.sandbox_id); return sb ? sb.name : badge('unknown'); } },
      { label: '#', class: 'num', render: (t) => String(t.number) },
      { label: 'Title', class: 'title-cell', render: (t) => { const sb = ctx.store.sandboxById(t.sandbox_id); return sb ? taskLink(sb, t) : t.title; } },
      { label: 'Status', render: statusCell, phoneHide: true },
      { label: 'Archived', class: 'num', render: (t) => h('span', { title: dateTime(t.updated_at) }, relShort(t.updated_at)) },
      { label: '', render: (t) => {
        const sb = ctx.store.sandboxById(t.sandbox_id);
        const can = sb && allows(sb.access, 'write');
        return h('button', { type: 'button', class: 'btn btn-sm', disabled: !can, title: can ? 'Restore to the sidebar' : 'Requires write access', onClick: async () => {
          const updated = await restoreTask(ctx, t);
          if (updated) reload();
        } }, 'Restore');
      } },
    ], list, { empty: 'No archived tasks. Archive finished tasks from their action menu.' });
  });
  return { el: frame.el, key: 'archived' };
}

/** CLI device approval (docs/api.md device flow): approve or deny a user code. */
export function mountDevicePage(ctx, route) {
  const frame = pageFrame({ title: 'CLI device approval', sub: 'approve a command-line sign-in, such as “yulius import”' });
  const code = h('input', { placeholder: 'ABCD-EFGH', autocomplete: 'off', spellcheck: 'false', value: route.query.get('code') || route.query.get('user_code') || '', class: 'input mono code-input', 'aria-label': 'User code' });
  const result = h('div', { role: 'status' });
  const decide = (approve) => singleFlight(async () => {
    const userCode = code.value.trim().toUpperCase();
    if (!userCode) { code.focus(); return; }
    try {
      await ctx.api.post('/api/device/approve', { user_code: userCode, approve });
      setChildren(result, h('div', { class: `banner ${approve ? 'banner-info' : ''}` }, approve ? `Approved ${userCode}. The CLI receives a token on its next poll.` : `Denied ${userCode}.`));
      toast(approve ? 'Device approved' : 'Device denied', { kind: approve ? 'ok' : 'info' });
    } catch (error) {
      errorToast(error, 'Approval failed');
    }
  });
  frame.body.append(h('div', { class: 'card card-pad stack narrow' },
    h('p', { class: 'text-2' }, 'Enter the code shown by the CLI. Approve only codes you started yourself; the CLI gets a token with your permissions.'),
    h('div', { class: 'row-wrap' }, code, h('button', { type: 'button', class: 'btn btn-primary', onClick: decide(true) }, 'Approve'), h('button', { type: 'button', class: 'btn', onClick: decide(false) }, 'Deny')),
    result));
  return { el: frame.el, key: 'device' };
}

