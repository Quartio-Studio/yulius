// Global layout pieces (SPEC-001 §6.1): thin sandbox tab row with the Status panel, and the
// left sidebar with navigation, the active task list and the user menu at its foot.

import { h, icon, iconButton, openMenu, setChildren, errorToast } from './dom.js';
import { relShort, relLong, bytes, duration, dateTime } from './format.js';
import { statusInfo, harnessInfo, allows } from './status.js';
import { href } from './router.js';
import { openSandboxDialog, confirmRemoveSandbox, openTaskMenu } from './actions.js';
import { createNotificationCenter } from './notifications.js';
import { readJson, writeJson } from './store.js';

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

const STATUS_OPEN_KEY = 'yulius.statusOpen';
/** Account states as a person reads them: icon, tone and a plain label. */
const ACCOUNT_STATES = {
  ok: { icon: 'check', tone: 'ok', label: 'Ready' },
  limited: { icon: 'clock', tone: 'warn', label: 'Limit reached' },
  relogin: { icon: 'lock', tone: 'warn', label: 'Sign in again' },
  pending_login: { icon: 'lock', tone: 'warn', label: 'Not signed in' },
  error: { icon: 'warn', tone: 'danger', label: 'Error' },
};

function toneOf(percent) {
  if (!Number.isFinite(percent)) return 'unknown';
  return percent >= 90 ? 'danger' : percent >= 70 ? 'warn' : 'ok';
}

/** A bare usage bar (the number is shown next to it). Unknown renders hatched, never as zero. */
function usageBar(percent, label) {
  const known = Number.isFinite(percent);
  const value = known ? Math.max(0, Math.min(100, percent)) : 0;
  return h('div', { class: ['meter', `meter-${toneOf(percent)}`] },
    h('div', { class: 'meter-track', role: 'meter', 'aria-valuemin': 0, 'aria-valuemax': 100, 'aria-valuenow': known ? value : null, 'aria-label': label },
      h('div', { class: 'meter-fill', style: { width: `${value}%` } })));
}

const percentText = (percent) => (Number.isFinite(percent) ? `${Math.round(percent)}%` : '—');

/** "5-hour limit", "Weekly limit"… from a provider usage window. */
function limitName(minutes, fallback) {
  if (!minutes) return fallback;
  if (minutes === 10080) return 'Weekly limit';
  if (minutes === 1440) return 'Daily limit';
  if (minutes % 1440 === 0) return `${minutes / 1440}-day limit`;
  if (minutes % 60 === 0) return `${minutes / 60}-hour limit`;
  return `${minutes}-minute limit`;
}

/** One machine tile: icon and name, a big value, a bar and a detail line. */
function statTile(iconName, name, value, percent, detail) {
  return h('div', { class: ['stat-tile', `stat-${toneOf(percent)}`] },
    h('div', { class: 'stat-head' }, icon(iconName, { size: 15 }), h('span', null, name)),
    h('div', { class: 'stat-value' }, value),
    usageBar(percent, name),
    h('div', { class: 'stat-detail' }, detail || ' '));
}

/** A collapsible status section whose open state survives the panel's refresh. */
function statusSection(key, iconName, title, summary, body, openByDefault) {
  const saved = readJson(STATUS_OPEN_KEY, {}) || {};
  const details = h('details', { class: 'status-section status-fold', open: key in saved ? Boolean(saved[key]) : openByDefault },
    h('summary', null, icon('right', { size: 12 }), icon(iconName, { size: 15 }), h('h3', null, title), h('span', { class: 'status-summary-text' }, summary)),
    h('div', { class: 'status-fold-body' }, body));
  details.addEventListener('toggle', () => {
    const next = readJson(STATUS_OPEN_KEY, {}) || {};
    next[key] = details.open;
    writeJson(STATUS_OPEN_KEY, next);
  });
  return details;
}

