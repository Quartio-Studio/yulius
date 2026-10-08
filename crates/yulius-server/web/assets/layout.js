// Global layout pieces (SPEC-001 §6.1): thin sandbox tab row with the Status panel, and the
// left sidebar with navigation, the active task list and the user menu at its foot.

import { h, icon, iconButton, openMenu, setChildren, meter, errorToast, badge } from './dom.js';
import { relShort, relLong, bytes, duration, windowLabel, dateTime } from './format.js';
import { statusInfo, harnessInfo, allows } from './status.js';
import { href } from './router.js';
import { openSandboxDialog, confirmRemoveSandbox, openTaskMenu } from './actions.js';
import { createNotificationCenter } from './notifications.js';

const NAV = [
  ['tasks', 'Tasks'],
  ['git', 'Git'],
  ['workflows', 'Workflows'],
  ['skills', 'Skills'],
  ['automations', 'Automations'],
  ['mcp', 'MCP'],
  ['dashboard', 'Dashboard'],
];

// ---------------------------------------------------------------- tab row

/** Thin sandbox tab row: tabs (or a dropdown on phones), "+", Status, notifications, tools toggle. */
export function createTabRow(ctx) {
  const tabs = h('nav', { class: 'tabrow-tabs', 'aria-label': 'Sandboxes' });
  const select = h('select', { class: 'select tab-select', 'aria-label': 'Sandbox', onChange: () => {
    if (select.value === '__add') {
      select.value = currentSlug || '';
      openSandboxDialog(ctx);
    } else if (select.value) {
      ctx.navigate(`/s/${encodeURIComponent(select.value)}`);
    }
  } });
  const hamburger = iconButton('menu', 'Open navigation', () => document.body.classList.toggle('drawer-open'), { class: 'hamburger' });
  const addButton = iconButton('plus', 'Add sandbox: create or import a repository', () => openSandboxDialog(ctx), { class: 'tab-add' });
  const contained = h('span', { class: 'chip chip-warn', hidden: true, title: 'Host mode: agent sessions are not isolated by an OS sandbox on this server (SPEC-002 SEC-001).' }, icon('warn', { size: 12 }), h('span', { class: 'chip-text' }, 'Not contained'));
  const eventDot = h('span', { class: 'ev-dot', title: 'Live updates connected' });
  const statusButton = h('button', { type: 'button', class: 'btn btn-status', 'aria-expanded': 'false', 'aria-haspopup': 'dialog', title: 'Status: machine and account usage' }, eventDot, h('span', { class: 'status-text' }, 'Status'), icon('down', { size: 12 }));
  // Shows/hides the right tool rail (browser, editor, shell, messages); hidden by default.
  const toolsButton = iconButton('tools', 'Show tools (browser, editor, shell, messages)', () => ctx.toggleRail && ctx.toggleRail(), { class: 'tools-toggle' });
  const toolsDot = h('span', { class: 'rail-dot', hidden: true });
  toolsButton.append(toolsDot);
  toolsButton.style.position = 'relative';
  ctx.onToolsAttention = (attention) => { toolsDot.hidden = !attention; };
  ctx.onRailChange = (visible) => {
    toolsButton.classList.toggle('active', visible);
    toolsButton.setAttribute('aria-pressed', String(visible));
    toolsButton.title = visible ? 'Hide tools' : 'Show tools (browser, editor, shell, messages)';
    toolsButton.setAttribute('aria-label', toolsButton.title);
  };
  const notifications = createNotificationCenter(ctx);
  ctx.notifications = notifications;
  const statusPanel = createStatusPanel(ctx, statusButton);
  statusButton.addEventListener('click', () => statusPanel.toggle());

  const el = h('header', { class: 'tabrow' },
    h('div', { class: 'brand', title: 'Yulius' }, 'Y'),
    hamburger,
    tabs,
    select,
    addButton,
    h('div', { class: 'tabrow-right' }, contained, statusButton, notifications.button, toolsButton),
  );

  let currentSlug = null;
  let dragId = null;

  function renderTabs() {
    const sandboxes = ctx.store.state.sandboxes;
    const anchors = sandboxes.map((sandbox) => {
      const counts = sandbox.counts || {};
      const a = h('a', {
        class: ['tab', sandbox.slug === currentSlug && 'active'],
        href: href(`/s/${encodeURIComponent(sandbox.slug)}`),
        dataset: { route: `/s/${encodeURIComponent(sandbox.slug)}`, id: sandbox.id },
        draggable: 'true',
        title: `${sandbox.repo ? `${sandbox.repo.owner}/${sandbox.repo.name}` : sandbox.name} · ${sandbox.access} access`,
        'aria-current': sandbox.slug === currentSlug ? 'page' : null,
      },
      h('span', { class: 'tab-name' }, sandbox.name),
      counts.running ? h('span', { class: 'tab-running', title: `${counts.running} running` }, String(counts.running)) : null,
      counts.needs_attention ? h('span', { class: 'tab-dot', title: `${counts.needs_attention} need attention` }) : null);
      a.addEventListener('contextmenu', (event) => {
        event.preventDefault();
        openMenu({ x: event.clientX, y: event.clientY }, [
          { label: 'Open', onSelect: () => ctx.navigate(`/s/${encodeURIComponent(sandbox.slug)}`) },
          sandbox.repo && sandbox.repo.url ? { label: 'Open on GitHub', onSelect: () => window.open(sandbox.repo.url, '_blank', 'noopener,noreferrer') } : null,
          { separator: true },
          { label: 'Remove from Yulius…', danger: true, disabled: !allows(sandbox.access, 'admin'), title: allows(sandbox.access, 'admin') ? null : 'Requires repository admin', onSelect: () => confirmRemoveSandbox(ctx, sandbox) },
        ]);
      });
      a.addEventListener('dragstart', (event) => { dragId = sandbox.id; event.dataTransfer.effectAllowed = 'move'; event.dataTransfer.setData('text/plain', sandbox.id); });
      a.addEventListener('dragover', (event) => { if (dragId && dragId !== sandbox.id) { event.preventDefault(); a.classList.add('drag-over'); } });
      a.addEventListener('dragleave', () => a.classList.remove('drag-over'));
      a.addEventListener('drop', (event) => {
        event.preventDefault();
        a.classList.remove('drag-over');
        if (!dragId || dragId === sandbox.id) return;
        const ids = ctx.store.state.sandboxes.map((s) => s.id).filter((id) => id !== dragId);
        ids.splice(ids.indexOf(sandbox.id), 0, dragId);
        dragId = null;
        ctx.store.setSandboxOrder(ids);
      });
      a.addEventListener('dragend', () => { dragId = null; });
      return a;
    });
    setChildren(tabs, anchors);
    const active = tabs.querySelector('.tab.active');
    if (active) active.scrollIntoView({ block: 'nearest', inline: 'nearest' });

    setChildren(select,
      currentSlug ? null : h('option', { value: '' }, 'Choose sandbox…'),
      sandboxes.map((s) => h('option', { value: s.slug }, `${s.name}${s.counts && s.counts.needs_attention ? ' (!)' : ''}`)),
      h('option', { value: '__add' }, '+ Add sandbox…'));
    select.value = currentSlug || '';
  }

  function renderMe() {
    const me = ctx.store.state.me;
    if (!me) return;
    contained.hidden = !(me.instance && me.instance.contained === false);
  }

  ctx.store.on('sandboxes', renderTabs);
  ctx.store.on('events-status', (status) => {
    const ok = status === 'open';
    eventDot.classList.toggle('off', !ok);
    eventDot.title = ok ? 'Live updates connected' : 'Live updates reconnecting…';
  });

  return {
    el,
    update(slug) {
      currentSlug = slug;
      renderTabs();
      renderMe();
    },
    renderMe,
  };
}

