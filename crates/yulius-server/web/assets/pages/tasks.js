// Tasks overview (SPEC-001 §6.2, UI-024) and Archived tasks (UI-006, AC-ARC-1).
//
// The overview is a grouped list, not a table: Needs you → Working → Stopped → Closed, with the
// sidebar's exact rules (status.js taskInfo). Each row reads in words, with its pull request and
// checks, author and age on the right. Filters (state tabs with counts, Author and Origin
// multi-select, search) live in the URL so a filtered view can be shared as a link. Search is not a
// field: "/" opens a Spotlight-style search (spotlight.js, UI-036) that jumps to a task or filters.

import { h, icon, setChildren, iconButton, badge, statusMark, openMenu, loadError, singleFlight, keepFocus, coalesce, prChip, emptyState } from '../dom.js';
import { relShort, dateTime, plural } from '../format.js';
import { statusInfo, taskInfo, activityLine, allows, taskActions } from '../status.js';
import { openTaskMenu, restoreTask, archiveTask } from '../actions.js';
import { href, navigate, currentLocation, replaceQuietly } from '../router.js';
import { openSpotlight } from '../spotlight.js';
import { readJson, writeJson } from '../store.js';
import { initials } from '../layout.js';
import { pageFrame, dataTable, loader } from './common.js';
import { NEW_TASK_KEYS } from '../shortcuts.js';

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
  { id: 'done', name: 'Done', icon: 'check' },
  { id: 'stopped', name: 'Stopped', icon: 'st-stopped' },
  { id: 'closed', name: 'Closed', icon: 'st-closed' },
];
const ORIGINS = { ui: 'Started here', automation: 'Automation', import: 'Imported', onboarding: 'Onboarding', agent: 'Another task' };
const CLOSED_PREVIEW = 5;
// Long groups show this many rows, then "Show more" adds a page (PERF-03: hundreds of rows made
// every repaint and keystroke slow).
const GROUP_PAGE = 50;
const COLLAPSED_KEY = 'yulius.tasksCollapsed';

/** Which list group a task is in: the sidebar's groups, with Closed apart from Stopped. */
function groupOf(task, info) {
  if (info.group !== 'ended') return info.group;
  return task.status === 'closed' ? 'closed' : 'stopped';
}

/** The second line: what is happening or what to do, in words (status.js activityLine). */
const detailLine = (task, info) => activityLine(task, info);