/**
 * Render GET /api/status (`StatusView`: vm, sessions, sandboxes, accounts, contained) so it reads
 * in seconds: one health line, machine tiles, then sandboxes and accounts as collapsible lists.
 * VM fields are null when unknown (CPU needs two readings) and render as "—"/"unknown", never as
 * zero. Account id/priority/last_error/in_use_by are present for admins only.
 */
export function renderStatus(data) {
  const vm = data.vm || {};
  const memPct = pct(vm.memory_used_bytes, vm.memory_total_bytes);
  const diskPct = pct(vm.disk_used_bytes, vm.disk_total_bytes);
  const sessions = data.sessions || {};
  const sessionsPct = Number.isFinite(sessions.live) && sessions.limit ? pct(sessions.live, sessions.limit) : null;
  const sandboxes = Array.isArray(data.sandboxes) ? data.sandboxes : [];
  const accounts = Array.isArray(data.accounts) ? data.accounts : [];
  const load = Array.isArray(vm.load) ? vm.load.map((v) => Number(v).toFixed(2)) : Number.isFinite(vm.load_1m) ? [Number(vm.load_1m).toFixed(2)] : null;

  // One line that says whether anything needs a person; those accounts are listed first.
  const troubled = accounts.filter((a) => a.enabled !== false && a.status && a.status !== 'ok');
  const hot = [vm.cpu_percent, memPct, diskPct].some((v) => Number.isFinite(v) && v >= 90);
  let health = { tone: 'ok', icon: 'check', text: 'Everything is running normally' };
  if (data.contained === false) health = { tone: 'warn', icon: 'shield', text: 'Host mode: agent sessions are not isolated' };
  if (hot) health = { tone: 'warn', icon: 'warn', text: 'The machine is under heavy load' };
  if (troubled.length) health = { tone: 'warn', icon: 'warn', text: `${troubled.length} ${troubled.length === 1 ? 'account needs' : 'accounts need'} attention` };

  const facts = [
    h('span', { class: 'status-fact', title: 'Time since the machine started' }, icon('clock', { size: 13 }), Number.isFinite(vm.uptime_s) ? `Up ${duration(vm.uptime_s * 1000)}` : 'Uptime unknown'),
    load ? h('span', { class: 'status-fact', title: 'Load average over 1, 5 and 15 minutes' }, icon('pulse', { size: 13 }), `Load ${load.join(' · ')}`) : null,
    h('span', { class: ['status-fact', data.contained === false && 'fact-warn'], title: 'Containment: agent sessions run in an OS sandbox (SPEC-002 SEC-001)' }, icon('shield', { size: 13 }), data.contained === true ? 'Isolated' : data.contained === false ? 'Not isolated' : 'Isolation unknown'),
  ];

  const liveTotal = sandboxes.reduce((n, s) => n + (Number.isFinite(s.live_sessions) ? s.live_sessions : 0), 0);
  const busy = sandboxes.filter((s) => s.live_sessions > 0).length;
  const sandboxRows = sandboxes.map((s) => h('div', { class: 'status-row' },
    icon('folder', { size: 15 }),
    h('span', { class: 'status-row-name' }, s.name || s.slug || s.id),
    Number.isFinite(s.live_sessions) && s.live_sessions > 0
      ? h('span', { class: 'status-count live', title: `${s.live_sessions} live sessions` }, h('span', { class: 'live-dot' }), `${s.live_sessions} running`)
      : h('span', { class: 'status-count' }, Number.isFinite(s.live_sessions) ? 'idle' : '—')));

  const ready = accounts.filter((a) => a.enabled !== false && a.status === 'ok').length;
  const accountSummary = [`${ready} ready`, troubled.length ? `${troubled.length} need attention` : null].filter(Boolean).join(' · ');

  return [
    h('div', { class: ['status-health', `health-${health.tone}`] }, icon(health.icon, { size: 16 }), h('span', null, health.text)),
    h('div', { class: 'status-section' },
      h('h3', null, 'Machine'),
      h('div', { class: 'stat-tiles' },
        statTile('cpu', 'CPU', percentText(vm.cpu_percent), vm.cpu_percent, vm.cpu_count ? `${vm.cpu_count} cores` : null),
        statTile('memory', 'Memory', percentText(memPct), memPct, vm.memory_total_bytes ? `${bytes(vm.memory_used_bytes)} of ${bytes(vm.memory_total_bytes)}` : null),
        statTile('disk', 'Disk', diskPct === null ? 'unknown' : percentText(diskPct), diskPct, vm.disk_total_bytes ? `${bytes(vm.disk_used_bytes)} of ${bytes(vm.disk_total_bytes)}` : null),
        statTile('terminal', 'Sessions', Number.isFinite(sessions.live) ? `${sessions.live}${sessions.limit ? ` / ${sessions.limit}` : ''}` : '—', sessionsPct, sessions.queued ? `${sessions.queued} queued` : 'agent sessions running')),
      h('div', { class: 'status-facts' }, facts)),
    sandboxes.length ? statusSection('sandboxes', 'folder', 'Sandboxes', `${liveTotal} running in ${busy} of ${sandboxes.length}`, h('div', { class: 'status-rows' }, sandboxRows), false) : null,
    statusSection('accounts', 'user', 'Accounts', accounts.length ? accountSummary : 'none connected',
      accounts.length === 0 ? h('div', { class: 'muted small' }, 'No accounts connected.') : h('div', { class: 'account-list' }, [...troubled, ...accounts.filter((a) => !troubled.includes(a))].map(accountUsage)), troubled.length > 0 || accounts.length <= 3),
  ];
}