function initials(name) {
  const parts = String(name).trim().split(/[\s._-]+/).filter(Boolean);
  if (parts.length === 0) return '?';
  return (parts[0][0] + (parts[1] ? parts[1][0] : parts[0][1] || '')).slice(0, 2);
}

function openUserMenu(ctx, anchor) {
  const me = ctx.store.state.me || { user: {} };
  const user = me.user || {};
  const superadmin = Boolean(user.is_superadmin);
  const instanceAdmin = superadmin || Boolean(user.is_instance_admin);
  const sandbox = ctx.currentSandbox() || ctx.store.state.sandboxes[0] || null;
  const items = [
    { header: `${user.display_name || user.login || 'Signed in'}${user.login ? ` · @${user.login}` : ''}${superadmin ? ' · superadmin' : ''}` },
    { label: 'Archived tasks', onSelect: () => ctx.navigate('/archived') },
    { label: 'CLI device approval', onSelect: () => ctx.navigate('/device') },
  ];
  if (instanceAdmin) {
    items.push({ separator: true }, { header: 'Administration' });
    if (superadmin) items.push({ label: 'Users', onSelect: () => ctx.navigate('/admin/users') });
    items.push({ label: 'Accounts', onSelect: () => ctx.navigate('/admin/accounts') });
    if (superadmin) items.push({ label: 'GitHub connection', onSelect: () => ctx.navigate('/admin/github') });
    items.push(
      { label: 'Instance MCP', disabled: !sandbox, title: sandbox ? null : 'Open a sandbox first', onSelect: () => ctx.navigate(`/s/${encodeURIComponent(sandbox.slug)}/mcp?scope=instance`) },
      { label: 'Instance skills', disabled: !sandbox, title: sandbox ? null : 'Open a sandbox first', onSelect: () => ctx.navigate(`/s/${encodeURIComponent(sandbox.slug)}/skills?source=instance`) },
    );
  }
  items.push({ separator: true }, { label: 'Sign out', onSelect: () => ctx.signOut() });
  openMenu(anchor, items);
}