/** The pull request cell: its chip (opening GitHub), empty (not "—") when there is none. */
const prCell = (pr) => h('span', { class: 'tl-pr' }, pr ? prChip(pr) : null);

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
  const shownCount = new Map(); // group id → rows shown after "Show more"
  let searchQuery = query.get('q') || ''; // the list's text filter, set from the search's last row

  // Read access watches; starting tasks needs write access, as on Home and every other page.
  const canStart = allows(sandbox.access, 'write');
  const frame = pageFrame({ title: 'Tasks', sub: 'each task is a Claude Code session on its own branch', actions: [
    h('button', { type: 'button', class: 'tl-find', 'aria-keyshortcuts': '/', 'aria-label': 'Search tasks', onClick: () => openSearch() }, icon('search', { size: 14 }), h('span', { class: 'tl-find-text', 'aria-hidden': 'true' }, 'Press', h('span', { class: 'kbd' }, '/'), 'to search')),
    h('button', { type: 'button', class: 'btn btn-primary', disabled: !canStart, title: canStart ? null : 'Requires write access', onClick: () => ctx.openNewTask && ctx.openNewTask() }, icon('plus', { size: 14 }), 'New task', h('span', { class: 'kbd kbd-on-accent' }, NEW_TASK_KEYS)),
  ] });
  const tabs = h('div', { class: 'tl-tabs', role: 'radiogroup', 'aria-label': 'Task state' });
  const chips = h('div', { class: 'tl-chips' });
  const summary = h('span', { class: 'tl-summary', 'aria-live': 'polite' });
  const listHost = h('div', { class: 'tl' });
  const footer = h('div', { class: 'tl-footer' }, h('a', { href: href('/archived'), dataset: { route: '/archived' } }, 'View archived tasks', icon('right', { size: 12 })));
  frame.body.append(h('div', { class: 'tl-toolbar' }, tabs, chips, summary), listHost, footer);

  function syncUrl() {
    const params = new URLSearchParams();
    if (filters.state !== 'all') params.set('state', filters.state);
    if (filters.authors.size) params.set('author', [...filters.authors].join(','));
    if (filters.origins.size) params.set('origin', [...filters.origins].join(','));
    if (searchQuery) params.set('q', searchQuery);
    const qs = params.toString();
    if (alive) replaceQuietly(qs ? `${base}?${qs}` : base);
  }

  const myLogin = () => (ctx.store.state.me && ctx.store.state.me.user ? ctx.store.state.me.user.login : null);
  const filtered = () => filters.state !== 'all' || filters.authors.size || filters.origins.size || searchQuery;
  function clearFilters() {
    filters.state = 'all';
    filters.authors.clear();
    filters.origins.clear();
    searchQuery = '';
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
  // The text filter, once the search's "Show all matching" row set it: reopens the search, or clears.
  const searchChip = h('span', { class: 'tl-chip-group', hidden: true },
    h('button', { type: 'button', class: ['chip-btn', 'tl-chip', 'active'], onClick: () => openSearch(searchQuery) }, icon('search', { size: 12 }), h('span', { class: 'chip-label' })),
    iconButton('x', 'Clear search', () => { searchQuery = ''; render(); }, { class: 'tl-chip-clear' }));
  chips.append(searchChip, authorChip.el, originChip.el);

  const matchesText = (task, q) => `#${task.number} ${task.title} ${task.branch || ''} ${(task.created_by && task.created_by.login) || ''}`.toLowerCase().includes(q);
  /** "/" — search every task of the sandbox; Enter opens its overview, ⌘↵ its session. */
  function openSearch(initial = '') {
    openSpotlight({
      label: `Search tasks in ${sandbox.name}`,
      placeholder: `Search tasks in ${sandbox.name}`,
      initial,
      search: (text) => {
        const q = text.toLowerCase();
        const tasks = ctx.store.tasksOf(sandbox.id) || [];
        const number = /^#?\d+$/.test(q) ? Number(q.replace('#', '')) : null;
        const score = (t) => (t.number === number ? 0 : (t.title || '').toLowerCase().startsWith(q) ? 1 : 2);
        const found = q
          ? tasks.filter((t) => matchesText(t, q)).sort((a, b) => score(a) - score(b) || String(b.updated_at).localeCompare(String(a.updated_at)))
          : [...tasks].sort((a, b) => String(b.updated_at).localeCompare(String(a.updated_at)));
        return found.slice(0, 12).map((task) => {
          const info = taskInfo(task, ctx.store.state.actionRequests, ctx.store.pullsOf(task.sandbox_id).get(task.id));
          return {
            mark: statusMark(info, 16),
            title: task.title || `Task #${task.number}`,
            sub: `#${task.number} · ${detailLine(task, info)}`,
            meta: relShort(task.updated_at),
            onPick: () => navigate(`/s/${encodeURIComponent(sandbox.slug)}/tasks/${task.number}`),
            onAlt: () => navigate(`/s/${encodeURIComponent(sandbox.slug)}/t/${task.number}`),
          };
        });
      },
      more: (text) => (text ? { title: `Show every task matching “${text}” in the list`, onPick: () => { searchQuery = text; render(); } } : null),
    });
  }

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
    const q = searchQuery.toLowerCase();
    const items = tasks.map((task) => { const info = taskInfo(task, ctx.store.state.actionRequests, ctx.store.pullsOf(task.sandbox_id).get(task.id)); return { task, info, group: groupOf(task, info) }; });

    // Author and origin choices come from all tasks, so a choice never disappears while selected.
    const count = (key) => { const m = new Map(); for (const { task } of items) { const k = key(task); if (k) m.set(k, (m.get(k) || 0) + 1); } return m; };
    const authorCounts = count((t) => t.created_by && t.created_by.login);
    const originCounts = count((t) => t.origin);
    const authors = [...authorCounts].sort((a, b) => (b[0] === me) - (a[0] === me) || b[1] - a[1] || a[0].localeCompare(b[0]));
    const origins = [...originCounts].sort((a, b) => b[1] - a[1]);

    const matches = items.filter(({ task }) => (!filters.authors.size || filters.authors.has(task.created_by && task.created_by.login))
      && (!filters.origins.size || filters.origins.has(task.origin))
      && (!q || matchesText(task, q)));
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
    searchChip.hidden = !searchQuery;
    searchChip.querySelector('.chip-label').textContent = `“${searchQuery}”`;
    originChip.update(origins);
    setChildren(summary, filtered()
      ? [`Showing ${visible.length} of ${tasks.length}`, ' · ', h('button', { type: 'button', class: 'link-btn', onClick: clearFilters }, 'Clear filters')]
      : plural(tasks.length, 'task'));

    const focusedId = listHost.contains(document.activeElement) ? document.activeElement.closest('[data-task-id]')?.dataset.taskId : null;
    // The shared empty state (#147), as on every other list page.
    if (!tasks.length) {
      setChildren(listHost, emptyState({ iconName: 'tasks', title: `No tasks in ${sandbox.name} yet`, text: 'Describe what you want built; the agent works on its own branch until it is done.',
        action: canStart ? h('button', { type: 'button', class: 'btn btn-primary', onClick: () => ctx.openNewTask && ctx.openNewTask() }, icon('plus', { size: 14 }), 'New task') : null }));
      return;
    }
    if (!visible.length) {
      setChildren(listHost, emptyState({ iconName: 'search', title: q ? `No tasks match “${searchQuery}”` : 'No tasks match these filters', text: 'The counts on the tabs show where the matches are.',
        action: h('button', { type: 'button', class: 'btn', onClick: clearFilters }, 'Clear filters') }));
      return;
    }

    const sections = [];
    nextRows = new Map();
    for (const group of GROUPS) {
      const list = filters.state === 'all' ? byGroup.get(group.id) : (filters.state === group.id ? visible : []);
      if (!list.length) continue;
      const open = filters.state === group.id || !collapsed.has(group.id);
      const limit = shownCount.get(group.id) || (group.id === 'closed' && filters.state === 'all' && !q ? CLOSED_PREVIEW : GROUP_PAGE);
      const shown = list.slice(0, limit);
      const more = Math.min(list.length - shown.length, GROUP_PAGE);
      const headId = `tl-head-${group.id}`;
      sections.push(h('section', { class: ['tl-group', `group-${group.id}`], 'aria-labelledby': headId },
        h('h2', { class: 'tl-group-title' }, h('button', { type: 'button', id: headId, class: 'tl-group-head', 'aria-expanded': String(open), onClick: () => {
          if (open) collapsed.add(group.id); else collapsed.delete(group.id);
          writeJson(COLLAPSED_KEY, [...collapsed]);
          render();
        } }, icon(open ? 'down' : 'right', { size: 12 }), icon(group.icon, { size: 14 }), h('span', { class: 'tl-group-name' }, group.name), h('span', { class: 'tl-group-count' }, String(list.length)))),
        open ? h('ul', { class: 'tl-rows' }, shown.map((item) => cachedRow(item))) : null,
        open && shown.length < list.length ? h('button', { type: 'button', class: 'tl-more', onClick: () => { shownCount.set(group.id, shown.length + more); render(); } }, more < list.length - shown.length ? `Show ${more} more (${list.length - shown.length} hidden)` : `Show ${more} more`) : null));
    }
    rowCache = nextRows;
    // Column names over the groups (the rows say the same to screen readers in their own words).
    const columns = h('div', { class: 'tl-columns', 'aria-hidden': 'true' }, ['Task', 'Status', 'Branch', 'Changes', 'Pull request', 'By', 'Started', 'Updated', ''].map((label) => h('span', null, label)));
    if (focusedId) setChildren(listHost, columns, sections);
    else keepFocus(listHost, () => setChildren(listHost, columns, sections));
    if (focusedId) listHost.querySelector(`[data-task-id="${CSS.escape(focusedId)}"] .tl-link`)?.focus({ preventScroll: true });
  }

  // A repaint reuses the row of every task whose row would come out the same (the key holds all a
  // row depends on), so a burst of events rebuilds only the rows that changed (PERF-02).
  let rowCache = new Map(); // task id → { key, el }
  let nextRows = new Map();
  function cachedRow(item) {
    const pr = ctx.store.pullsOf(sandbox.id).get(item.task.id) || null;
    const key = JSON.stringify([item.task, item.info, item.group, pr, ctx.store.gitRowOf(sandbox.id, item.task.id), sandbox.access, myLogin(), relShort(item.task.updated_at), relShort(item.task.created_at)]);
    const hit = rowCache.get(item.task.id);
    const el = hit && hit.key === key ? hit.el : rowView(item);
    nextRows.set(item.task.id, { key, el });
    return el;
  }

  /** The creator's initials; your own rows say so on hover and to screen readers. */
  function authorAvatar(login) {
    const who = login === myLogin() ? `you (${login})` : login;
    return h('span', { class: 'avatar tl-avatar', role: 'img', title: `Started by ${who}`, 'aria-label': `Started by ${who}` }, initials(login));
  }

  function rowView({ task, info, group }) {
    // A row opens the task's overview (times, sessions, plan, changes, Go to task), not its session.
    const path = `/s/${encodeURIComponent(sandbox.slug)}/tasks/${task.number}`;
    const login = task.created_by && task.created_by.login;
    const actions = taskActions(task, sandbox.access, ctx.store.pullsOf(sandbox.id).get(task.id));
    const git = ctx.store.gitRowOf(sandbox.id, task.id);
    const what = detailLine(task, info);
    const kebab = iconButton('more', `Actions for ${task.title || `task #${task.number}`}`, () => openTaskMenu(ctx, kebab, task), { class: 'tl-action', 'aria-haspopup': 'menu' });
    const row = h('li', { class: ['tl-row', `group-${group}`], dataset: { taskId: task.id, tone: info.tone } },
      h('a', { class: 'tl-link', href: href(path), dataset: { route: path }, title: `#${task.number} ${task.title}${task.branch ? ` · ${task.branch}` : ''}` },
        h('span', { class: 'tl-mark' }, statusMark(info, 16)),
        h('span', { class: 'tl-title' }, task.title || `Task #${task.number}`),
        task.unread_messages ? h('span', { class: 'task-unread', title: `${task.unread_messages} unread messages` }, String(task.unread_messages), h('span', { class: 'sr-only' }, ' unread')) : null,
        task.origin && task.origin !== 'ui' ? h('span', { class: 'tl-origin' }, ORIGINS[task.origin] || task.origin) : null,
        h('span', { class: 'sr-only' }, `, ${what}`)),
      h('span', { class: 'tl-cell tl-status', title: what, 'aria-hidden': 'true' }, what),
      h('span', { class: 'tl-cell tl-branch', title: task.branch || null }, task.branch || '—'),
      h('span', { class: 'tl-cell tl-diff' }, git && git.files_changed ? [h('span', { class: 'gt-add' }, `+${git.insertions ?? 0}`), ' ', h('span', { class: 'gt-del' }, `−${git.deletions ?? 0}`)] : '—'),
      prCell(ctx.store.pullsOf(sandbox.id).get(task.id)),
      // Every row says who started it, yours too, so the list answers "who did what" (#36).
      login ? authorAvatar(login) : h('span', { class: 'tl-avatar' }),
      h('span', { class: 'tl-cell tl-started', title: `Started ${dateTime(task.created_at)}` }, relShort(task.created_at)),
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

  // Pull requests and checks: the store keeps them fresh while this page (or the sidebar) shows them.
  const unwatchPulls = ctx.store.watchPulls(sandbox.id);

  // Keyboard: "/" opens the search, j/k move between tasks, "." opens the focused task's menu.
  const typing = (target) => target.closest('input, textarea, select, [contenteditable="true"], .xterm');
  function onKey(event) {
    if (event.metaKey || event.ctrlKey || event.altKey || event.defaultPrevented || !frame.el.isConnected) return;
    if (readJson('yulius.singleKeyShortcuts', true) === false) return; // turned off in Settings (WCAG 2.1.4)
    // "/" also works from the app's chrome (the sidebar link just clicked, the tabs, the rail), so
    // the search opens right after arriving; never from a tool panel, a dialog or a menu, nor while
    // typing (WCAG 2.1.4). The other keys move within the list, so they stay on this page.
    const onPage = event.target === document.body || frame.el.contains(event.target);
    if (typing(event.target)) return;
    if (event.key === '/' && (onPage || !event.target.closest('.panel, dialog, .menu, [role="menu"]'))) { event.preventDefault(); openSearch(); return; }
    if (!onPage) return;
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
  document.addEventListener('keydown', onKey);
  // Live events repaint at most every 200 ms; the user's own clicks and keys render at once.
  const repaint = coalesce(render, { minInterval: 200 });
  const offs = [
    ctx.store.on('tasks', (sid) => { if (sid === sandbox.id) repaint(); }),
    ctx.store.on('browser-action', (event) => { if (event.sandbox_id === sandbox.id) repaint(); }),
    ctx.store.on('pulls', (sid) => { if (sid === sandbox.id) repaint(); }),
  ];
  render();
  // A link to this page with other filters (the nav link, a shared URL) updates it in place.
  function update(route) {
    const next = route.query;
    filters.state = GROUPS.some((g) => g.id === next.get('state')) ? next.get('state') : 'all';
    filters.authors.clear(); (next.get('author') || '').split(',').filter(Boolean).forEach((a) => filters.authors.add(a));
    filters.origins.clear(); (next.get('origin') || '').split(',').filter(Boolean).forEach((o) => filters.origins.add(o));
    searchQuery = next.get('q') || '';
    render();
  }
  return { el: frame.el, key: `tasks:${sandbox.id}`, update, destroy: () => {
    alive = false;
    unwatchPulls();
    repaint.cancel();
    offs.forEach((off) => off());
    document.removeEventListener('keydown', onKey);
  } };
}

/** Archived tasks across visible sandboxes, with restore. */
export function mountArchivedPage(ctx) {
  const frame = pageFrame({ title: 'Archived tasks', sub: 'tasks you put away, from every sandbox, with restore' });
  const host = h('div');
  frame.body.append(host);
  const reload = loader(host, 'the archived tasks', () => ctx.api.get('/api/archived-tasks'), (tasks) => {
    const list = Array.isArray(tasks) ? tasks : [];
    return dataTable([
      { label: 'Sandbox', render: (t) => { const sb = ctx.store.sandboxById(t.sandbox_id); return sb ? sb.name : badge('unknown'); } },
      { label: '#', class: 'num', render: (t) => String(t.number) },
      { label: 'Title', class: 'title-cell', render: (t) => { const sb = ctx.store.sandboxById(t.sandbox_id); return sb ? taskLink(sb, t) : t.title; } },
      { label: 'Status', render: statusCell, phoneHide: true },
      { label: 'Archived', class: 'num', render: (t) => h('span', { title: dateTime(t.updated_at) }, relShort(t.updated_at)) },
      { label: '', render: (t) => {
        const sb = ctx.store.sandboxById(t.sandbox_id);
        const can = sb && allows(sb.access, 'write');
        return h('button', { type: 'button', class: 'btn btn-sm', disabled: !can, title: can ? 'Restore to its sandbox’s task list' : 'Requires write access', onClick: async () => {
          const updated = await restoreTask(ctx, t);
          if (updated) reload();
        } }, 'Restore');
      } },
    ], list, { empty: emptyState({ iconName: 'archive', title: 'No archived tasks', text: 'Archive a finished task from its action menu to put it away; restore it here any time.' }) });
  });
  return { el: frame.el, key: 'archived' };
}

/** CLI device approval (docs/api.md device flow): approve or deny a user code. */
export function mountDevicePage(ctx, route) {
  const frame = pageFrame({ title: 'CLI device approval', sub: 'approve a sign-in from the command line, such as “yulius login”' });
  const code = h('input', { placeholder: 'ABCD-EFGH', autocomplete: 'off', spellcheck: 'false', value: route.query.get('code') || route.query.get('user_code') || '', class: 'input mono code-input', 'aria-label': 'User code' });
  const result = h('div', { role: 'status' });
  const decide = (approve) => singleFlight(async () => {
    const userCode = code.value.trim().toUpperCase();
    if (!userCode) { code.focus(); return; }
    try {
      await ctx.api.post('/api/device/approve', { user_code: userCode, approve });
      setChildren(result, h('div', { class: 'banner banner-info' }, approve ? `Approved ${userCode}. The CLI receives a token on its next poll.` : `Denied ${userCode}. The CLI is refused on its next poll.`));
    } catch (error) {
      // Said where the answer appears, like the success (not a toast that vanishes).
      setChildren(result, h('div', { class: 'banner banner-danger' }, `Couldn’t ${approve ? 'approve' : 'deny'} ${userCode}: ${error.message || String(error)}`));
    }
  });
  const approveCode = decide(true);
  // Enter in the code field approves, as Enter submits every other form.
  code.addEventListener('keydown', (event) => { if (event.key === 'Enter' && !event.isComposing) { event.preventDefault(); approveCode(); } });
  frame.body.append(h('div', { class: 'card card-pad stack' },
    h('p', { class: 'text-2' }, 'Enter the code shown by the CLI. Approve only codes you started yourself; the CLI gets a token with your permissions.'),
    h('div', { class: 'row-wrap' }, code, h('button', { type: 'button', class: 'btn btn-primary', onClick: approveCode }, 'Approve'), h('button', { type: 'button', class: 'btn', onClick: decide(false) }, 'Deny')),
    result));
  return { el: frame.el, key: 'device' };
}

