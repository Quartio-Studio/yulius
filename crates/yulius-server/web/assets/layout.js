// Global layout pieces (SPEC-001 §6.1): thin sandbox tab row with the Status panel, and the
// left sidebar with navigation, the active task list and the user menu at its foot.

import { h, icon, iconButton, openMenu, setChildren, errorToast, statusMark, popoverFocus, keepFocus } from './dom.js';
import { relShort, relLong, bytes, duration, dateTime, resetLabel } from './format.js';
import { taskInfo, allows } from './status.js';
import { href } from './router.js';
import { openSandboxDialog, confirmRemoveSandbox, openTaskMenu } from './actions.js';
import { createNotificationCenter } from './notifications.js';
import { BRAND } from './brand.js';
import { readJson, writeJson } from './store.js';

const NAV = [
  ['tasks', 'Tasks', 'tasks'],
  ['git', 'Git', 'branch'],
  ['workflows', 'Workflows', 'workflow'],
  ['skills', 'Skills', 'book'],
  ['automations', 'Automations', 'zap'],
  ['mcp', 'MCP', 'plug'],
  ['dashboard', 'Dashboard', 'chart'],
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
  const hamburger = iconButton('menu', 'Open navigation', () => document.body.classList.toggle('drawer-open'), { class: 'hamburger', 'aria-controls': 'sidebar', 'aria-expanded': 'false' });
  const addButton = iconButton('plus', 'Add sandbox: create or import a repository', () => openSandboxDialog(ctx), { class: 'tab-add' });
  const contained = h('span', { class: 'chip chip-warn', hidden: true, title: 'Host mode: agent sessions on this server are not isolated in an OS sandbox, so an agent can reach files outside its sandbox.' }, icon('warn', { size: 12 }), h('span', { class: 'chip-text' }, 'Not contained'));
  const eventDot = h('span', { class: 'ev-dot', title: 'Live updates connected' });
  const statusButton = h('button', { type: 'button', class: 'btn btn-status', 'aria-expanded': 'false', 'aria-haspopup': 'dialog', title: 'Status: machine and account usage' }, eventDot, h('span', { class: 'status-text' }, 'Status'), icon('down', { size: 12 }));
  // Shows/hides the right tool rail (browser, editor, shell, messages); hidden by default.
  const toolsButton = iconButton('tools', 'Show tools (browser, editor, shell, messages)', () => ctx.toggleRail && ctx.toggleRail(), { class: 'tools-toggle' });
  const toolsDot = h('span', { class: 'rail-dot', hidden: true });
  toolsButton.append(toolsDot);
  toolsButton.style.position = 'relative';
  // The button's name carries the attention dot too ("…, an agent needs your action").
  let toolsAttention = false;
  let railVisible = false;
  const labelTools = () => {
    toolsButton.title = railVisible ? 'Hide tools' : 'Show tools (browser, editor, shell, messages)';
    toolsButton.setAttribute('aria-label', `${toolsButton.title}${toolsAttention ? ', an agent needs your action' : ''}`);
  };
  ctx.onToolsAttention = (attention) => { toolsAttention = attention; toolsDot.hidden = !attention; labelTools(); };
  ctx.onRailChange = (visible) => {
    railVisible = visible;
    toolsButton.classList.toggle('active', visible);
    toolsButton.setAttribute('aria-pressed', String(visible));
    labelTools();
  };
  const notifications = createNotificationCenter(ctx);
  ctx.notifications = notifications;
  const statusPanel = createStatusPanel(ctx, statusButton);
  statusButton.addEventListener('click', () => statusPanel.toggle());
  ctx.openStatus = () => statusPanel.open();

  const el = h('header', { class: 'tabrow' },
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
      h('span', { class: 'sr-only' }, [counts.running ? `, ${counts.running} running` : '', counts.needs_attention ? `, ${counts.needs_attention} need${counts.needs_attention === 1 ? 's' : ''} attention` : ''].join('')),
      counts.running ? h('span', { class: 'tab-running', title: `${counts.running} running`, 'aria-hidden': 'true' }, String(counts.running)) : null,
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

export function initials(name) {
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
    { label: 'Archived tasks', icon: 'archive', onSelect: () => ctx.navigate('/archived') },
    { label: 'CLI device approval', icon: 'key', onSelect: () => ctx.navigate('/device') },
  ];
  if (instanceAdmin) {
    items.push({ separator: true }, { header: 'Administration' });
    if (superadmin) items.push({ label: 'Users', icon: 'users', onSelect: () => ctx.navigate('/admin/users') });
    items.push({ label: 'AI accounts', icon: 'user', onSelect: () => ctx.navigate('/admin/accounts') });
    if (superadmin) items.push({ label: 'GitHub connection', icon: 'github', onSelect: () => ctx.navigate('/admin/github') });
    items.push(
      { label: 'Instance MCP', icon: 'plug', disabled: !sandbox, title: sandbox ? null : 'Open a sandbox first', onSelect: () => ctx.navigate(`/s/${encodeURIComponent(sandbox.slug)}/mcp?scope=instance`) },
      { label: 'Instance skills', icon: 'book', disabled: !sandbox, title: sandbox ? null : 'Open a sandbox first', onSelect: () => ctx.navigate(`/s/${encodeURIComponent(sandbox.slug)}/skills?source=instance`) },
    );
  }
  items.push({ separator: true }, { label: 'Settings', icon: 'gear', onSelect: () => ctx.navigate('/settings') }, { label: 'Sign out', icon: 'logout', onSelect: () => ctx.signOut() });
  openMenu(anchor, items);
}

// ---------------------------------------------------------------- status panel

/** Hidden-by-default Status panel with VM metrics and account usage (UI-007, AC-STA-1). */
function createStatusPanel(ctx, button) {
  let offFocus = null;
  let el = null;
  let timer = null;
  let onDoc = null;
  let onKey = null;

  async function load() {
    if (!el) return;
    try {
      const data = await ctx.api.get('/api/status');
      if (el) keepFocus(el, () => setChildren(el, renderStatus(data || {})));
    } catch (error) {
      if (el) setChildren(el, h('div', { class: 'status-section' }, h('div', { class: 'placeholder placeholder-error' }, error.message)));
    }
  }

  function open() {
    el = h('div', { class: 'status-pop', role: 'dialog', 'aria-label': 'Status' }, h('div', { class: 'placeholder placeholder-loading' }, 'Loading status…'));
    document.body.append(el);
    button.setAttribute('aria-expanded', 'true');
    offFocus = popoverFocus(el, button, close);
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
    if (offFocus) offFocus();
    offFocus = null;
    document.removeEventListener('pointerdown', onDoc, true);
    document.removeEventListener('keydown', onKey);
    if (el) el.remove();
    el = null;
    button.setAttribute('aria-expanded', 'false');
  }

  return { toggle: () => (el ? close() : open()), open: () => { if (!el) open(); }, close };
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

/** The names Claude uses for its usage windows: "Current session" (5 h), "This week"… */
function limitName(minutes, fallback) {
  if (!minutes) return fallback;
  if (minutes === 300) return 'Current session';
  if (minutes === 10080) return 'This week';
  if (minutes === 1440) return 'Today';
  if (minutes % 1440 === 0) return `${minutes / 1440}-day window`;
  if (minutes % 60 === 0) return `${minutes / 60}-hour window`;
  return `${minutes}-minute window`;
}

/** Keeps a <details> open or closed across the panel's refresh (per browser). */
function rememberOpen(details, key, openByDefault) {
  const saved = readJson(STATUS_OPEN_KEY, {}) || {};
  details.open = key in saved ? Boolean(saved[key]) : openByDefault;
  details.addEventListener('toggle', () => {
    const next = readJson(STATUS_OPEN_KEY, {}) || {};
    next[key] = details.open;
    writeJson(STATUS_OPEN_KEY, next);
  });
  return details;
}

/** One machine tile: icon and name, a big value, a bar and a detail line. */
function statTile(iconName, name, value, percent, detail) {
  return h('div', { class: ['stat-tile', `stat-${toneOf(percent)}`] },
    h('div', { class: 'stat-head' }, icon(iconName, { size: 15 }), h('span', null, name)),
    h('div', { class: 'tile-value' }, value),
    usageBar(percent, name),
    h('div', { class: 'stat-detail' }, detail || ' '));
}

/** A collapsible status section whose open state survives the panel's refresh. */
function statusSection(key, iconName, title, summary, body, openByDefault) {
  return rememberOpen(h('details', { class: 'status-section status-fold' },
    h('summary', null, icon('right', { size: 12 }), icon(iconName, { size: 15 }), h('h3', null, title), h('span', { class: 'status-summary-text' }, summary)),
    h('div', { class: 'status-fold-body' }, body)), key, openByDefault);
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
    h('span', { class: ['status-fact', data.contained === false && 'fact-warn'], title: 'Isolation: whether agent sessions run inside an OS sandbox on this server' }, icon('shield', { size: 13 }), data.contained === true ? 'Isolated' : data.contained === false ? 'Not isolated' : 'Isolation unknown'),
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
    statusSection('accounts', 'user', 'AI accounts', accounts.length ? accountSummary : 'none connected',
      accounts.length === 0 ? h('div', { class: 'muted small' }, 'No accounts connected.') : h('div', { class: 'account-list' }, [...troubled, ...accounts.filter((a) => !troubled.includes(a))].map((a) => accountUsage(a, { foldable: true, open: troubled.includes(a) }))), troubled.length > 0 || accounts.length <= 3),
  ];
}

/** Initials for an account's avatar: from the email's name part, else from the label. */
function accountInitials(account) {
  const source = account.email ? account.email.split('@')[0] : (account.label || account.id || '?');
  const parts = String(source).split(/[^A-Za-z0-9]+/).filter(Boolean);
  if (parts.length === 0) return '?';
  return (parts.length > 1 ? parts[0][0] + parts[1][0] : parts[0].slice(0, 2)).toUpperCase();
}

/**
 * One account, shared by the Status panel (foldable: a row that expands) and the Accounts page:
 * an avatar with initials, label, email and plan (when the provider reports them), its state when
 * it is not ready, and each usage window as a horizontal row like Claude's own usage page.
 */
export function accountUsage(account, { foldable = false, open = false } = {}) {
  const usage = account.usage || {};
  const provider = account.provider === 'anthropic' ? 'Claude' : account.provider;
  const state = account.enabled === false
    ? { icon: 'x', tone: 'muted', label: 'Disabled' }
    : ACCOUNT_STATES[account.status] || { icon: 'warn', tone: 'muted', label: account.status || 'Unknown' };
  const usageRow = (w, fallback) => {
    const name = limitName(w && w.window_minutes, fallback);
    const used = w ? w.used_percent : null;
    return h('div', { class: 'usage-row' },
      h('div', { class: 'usage-name' }, h('span', null, name), h('span', { class: 'usage-reset', title: w && w.resets_at ? dateTime(w.resets_at) : '' }, w && w.resets_at ? resetLabel(w.resets_at) : Number.isFinite(used) ? '' : 'Not known yet')),
      usageBar(used, name),
      h('span', { class: ['usage-pct', `tone-${toneOf(used)}`] }, Number.isFinite(used) ? `${Math.round(used)}% used` : '—'));
  };
  const primary = usage.primary;
  const head = [
    h('span', { class: 'avatar account-avatar', 'aria-hidden': 'true' }, accountInitials(account)),
    h('span', { class: 'account-who' },
      h('span', { class: 'account-name' }, h('strong', { class: 'ellipsis' }, account.label || account.id), account.plan ? h('span', { class: 'plan-badge', title: `${provider} plan` }, account.plan) : null),
      h('span', { class: 'account-sub' }, account.email || provider)),
    state.tone !== 'ok' ? h('span', { class: ['account-state', `state-${state.tone}`] }, icon(state.icon, { size: 12 }), state.label) : null,
    foldable ? h('span', { class: 'account-glance', title: 'Current session' }, usageBar(primary ? primary.used_percent : null, 'Current session'), h('span', { class: ['usage-pct', `tone-${toneOf(primary ? primary.used_percent : null)}`] }, primary && Number.isFinite(primary.used_percent) ? `${Math.round(primary.used_percent)}%` : '—')) : null,
    foldable ? icon('down', { size: 14 }) : null,
  ];
  const body = h('div', { class: 'account-body' },
    account.last_error ? h('div', { class: 'account-error' }, icon('warn', { size: 13 }), h('span', null, account.last_error)) : null,
    usageRow(usage.primary, 'Current session'),
    usageRow(usage.secondary, 'Longer window'),
    h('div', { class: 'account-foot' },
      account.in_use_by ? h('span', { class: 'status-count live' }, h('span', { class: 'live-dot' }), `${account.in_use_by} ${account.in_use_by === 1 ? 'task' : 'tasks'} on this account`) : h('span', null, 'No tasks right now'),
      h('span', null, usage.observed_at ? `Updated ${relLong(usage.observed_at)}` : 'Usage not known yet')));
  if (!foldable) return h('div', { class: ['account-card', account.enabled === false && 'disabled'] }, h('div', { class: 'account-head' }, head), body);
  return rememberOpen(h('details', { class: ['account-fold', account.enabled === false && 'disabled'] }, h('summary', { class: 'account-head' }, head), body), `account:${account.id}`, open);
}

// ---------------------------------------------------------------- sidebar

/**
 * Left sidebar: New task, navigation, active task list with status icons (UI-004, UI-005) and
 * the signed-in user at its foot, which opens the user menu.
 */
export function createSidebar(ctx) {
  // The logo slot holds the favicon's Y until Yulius has a real logo.
  const brand = h('a', { class: 'sidebar-brand', href: href('/'), dataset: { route: '/' }, title: BRAND.name },
    h('span', { class: 'brand-logo', 'aria-hidden': 'true' }, icon('logo', { size: 20 })),
    h('span', { class: 'brand-text' }, h('span', { class: 'brand-name' }, BRAND.name), h('span', { class: 'brand-by' }, BRAND.tagline)));
  const shortcut = /Mac|iPhone|iPad/.test(navigator.platform || navigator.userAgent || '') ? '⌘⇧O' : 'Ctrl+Shift+O';
  const newTask = h('a', { class: 'btn btn-newtask', href: '#', dataset: { route: '/' }, title: `New task (${shortcut})`, 'aria-keyshortcuts': shortcut === '⌘⇧O' ? 'Meta+Shift+O' : 'Control+Shift+O' },
    h('span', { class: 'newtask-plus', 'aria-hidden': 'true' }, icon('plus', { size: 14 })), h('span', null, 'New task'), h('span', { class: 'newtask-key', 'aria-hidden': 'true' }, shortcut));
  const nav = h('nav', { class: 'nav', 'aria-label': 'Sandbox' });
  const list = h('div', { class: 'task-list', 'aria-label': 'Active tasks' });
  const avatar = h('span', { class: 'avatar', 'aria-hidden': 'true' }, '?');
  const userName = h('span', { class: 'user-name' });
  const userButton = h('button', { type: 'button', class: 'user-btn', 'aria-label': 'User menu', 'aria-haspopup': 'menu', onClick: () => openUserMenu(ctx, userButton) }, avatar, userName);
  const el = h('aside', { class: 'sidebar', id: 'sidebar' },
    h('div', { class: 'sidebar-full', style: { display: 'flex', flexDirection: 'column', minHeight: 0, flex: 1 } },
      h('div', { class: 'sidebar-top' }, brand, newTask),
      nav,
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
    userButton.setAttribute('aria-label', `${name}, account menu`);
    userButton.title = `${name}${user.login ? ` (@${user.login})` : ''}`;
  }

  function renderNav() {
    const slug = sandbox ? encodeURIComponent(sandbox.slug) : null;
    newTask.dataset.route = slug ? `/s/${slug}` : '/';
    newTask.href = href(newTask.dataset.route);
    newTask.classList.toggle('disabled', !slug);
    // Links are built once and updated in place, so a focused link keeps focus across routes.
    if (!nav.children.length) nav.append(...NAV.map(([, label, iconName]) => h('a', { class: 'nav-item' }, icon(iconName, { size: 16 }), h('span', null, label))));
    NAV.forEach(([page], i) => {
      const link = nav.children[i];
      const path = slug ? `/s/${slug}/${page}` : '/';
      const active = Boolean(route && route.name === 'page' && route.page === page);
      link.href = href(path);
      link.dataset.route = path;
      link.classList.toggle('active', active);
      if (active) link.setAttribute('aria-current', 'page'); else link.removeAttribute('aria-current');
    });
  }

  // Active tasks (UI-005), grouped so what needs the user comes first: "Needs you", "Working",
  // "Stopped" (collapsible). Each row says in words what is happening, not just a mark.
  const GROUPS = [
    { id: 'needs', name: 'Needs you', icon: 'bell' },
    { id: 'working', name: 'Working', icon: 'pulse' },
    { id: 'ended', name: 'Stopped', icon: 'st-stopped' },
  ];
  const ENDED_OPEN_KEY = 'yulius.sidebarStoppedOpen';
  let endedOpen = readJson(ENDED_OPEN_KEY, true) !== false;
  let shape = '';

  const rowInfo = (task) => taskInfo(task, ctx.store.state.actionRequests);

  function subline(task, info) {
    const parts = [info.label];
    const steps = task.steps || {};
    if (info.group === 'working' && task.status === 'running' && steps.total) parts.push(`step ${Math.min((steps.done || 0) + 1, steps.total)}/${steps.total}`);
    parts.push(relShort(task.updated_at));
    return parts.join(' · ');
  }

  function rowFor(task) {
    let row = rows.get(task.id);
    if (!row) {
      const mark = h('span', { class: 'task-mark' });
      const title = h('span', { class: 'task-title' });
      const unread = h('span', { class: 'task-unread' });
      const sub = h('span', { class: 'task-sub' });
      const link = h('a', { class: 'task-link' }, mark, h('span', { class: 'task-text' }, h('span', { class: 'task-line' }, title, unread), sub));
      const kebab = iconButton('more', 'Task actions', (event) => {
        event.preventDefault();
        event.stopPropagation();
        openTaskMenu(ctx, kebab, row.task);
      }, { class: 'task-kebab' });
      const el = h('div', { class: 'task-row', role: 'listitem' }, link, kebab);
      el.addEventListener('contextmenu', (event) => {
        event.preventDefault();
        openTaskMenu(ctx, { x: event.clientX, y: event.clientY }, row.task);
      });
      link.addEventListener('click', () => document.body.classList.remove('drawer-open'));
      row = { el, link, mark, title, unread, sub, task };
      rows.set(task.id, row);
    }
    row.task = task;
    paintRow(row);
    return row;
  }

  function paintRow(row) {
    const task = row.task;
    const info = rowInfo(task);
    row.info = info;
    const path = `/s/${encodeURIComponent(sandbox.slug)}/t/${task.number}`;
    const active = route && route.name === 'task' && route.number === task.number;
    row.link.href = href(path);
    row.link.dataset.route = path;
    row.link.setAttribute('aria-current', active ? 'page' : 'false');
    row.el.classList.toggle('active', Boolean(active));
    row.el.dataset.group = info.group;
    row.link.title = `#${task.number} ${task.title} — ${info.label}${info.detail ? `: ${info.detail}` : ''}${task.branch ? ` · ${task.branch}` : ''} · ${dateTime(task.updated_at)}`;
    setChildren(row.mark, statusMark(info));
    row.title.textContent = task.title || `Task #${task.number}`;
    row.sub.textContent = subline(task, info);
    row.sub.className = `task-sub tone-text-${info.group === 'needs' ? info.tone : 'muted'}`;
    row.unread.hidden = !task.unread_messages;
    row.unread.textContent = task.unread_messages ? String(task.unread_messages) : '';
    row.unread.title = task.unread_messages ? `${task.unread_messages} unread messages` : '';
  }

  function groupHead(group, size) {
    const label = [icon(group.icon, { size: 14 }), h('span', { class: 'task-group-name' }, group.name), h('span', { class: 'task-group-count' }, String(size))];
    if (group.id !== 'ended') return h('div', { class: ['task-group-head', `group-${group.id}`] }, label);
    return h('button', { type: 'button', class: ['task-group-head', 'group-ended'], 'aria-expanded': String(endedOpen), onClick: () => {
      endedOpen = !endedOpen;
      writeJson(ENDED_OPEN_KEY, endedOpen);
      shape = '';
      renderTasks();
    } }, label, icon(endedOpen ? 'down' : 'right', { size: 12 }));
  }

  function renderTasks() {
    if (!sandbox) {
      shape = '';
      setChildren(list, h('div', { class: 'placeholder' }, 'No sandbox selected.'));
      return;
    }
    const all = ctx.store.tasksOf(sandbox.id);
    if (all === null) {
      shape = '';
      setChildren(list, h('div', { class: 'placeholder placeholder-loading' }, 'Loading…'));
      return;
    }
    // A task the user left normally is done for now: it stays under Tasks (with Continue) but
    // leaves the Active list (HRN-002).
    const tasks = all.filter((task) => task.status !== 'closed');
    if (tasks.length === 0) {
      rows.clear();
      shape = '';
      setChildren(list, h('div', { class: 'placeholder small' }, 'No active tasks. Start one with “New task”.'));
      return;
    }
    const seen = new Set();
    const grouped = new Map(GROUPS.map((g) => [g.id, []]));
    for (const task of tasks) {
      seen.add(task.id);
      const row = rowFor(task);
      grouped.get(row.info.group).push(row);
    }
    for (const id of [...rows.keys()]) if (!seen.has(id)) rows.delete(id);
    // Rebuild only when the grouping or order changed, so focus is not lost on every update.
    const next = GROUPS.map((g) => `${g.id}:${g.id === 'ended' && !endedOpen ? 'closed' : ''}:${grouped.get(g.id).map((r) => r.task.id).join(',')}`).join('|');
    if (next === shape) return;
    shape = next;
    setChildren(list, GROUPS.filter((g) => grouped.get(g.id).length).map((g) => h('section', { class: ['task-group', `group-${g.id}`], 'aria-label': g.name },
      groupHead(g, grouped.get(g.id).length),
      g.id === 'ended' && !endedOpen ? null : h('div', { role: 'list' }, grouped.get(g.id).map((r) => r.el)))));
  }

  ctx.store.on('tasks', (sandboxId) => { if (sandbox && sandboxId === sandbox.id) renderTasks(); });
  ctx.store.on('browser-action', (event) => { if (sandbox && event.sandbox_id === sandbox.id) renderTasks(); });
  // Refresh relative times twice a minute (single bounded timer).
  setInterval(() => {
    for (const row of rows.values()) row.sub.textContent = subline(row.task, row.info);
  }, 30000);

  return {
    el,
    update(nextRoute, nextSandbox) {
      if (!nextSandbox || !sandbox || nextSandbox.id !== sandbox.id) { rows.clear(); shape = ''; }
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