// ---------------------------------------------------------------- status panel

/** Hidden-by-default Status panel with VM metrics and account usage (UI-007, AC-STA-1). */
function createStatusPanel(ctx, button) {
  let el = null;
  let timer = null;
  let onDoc = null;
  let onKey = null;

  async function load() {
    if (!el) return;
    try {
      const data = await ctx.api.get('/api/status');
      if (el) setChildren(el, renderStatus(data || {}));
    } catch (error) {
      if (el) setChildren(el, h('div', { class: 'status-section' }, h('div', { class: 'placeholder placeholder-error' }, error.message)));
    }
  }

  function open() {
    el = h('div', { class: 'status-pop', role: 'dialog', 'aria-label': 'Status' }, h('div', { class: 'placeholder placeholder-loading' }, 'Loading status…'));
    document.body.append(el);
    button.setAttribute('aria-expanded', 'true');
    load();
    timer = setInterval(load, 10000);
    onDoc = (event) => { if (el && !el.contains(event.target) && !button.contains(event.target)) close(); };
    onKey = (event) => { if (event.key === 'Escape') { close(); button.focus(); } };
    setTimeout(() => document.addEventListener('pointerdown', onDoc, true), 0);
    document.addEventListener('keydown', onKey);
  }

  function close() {
    clearInterval(timer);
    timer = null;
    document.removeEventListener('pointerdown', onDoc, true);
    document.removeEventListener('keydown', onKey);
    if (el) el.remove();
    el = null;
    button.setAttribute('aria-expanded', 'false');
  }

  return { toggle: () => (el ? close() : open()), close };
}

function pct(used, total) {
  return Number.isFinite(used) && Number.isFinite(total) && total > 0 ? (used / total) * 100 : null;
}

/**
 * Render GET /api/status (`StatusView`: vm, sessions, sandboxes, accounts, contained).
 * VM fields are null when unknown (CPU needs two readings) and render as "—"/"unknown",
 * never as zero. Account id/priority/last_error/in_use_by are present for admins only.
 */