/** Account usage block shared by the Status panel and the Accounts page. */
export function accountUsage(account) {
  const usage = account.usage || {};
  const provider = account.provider === 'anthropic' ? 'Claude' : account.provider;
  const state = account.enabled === false
    ? { icon: 'x', tone: 'muted', label: 'Disabled' }
    : ACCOUNT_STATES[account.status] || { icon: 'warn', tone: 'muted', label: account.status || 'Unknown' };
  const limitRow = (w, fallback) => {
    const name = limitName(w && w.window_minutes, fallback);
    const used = w ? w.used_percent : null;
    return h('div', { class: 'limit-row' },
      h('span', { class: 'limit-name' }, name),
      usageBar(used, name),
      h('span', { class: ['limit-value', `tone-${toneOf(used)}`] }, percentText(used)),
      h('span', { class: 'limit-reset', title: w && w.resets_at ? dateTime(w.resets_at) : '' }, w && w.resets_at ? `resets ${relLong(w.resets_at)}` : ''));
  };
  const showProvider = provider && !String(account.label || '').toLowerCase().includes(String(provider).toLowerCase());
  return h('div', { class: ['account-card', account.enabled === false && 'disabled'] },
    h('div', { class: 'account-head' },
      icon('user', { size: 15 }),
      h('strong', { class: 'ellipsis' }, account.label || account.id),
      showProvider ? h('span', { class: 'muted xs' }, provider) : null,
      h('span', { class: ['account-state', `state-${state.tone}`] }, icon(state.icon, { size: 12 }), state.label),
      h('span', { class: 'spacer' }),
      account.in_use_by ? h('span', { class: 'status-count live', title: `${account.in_use_by} live tasks use this account` }, h('span', { class: 'live-dot' }), `${account.in_use_by} ${account.in_use_by === 1 ? 'task' : 'tasks'}`) : null),
    account.last_error ? h('div', { class: 'account-error' }, account.last_error) : null,
    limitRow(usage.primary, 'Short-term limit'),
    limitRow(usage.secondary, 'Long-term limit'),
    h('div', { class: 'muted xs' }, usage.observed_at ? `updated ${relLong(usage.observed_at)}` : 'usage not known yet'),
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
  const newTask = h('a', { class: 'btn btn-newtask', href: '#', dataset: { route: '/' } }, h('span', { class: 'newtask-plus', 'aria-hidden': 'true' }, icon('plus', { size: 14 })), 'New task');
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