export function renderStatus(data) {
  const vm = data.vm || {};
  const memPct = pct(vm.memory_used_bytes, vm.memory_total_bytes);
  const diskPct = pct(vm.disk_used_bytes, vm.disk_total_bytes);
  const sessions = data.sessions || {};
  const sandboxes = Array.isArray(data.sandboxes) ? data.sandboxes : [];
  const accounts = Array.isArray(data.accounts) ? data.accounts : [];
  const load = Array.isArray(vm.load) ? vm.load.map((v) => Number(v).toFixed(2)).join(' · ') : Number.isFinite(vm.load_1m) ? Number(vm.load_1m).toFixed(2) : '—';
  return [
    h('div', { class: 'status-section' },
      h('h3', null, 'Machine'),
      h('div', { class: 'status-grid' },
        meter(vm.cpu_percent ?? null, { label: `CPU${vm.cpu_count ? ` · ${vm.cpu_count} cores` : ''}` }),
        meter(memPct, { label: 'Memory', detail: vm.memory_total_bytes ? `${bytes(vm.memory_used_bytes)} of ${bytes(vm.memory_total_bytes)}` : null }),
        meter(diskPct, { label: 'Disk', detail: vm.disk_total_bytes ? `${bytes(vm.disk_used_bytes)} of ${bytes(vm.disk_total_bytes)}` : null }),
        h('dl', { class: 'kv' },
          h('dt', null, 'Load'), h('dd', null, load),
          h('dt', null, 'Uptime'), h('dd', null, Number.isFinite(vm.uptime_s) ? duration(vm.uptime_s * 1000) : '—'),
          h('dt', null, 'Sessions'), h('dd', null, Number.isFinite(sessions.live) ? `${sessions.live}${sessions.limit ? ` / ${sessions.limit}` : ''}${sessions.queued ? ` · ${sessions.queued} queued` : ''}` : '—'),
          h('dt', null, 'Containment'), h('dd', null, data.contained === true ? 'on' : data.contained === false ? 'off (host mode)' : '—'))),
    ),
    sandboxes.length ? h('div', { class: 'status-section' },
      h('h3', null, 'Sandboxes'),
      h('table', { class: 'table' },
        h('thead', null, h('tr', null, h('th', null, 'Sandbox'), h('th', { class: 'num' }, 'Live sessions'))),
        h('tbody', null, sandboxes.map((s) => h('tr', null,
          h('td', { class: 'title-cell' }, s.name || s.slug || s.id),
          h('td', { class: 'num' }, Number.isFinite(s.live_sessions) ? String(s.live_sessions) : '—')))))) : null,
    h('div', { class: 'status-section' },
      h('h3', null, 'Accounts'),
      accounts.length === 0 ? h('div', { class: 'muted small' }, 'No accounts connected.') : accounts.map(accountUsage),
    ),
  ];
}

/** Account usage block shared by the Status panel and the Accounts page. */
export function accountUsage(account) {
  const usage = account.usage || {};
  const provider = account.provider === 'anthropic' ? 'Claude' : account.provider;
  const tone = { ok: 'ok', relogin: 'warn', limited: 'warn', error: 'danger' }[account.status] || '';
  const windowMeter = (w, name) => w ? meter(w.used_percent ?? null, {
    label: `${name}${w.window_minutes ? ` · ${windowLabel(w.window_minutes)}` : ''}`,
    detail: w.resets_at ? `resets ${relLong(w.resets_at)} (${dateTime(w.resets_at)})` : null,
  }) : meter(null, { label: name });
  return h('div', { class: 'stack-sm', style: { opacity: account.enabled === false ? 0.55 : 1 } },
    h('div', { class: 'row' },
      h('strong', { class: 'ellipsis' }, account.label || account.id),
      badge(provider, 'accent'),
      badge(account.status || 'unknown', tone),
      account.enabled === false ? badge('disabled') : null,
      h('span', { class: 'spacer' }),
      account.in_use_by ? h('span', { class: 'muted xs' }, `${account.in_use_by} in use`) : null),
    h('div', { class: 'status-grid' }, windowMeter(usage.primary, 'Primary'), windowMeter(usage.secondary, 'Secondary')),
    usage.observed_at ? h('div', { class: 'muted xs' }, `provider usage as of ${relLong(usage.observed_at)}`) : h('div', { class: 'muted xs' }, 'provider usage not known yet'),
    account.last_error ? h('div', { class: 'field-error' }, account.last_error) : null,
  );
}

// ---------------------------------------------------------------- sidebar

/**
 * Left sidebar: New task, navigation, active task list with status icons (UI-004, UI-005) and
 * the signed-in user at its foot, which opens the user menu.
 */
export function createSidebar(ctx) {
  // The logo slot holds the favicon's Y until Yulius has a real logo.
  const brand = h('a', { class: 'sidebar-brand', href: href('/'), dataset: { route: '/' }, title: 'Yulius' },
    h('span', { class: 'brand-logo', 'aria-hidden': 'true' }, icon('logo', { size: 20 })),
    h('span', { class: 'brand-text' }, h('span', { class: 'brand-name' }, 'Yulius'), h('span', { class: 'brand-by' }, 'by Quartio team')));
  const newTask = h('a', { class: 'btn btn-primary btn-newtask', href: '#', dataset: { route: '/' } }, icon('plus', { size: 14 }), 'New task');
  const nav = h('nav', { class: 'nav', 'aria-label': 'Sandbox' });
  const count = h('span', null, '');
  const list = h('div', { class: 'task-list', role: 'list', 'aria-label': 'Active tasks' });
  const avatar = h('span', { class: 'avatar', 'aria-hidden': 'true' }, '?');
  const userName = h('span', { class: 'user-name' });
  const userButton = h('button', { type: 'button', class: 'user-btn', 'aria-label': 'User menu', 'aria-haspopup': 'menu', onClick: () => openUserMenu(ctx, userButton) }, avatar, userName);
  const el = h('aside', { class: 'sidebar' },
    h('div', { class: 'sidebar-full', style: { display: 'flex', flexDirection: 'column', minHeight: 0, flex: 1 } },
      h('div', { class: 'sidebar-top' }, brand, newTask),
      nav,
      h('div', { class: 'sidebar-sep' }, h('span', null, 'Active tasks'), count),
      list),
    h('div', { class: 'sidebar-foot' }, userButton),
  );

  let sandbox = null;
  let route = null;
  const rows = new Map();

  function renderUser() {
    const user = (ctx.store.state.me && ctx.store.state.me.user) || {};
    const name = user.display_name || user.login || '?';
    avatar.textContent = initials(name);
    userName.textContent = name;
    userButton.title = `${name}${user.login ? ` (@${user.login})` : ''}`;
  }

  function renderNav() {
    const slug = sandbox ? encodeURIComponent(sandbox.slug) : null;
    newTask.dataset.route = slug ? `/s/${slug}` : '/';
    newTask.href = href(newTask.dataset.route);
    newTask.classList.toggle('disabled', !slug);
    setChildren(nav, NAV.map(([page, label]) => {
      const path = slug ? `/s/${slug}/${page}` : '/';
      const active = route && route.name === 'page' && route.page === page;
      return h('a', { class: ['nav-item', active && 'active'], href: href(path), dataset: { route: path }, 'aria-current': active ? 'page' : null }, label);
    }));
  }

  function rowFor(task) {
    let row = rows.get(task.id);
    if (!row) {
      const glyph = h('span', { class: 'glyph' });
      const title = h('span', { class: 'task-title' });
      const harness = h('span', { class: 'task-harness' });
      const time = h('span', { class: 'task-time' });
      const unread = h('span', { class: 'task-unread' });
      const meta = h('span', { class: 'task-meta' }, unread, harness, time);
      const anchor = h('a', { class: 'task-row', role: 'listitem' }, glyph, title, meta);
      const kebab = iconButton('more', 'Task actions', (event) => {
        event.preventDefault();
        event.stopPropagation();
        openTaskMenu(ctx, kebab, row.task);
      }, { class: 'task-kebab' });
      anchor.append(kebab);
      anchor.addEventListener('contextmenu', (event) => {
        event.preventDefault();
        openTaskMenu(ctx, { x: event.clientX, y: event.clientY }, row.task);
      });
      anchor.addEventListener('click', () => document.body.classList.remove('drawer-open'));
      row = { anchor, glyph, title, harness, time, unread, task };
      rows.set(task.id, row);
    }
    row.task = task;
    const info = statusInfo(task);
    const hi = harnessInfo(task.harness);
    const path = `/s/${encodeURIComponent(sandbox.slug)}/t/${task.number}`;
    const active = route && route.name === 'task' && route.number === task.number;
    row.anchor.href = href(path);
    row.anchor.dataset.route = path;
    row.anchor.classList.toggle('active', Boolean(active));
    row.anchor.setAttribute('aria-current', active ? 'page' : 'false');
    row.anchor.title = `#${task.number} ${task.title} — ${info.label} · ${hi.name}${task.branch ? ` · ${task.branch}` : ''}`;
    row.glyph.className = `glyph tone-${info.tone}`;
    row.glyph.textContent = info.glyph;
    row.glyph.setAttribute('aria-label', info.label);
    row.glyph.setAttribute('role', 'img');
    row.title.textContent = task.title || `Task #${task.number}`;
    row.harness.textContent = hi.glyph;
    row.harness.className = `task-harness ${task.harness}`;
    row.harness.title = hi.name;
    row.time.textContent = relShort(task.updated_at);
    row.time.title = dateTime(task.updated_at);
    row.unread.hidden = !task.unread_messages;
    row.unread.textContent = task.unread_messages ? String(task.unread_messages) : '';
    row.unread.title = task.unread_messages ? `${task.unread_messages} unread messages` : '';
    return row;
  }

  function renderTasks() {
    if (!sandbox) {
      setChildren(list, h('div', { class: 'placeholder' }, 'No sandbox selected.'));
      count.textContent = '';
      return;
    }
    const all = ctx.store.tasksOf(sandbox.id);
    if (all === null) {
      setChildren(list, h('div', { class: 'placeholder placeholder-loading' }, 'Loading…'));
      return;
    }
    // A task the user left normally is done for now: it stays under Tasks (with Continue) but
    // leaves the Active list (HRN-002).
    const tasks = all.filter((task) => task.status !== 'closed');
    count.textContent = tasks.length ? String(tasks.length) : '';
    if (tasks.length === 0) {
      rows.clear();
      setChildren(list, h('div', { class: 'placeholder small' }, 'No active tasks. Start one with “New task”.'));
      return;
    }
    const seen = new Set();
    const ordered = tasks.map((task) => { seen.add(task.id); return rowFor(task); });
    for (const id of [...rows.keys()]) if (!seen.has(id)) rows.delete(id);
    // Reorder only when needed so focus is not lost on every update.
    const current = [...list.children];
    if (current.length !== ordered.length || ordered.some((row, i) => current[i] !== row.anchor)) {
      setChildren(list, ordered.map((row) => row.anchor));
    }
  }

  ctx.store.on('tasks', (sandboxId) => { if (sandbox && sandboxId === sandbox.id) renderTasks(); });
  // Refresh relative times twice a minute (single bounded timer).
  setInterval(() => {
    for (const row of rows.values()) row.time.textContent = relShort(row.task.updated_at);
  }, 30000);

  return {
    el,
    update(nextRoute, nextSandbox) {
      if (!nextSandbox || !sandbox || nextSandbox.id !== sandbox.id) rows.clear();
      route = nextRoute;
      sandbox = nextSandbox;
      renderUser();
      renderNav();
      renderTasks();
    },
    refresh: renderTasks,
  };
}

/** Surface API failures from layout-level loaders without breaking rendering. */
export function reportLoadError(error, what) {
  errorToast(error, `Could not load ${what}`);
}
