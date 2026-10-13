// Global layout pieces (SPEC-001 §6.1): thin sandbox tab row with the Status panel, and the
// left sidebar with navigation, the active task list and the user menu at its foot.

import { h, icon, iconButton, openMenu, setChildren, statusMark, popoverFocus, keepFocus, loadError, coalesce, pollPaused, prChip } from './dom.js';
import { relShort, relLong, bytes, duration, dateTime, resetLabel } from './format.js';
import { taskInfo, taskActions, activityLine, allows } from './status.js';
import { href } from './router.js';
import { openSandboxDialog, confirmRemoveSandbox, openTaskMenu, archiveTask, archiveAll } from './actions.js';
import { createNotificationCenter } from './notifications.js';
import { BRAND } from './brand.js';
import { readJson, writeJson } from './store.js';
import { NEW_TASK_KEYS, NEW_TASK_ARIA, HOME_KEYS, HOME_ARIA } from './shortcuts.js';
import { createSplitter, layoutPrefs } from './splitter.js';
import { isPhone } from './theme.js';

/** Sidebar widths (UI-044): its range, and what it leaves the panel and the centre (as panels.js). */
const SIDEBAR = { min: 200, max: 480, panelMin: 300, centerMin: 360 };

// The sandbox pages used every day (SPEC-029); a CD page would join CI here.
const NAV = [
  ['tasks', 'Tasks', 'tasks'],
  ['git', 'Git', 'branch'],
  ['ci', 'CI (tests)', 'check'],
  ['secrets', 'Secrets', 'lock'],
  ['dashboard', 'Dashboard', 'chart'],
];
// The rest, under "More": collapsed by default, remembered per browser (SPEC-029).
const NAV_MORE = [
  ['sessions', 'Sessions', 'chat'],
  ['workflows', 'Workflows', 'workflow'],
  ['skills', 'Skills', 'book'],
  ['automations', 'Automations', 'zap'],
  ['mcp', 'MCP', 'plug'],
];
const MORE_OPEN_KEY = 'yulius.sidebarMoreOpen';

// ---------------------------------------------------------------- tab row

/** Thin sandbox tab row: tabs (or a dropdown on phones), "+", Status, notifications, Agent, tools toggle. */
export function createTabRow(ctx) {
  const tabs = h('nav', { class: 'tabrow-tabs', 'aria-label': 'Sandboxes' });
  const select = h('select', { class: 'select tab-select', 'aria-label': 'Sandbox', onChange: () => {
    if (select.value === '__add') {
      select.value = currentSlug || (home ? '__home' : '');
      openSandboxDialog(ctx);
    } else if (select.value === '__home') {
      ctx.navigate('/');
    } else if (select.value) {
      ctx.navigate(`/s/${encodeURIComponent(select.value)}`);
    }
  } });
  const hamburger = iconButton('menu', 'Open navigation', () => document.body.classList.toggle('drawer-open'), { class: 'hamburger', 'aria-controls': 'sidebar', 'aria-expanded': 'false' });
  const addButton = iconButton('plus', 'Add sandbox: create or import a repository', () => openSandboxDialog(ctx), { class: 'tab-add' });
  // One name for one fact (#147): sessions are "isolated" in an OS sandbox, or "not isolated".
  const contained = h('span', { class: 'chip chip-warn', hidden: true, title: 'Not isolated: agent sessions on this server run without an OS sandbox, so an agent can reach files outside its sandbox.' }, icon('warn', { size: 12 }), h('span', { class: 'chip-text' }, 'Not isolated'));
  const eventDot = h('span', { class: 'ev-dot', title: 'Live updates connected' });
  const eventText = h('span', { class: 'sr-only' }, ', live updates connected');
  const statusButton = h('button', { type: 'button', class: 'btn btn-status', 'aria-expanded': 'false', 'aria-haspopup': 'dialog', title: 'Status: machine and subscription usage' }, eventDot, h('span', { class: 'status-text' }, 'Status'), eventText, icon('down', { size: 12 }));
  // Shows/hides the right tool rail (browser, editor, shell); hidden by default.
  const toolsButton = iconButton('tools', 'Show tools (browser, editor, shell)', () => ctx.toggleRail && ctx.toggleRail(), { class: 'tools-toggle' });
  const toolsDot = h('span', { class: 'rail-dot', hidden: true });
  toolsButton.append(toolsDot);
  toolsButton.style.position = 'relative';
  // The button's name carries the attention dot too ("…, an agent needs your action").
  let toolsAttention = false;
  let railVisible = false;
  const labelTools = () => {
    toolsButton.title = railVisible ? 'Hide tools' : 'Show tools (browser, editor, shell)';
    toolsButton.setAttribute('aria-label', `${toolsButton.title}${toolsAttention ? ', an agent needs your action' : ''}`);
  };
  ctx.onToolsAttention = (attention) => { toolsAttention = attention; toolsDot.hidden = !attention; labelTools(); };
  ctx.onRailChange = (visible) => {
    railVisible = visible;
    toolsButton.classList.toggle('active', visible);
    toolsButton.setAttribute('aria-pressed', String(visible));
    labelTools();
  };
  // The Agent (SPEC-035 AGT-005): the user's own session for quick work, in the right panel; one
  // click opens or closes it, whether the tools rail shows or not.
  const agentButton = h('button', { type: 'button', class: 'btn btn-agent agent-toggle', 'aria-label': 'Show Agent', 'aria-pressed': 'false', title: 'Agent: your own Claude Code session for quick work', onClick: () => ctx.toggleAgent && ctx.toggleAgent() },
    icon('spark', { size: 16 }), h('span', { class: 'agent-toggle-text', 'aria-hidden': 'true' }, 'Agent'));
  ctx.onPanelChange = (kind) => {
    const open = kind === 'agent';
    agentButton.classList.toggle('active', open);
    agentButton.setAttribute('aria-pressed', String(open));
    agentButton.setAttribute('aria-label', open ? 'Hide Agent' : 'Show Agent');
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
    h('div', { class: 'tabrow-right' }, contained, statusButton, notifications.button, agentButton, toolsButton),
  );

  let currentSlug = null;
  let home = false;
  let dragId = null;

  function renderTabs() {
    const sandboxes = ctx.store.state.sandboxes;
    // Home first (UI-045): every sandbox at a glance, also reached with the logo and ⌘⇧H.
    const homeTab = h('a', {
      class: ['tab', 'tab-home', home && 'active'],
      href: href('/'),
      dataset: { route: '/' },
      title: `All sandboxes (${HOME_KEYS})`,
      'aria-label': 'All sandboxes',
      'aria-keyshortcuts': HOME_ARIA,
      'aria-current': home ? 'page' : null,
    }, icon('grid', { size: 16 }));
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
      h('span', { class: 'sr-only' }, [counts.running ? `, ${counts.running} working` : '', counts.needs_attention ? `, ${counts.needs_attention} need${counts.needs_attention === 1 ? 's' : ''} you` : ''].join('')),
      counts.running ? h('span', { class: 'tab-running', title: `${counts.running} working`, 'aria-hidden': 'true' }, String(counts.running)) : null,
      counts.needs_attention ? h('span', { class: 'tab-dot', title: `${counts.needs_attention} need${counts.needs_attention === 1 ? 's' : ''} you` }) : null);
      const index = sandboxes.indexOf(sandbox);
      const move = (step) => {
        const ids = sandboxes.map((sb) => sb.id);
        const [moved] = ids.splice(index, 1);
        ids.splice(index + step, 0, moved);
        ctx.store.setSandboxOrder(ids);
      };
      a.addEventListener('contextmenu', (event) => {
        event.preventDefault();
        openMenu({ x: event.clientX, y: event.clientY }, [
          { label: 'Open', onSelect: () => ctx.navigate(`/s/${encodeURIComponent(sandbox.slug)}`) },
          sandbox.repo && sandbox.repo.url ? { label: 'Open on GitHub', onSelect: () => window.open(sandbox.repo.url, '_blank', 'noopener,noreferrer') } : null,
          { separator: true },
          // Reordering without dragging (WCAG 2.5.7): the menu moves a tab one place.
          { label: 'Move left', icon: 'left', disabled: index === 0, onSelect: () => move(-1) },
          { label: 'Move right', icon: 'right', disabled: index === sandboxes.length - 1, onSelect: () => move(1) },
          { separator: true },
          { label: `Remove from ${BRAND.name}…`, danger: true, disabled: !allows(sandbox.access, 'admin'), title: allows(sandbox.access, 'admin') ? null : 'Requires repository admin', onSelect: () => confirmRemoveSandbox(ctx, sandbox) },
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
    keepFocus(tabs, () => setChildren(tabs, homeTab, anchors)); // counts refresh often; focus stays
    const active = tabs.querySelector('.tab.active');
    if (active) active.scrollIntoView({ block: 'nearest', inline: 'nearest' });

    setChildren(select,
      h('option', { value: '__home' }, 'All sandboxes'),
      currentSlug || home ? null : h('option', { value: '' }, 'Choose sandbox…'),
      sandboxes.map((s) => h('option', { value: s.slug }, `${s.name}${s.counts && s.counts.needs_attention ? ' (!)' : ''}`)),
      h('option', { value: '__add' }, '+ Add sandbox…'));
    select.value = currentSlug || (home ? '__home' : '');
  }

  function renderMe() {
    const me = ctx.store.state.me;
    if (!me) return;
    contained.hidden = !(me.instance && me.instance.contained === false);
  }

  const offs = [
    ctx.store.on('sandboxes', renderTabs),
    ctx.store.on('events-status', (status) => {
      const ok = status === 'open';
      eventDot.classList.toggle('off', !ok);
      eventDot.title = ok ? 'Live updates connected' : 'Live updates reconnecting…';
      eventText.textContent = ok ? ', live updates connected' : ', live updates reconnecting'; // not colour alone (WCAG 1.4.1)
    }),
  ];

  return {
    el,
    /** Sign-out: unsubscribe and close the Status panel. */
    destroy() { offs.forEach((off) => off()); statusPanel.close(); },
    /** The sandbox in view (its tab is current), or the home screen (`{home: true}`). */
    update(slug, { home: onHome = false } = {}) {
      currentSlug = slug;
      home = onHome;
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
  // Outside a sandbox (Home, Settings…) the per-sandbox entries use the sandbox used last.
  const sandbox = ctx.currentSandbox() || (ctx.defaultSandbox ? ctx.defaultSandbox() : null);
  const items = [
    { header: `${user.display_name || user.login || 'Signed in'}${user.login ? ` · @${user.login}` : ''}${superadmin ? ' · superadmin' : ''}` },
    { label: 'Archived tasks', icon: 'archive', onSelect: () => ctx.navigate('/archived') },
    { label: 'CLI device approval', icon: 'key', onSelect: () => ctx.navigate('/device') },
    { label: 'Team secrets', icon: 'lock', onSelect: () => ctx.navigate(sandbox ? `/s/${encodeURIComponent(sandbox.slug)}/secrets` : '/secrets') },
  ];
  if (instanceAdmin) {
    items.push({ separator: true }, { header: 'Administration' });
    items.push({ label: 'Users', icon: 'users', onSelect: () => ctx.navigate('/admin/users') });
    items.push({ label: 'Subscriptions', icon: 'user', onSelect: () => ctx.navigate('/admin/accounts') });
    if (superadmin) items.push({ label: 'GitHub connection', icon: 'github', onSelect: () => ctx.navigate('/admin/github') });
    items.push(
      // One name per scope (#147): "Every sandbox" (the instance) and "This sandbox".
      { label: 'MCP for every sandbox', icon: 'plug', disabled: !sandbox, title: sandbox ? null : 'Open a sandbox first', onSelect: () => ctx.navigate(`/s/${encodeURIComponent(sandbox.slug)}/mcp?scope=instance`) },
      { label: 'Skills for every sandbox', icon: 'book', disabled: !sandbox, title: sandbox ? null : 'Open a sandbox first', onSelect: () => ctx.navigate(`/s/${encodeURIComponent(sandbox.slug)}/skills?source=instance`) },
    );
  }
  items.push({ separator: true }, { label: 'Settings', icon: 'gear', onSelect: () => ctx.navigate('/settings') }, { label: 'Sign out', icon: 'logout', onSelect: () => ctx.signOut() });
  openMenu(anchor, items);
}

// ---------------------------------------------------------------- status panel

/** Hidden-by-default Status panel with VM metrics and subscription usage (UI-007, AC-STA-1). */
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
    timer = setInterval(() => { if (!pollPaused()) load(); }, 10000);
    onDoc = (event) => { if (el && !el.contains(event.target) && !button.contains(event.target)) close(); };
    onKey = (event) => { if (event.key === 'Escape') { event.preventDefault(); event.stopPropagation(); close(); button.focus(); } };
    setTimeout(() => document.addEventListener('pointerdown', onDoc, true), 0);
    document.addEventListener('keydown', onKey, true); // before the drawer's Escape: innermost layer first
  }

  function close() {
    clearInterval(timer);
    timer = null;
    if (offFocus) offFocus();
    offFocus = null;
    document.removeEventListener('pointerdown', onDoc, true);
    document.removeEventListener('keydown', onKey, true);
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
  // At its limit until a window resets: the pool runs tasks on the others meanwhile, so it needs
  // nobody (F 2026-10-10).
  limited: { icon: 'clock', tone: 'muted', label: 'Limit reached' },
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
    h('div', { class: 'meter-track', role: 'meter', 'aria-valuemin': 0, 'aria-valuemax': 100, 'aria-valuenow': known ? value : 0, 'aria-valuetext': known ? `${Math.round(value)}% used` : 'unknown', 'aria-label': label },
      h('div', { class: 'meter-fill', style: { width: `${value}%` } })));
}

const percentText = (percent) => (Number.isFinite(percent) ? `${Math.round(percent)}%` : '—');

/**
 * Usage windows by their length: "5-hour window", "This week"… Claude calls the 5-hour one
 * "Current session"; here a session is a Claude Code run, so the window is named by its length (#147).
 */
function limitName(minutes, fallback) {
  if (!minutes) return fallback;
  if (minutes === 300) return '5-hour window';
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
    h('summary', null, icon('right', { size: 12 }), icon(iconName, { size: 15 }), h('h2', null, title), h('span', { class: 'status-summary-text' }, summary)),
    h('div', { class: 'status-fold-body' }, body)), key, openByDefault);
}

/**
 * Render GET /api/status (`StatusView`: vm, sessions, sandboxes, accounts, contained) so it reads
 * in seconds: one health line, machine tiles, then sandboxes and accounts as collapsible lists.
 * VM fields are null when unknown (CPU needs two readings) and render as "—"/"unknown", never as
 * zero. Account id/last_error/in_use_by are present for admins only.
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

  // One line that says whether anything needs a person; those accounts are listed first. A
  // subscription at its limit needs nobody: the pool uses the others until it resets.
  const troubled = accounts.filter((a) => ['relogin', 'pending_login', 'error'].includes(a.status));
  const hot = [vm.cpu_percent, memPct, diskPct].some((v) => Number.isFinite(v) && v >= 90);
  let health = { tone: 'ok', icon: 'check', text: 'Everything is running normally' };
  if (data.contained === false) health = { tone: 'warn', icon: 'shield', text: 'Not isolated: agent sessions run without an OS sandbox' };
  if (hot) health = { tone: 'warn', icon: 'warn', text: 'The machine is under heavy load' };
  // Every subscription at its limit: tasks wait for the first window reset (ROT-010).
  if (accounts.length && accounts.every((a) => a.status === 'limited')) health = { tone: 'warn', icon: 'clock', text: 'No subscription has capacity left: tasks continue when a window resets' };
  if (troubled.length) health = { tone: 'warn', icon: 'warn', text: `${troubled.length} ${troubled.length === 1 ? 'subscription needs' : 'subscriptions need'} attention` };

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

  const ready = accounts.filter((a) => a.status === 'ok').length;
  const limited = accounts.filter((a) => a.status === 'limited').length;
  const accountSummary = [`${ready} ready`, limited ? `${limited} at the limit` : null, troubled.length ? `${troubled.length} need attention` : null].filter(Boolean).join(' · ');

  return [
    h('div', { class: ['status-health', `health-${health.tone}`] }, icon(health.icon, { size: 16 }), h('span', null, health.text)),
    h('div', { class: 'status-section' },
      h('h2', null, 'Machine'),
      h('div', { class: 'stat-tiles' },
        statTile('cpu', 'CPU', percentText(vm.cpu_percent), vm.cpu_percent, vm.cpu_count ? `${vm.cpu_count} cores` : null),
        statTile('memory', 'Memory', percentText(memPct), memPct, vm.memory_total_bytes ? `${bytes(vm.memory_used_bytes)} of ${bytes(vm.memory_total_bytes)}` : null),
        statTile('disk', 'Disk', diskPct === null ? '—' : percentText(diskPct), diskPct, vm.disk_total_bytes ? `${bytes(vm.disk_used_bytes)} of ${bytes(vm.disk_total_bytes)}` : null),
        statTile('terminal', 'Sessions', Number.isFinite(sessions.live) ? `${sessions.live}${sessions.limit ? ` / ${sessions.limit}` : ''}` : '—', sessionsPct, sessions.queued ? `${sessions.queued} queued` : 'agent sessions running')),
      h('div', { class: 'status-facts' }, facts)),
    sandboxes.length ? statusSection('sandboxes', 'folder', 'Sandboxes', `${liveTotal} running in ${busy} of ${sandboxes.length}`, h('div', { class: 'status-rows' }, sandboxRows), false) : null,
    statusSection('accounts', 'user', 'Subscriptions', accounts.length ? accountSummary : 'none yet',
      accounts.length === 0 ? h('div', { class: 'muted small' }, 'No subscriptions yet.') : h('div', { class: 'account-list' }, [...troubled, ...accounts.filter((a) => !troubled.includes(a))].map((a) => accountUsage(a, { foldable: true, open: troubled.includes(a) }))), troubled.length > 0 || accounts.length <= 3),
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
  const state = ACCOUNT_STATES[account.status] || { icon: 'warn', tone: 'muted', label: account.status || 'Unknown' };
  const usageRow = (w, fallback) => {
    const name = limitName(w && w.window_minutes, fallback);
    const used = w ? w.used_percent : null;
    return h('div', { class: 'usage-row' },
      h('div', { class: 'usage-name' }, h('span', null, name), h('span', { class: 'usage-reset', title: w && w.resets_at ? dateTime(w.resets_at) : '' }, w && w.resets_at ? resetLabel(w.resets_at) : Number.isFinite(used) ? '' : 'Reading usage…')),
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
    foldable ? h('span', { class: 'account-glance', title: '5-hour window' }, usageBar(primary ? primary.used_percent : null, '5-hour window'), h('span', { class: ['usage-pct', `tone-${toneOf(primary ? primary.used_percent : null)}`] }, primary && Number.isFinite(primary.used_percent) ? `${Math.round(primary.used_percent)}%` : '—')) : null,
    foldable ? icon('down', { size: 14 }) : null,
  ];
  // A usage reading that failed for now ("usage: …", one plain sentence) is a quiet note, not a
  // broken account: tasks still run on it and the last known usage stays (ROT-009, #27). Usage
  // comes from the subscription's running tasks (ADR-17), so an idle one simply has none yet.
  const usageNote = Boolean(account.last_error) && account.last_error.startsWith('usage:') && !['relogin', 'error'].includes(account.status);
  const body = h('div', { class: 'account-body' },
    account.last_error && usageNote ? h('div', { class: 'account-note' }, icon('clock', { size: 13 }), h('span', null, account.last_error.slice('usage:'.length).trim())) : null,
    account.last_error && !usageNote ? h('div', { class: 'account-error' }, icon('warn', { size: 13 }), h('span', null, account.last_error)) : null,
    usageRow(usage.primary, '5-hour window'),
    usageRow(usage.secondary, 'This week'),
    // Per-model weekly caps, when the plan has them (they bind only tasks of that model).
    usage.weekly_opus ? usageRow({ ...usage.weekly_opus, window_minutes: null }, 'Opus this week') : null,
    usage.weekly_sonnet ? usageRow({ ...usage.weekly_sonnet, window_minutes: null }, 'Sonnet this week') : null,
    h('div', { class: 'account-foot' },
      account.in_use_by ? h('span', { class: 'status-count live' }, h('span', { class: 'live-dot' }), `${account.in_use_by} ${account.in_use_by === 1 ? 'task' : 'tasks'} on this subscription`) : h('span', null, 'No tasks right now'),
      h('span', null, usage.observed_at ? `Updated ${relLong(usage.observed_at)}` : 'Usage is read every few minutes')));
  if (!foldable) return h('div', { class: 'account-card' }, h('div', { class: 'account-head' }, head), body);
  return rememberOpen(h('details', { class: 'account-fold' }, h('summary', { class: 'account-head' }, head), body), `account:${account.id}`, open);
}

/** A number tile of the capacity panel: name, value and a detail line (no meter). */
function capacityTile(iconName, name, value, detail, tone = 'ok') {
  return h('div', { class: ['stat-tile', `stat-${tone}`] },
    h('div', { class: 'stat-head' }, icon(iconName, { size: 15 }), h('span', null, name)),
    h('div', { class: 'tile-value' }, value),
    h('div', { class: 'stat-detail' }, detail || ' '));
}

/** Subscriptions' weekly limits used per week at `burn` points per hour (ADR-17 §2.4). */
function perWeek(burn) {
  return Number.isFinite(burn) ? (burn * 168) / 100 : null;
}

/**
 * The subscription pool from GET /api/capacity (ROT-013): one recommendation line and the
 * numbers behind it — available now, what is left this week, the burn rate, when the pool runs
 * out at that rate and the hours tasks waited. Shared by the Accounts page and the Dashboard;
 * unknown values read as "—" or "not enough readings yet", never as zero.
 */
export function capacityPanel(view) {
  const v = view || {};
  const subscriptions = Number.isFinite(v.subscriptions) ? v.subscriptions : 0;
  const known = subscriptions - (v.unknown_usage || 0);
  const threshold = Number.isFinite(v.threshold) ? v.threshold : 98;
  const share = (left) => (Number.isFinite(left) && known > 0 ? Math.max(0, Math.min(100, (left / (threshold * known)) * 100)) : null);
  const weekly = share(v.weekly_left);
  const session = share(v.five_hour_left);
  const burn24 = perWeek(v.burn_24h);
  const burn7 = perWeek(v.burn_7d);
  const add = v.add || 0;
  const paused = v.paused_tasks || 0;
  const reset = v.resumes_at ? resetLabel(v.resumes_at) : '';
  const line = [v.recommendation || 'Capacity not known yet', paused ? `${paused} ${paused === 1 ? 'task' : 'tasks'} paused${reset ? ` · ${reset.toLowerCase()}` : ''}` : null].filter(Boolean).join(' · ');
  const leftTone = (p) => (!Number.isFinite(p) ? 'unknown' : p <= 10 ? 'danger' : p <= 30 ? 'warn' : 'ok');
  const hours = Number.isFinite(v.paused_hours_7d) ? v.paused_hours_7d : 0;
  return h('div', { class: 'stack capacity-panel' },
    h('div', { class: ['status-health', `health-${add > 0 ? 'warn' : 'ok'}`], role: 'status' }, icon(add > 0 ? 'warn' : 'check', { size: 16 }), h('span', null, line)),
    h('div', { class: 'stat-tiles' },
      capacityTile('user', 'Available now', `${Number.isFinite(v.available) ? v.available : 0} of ${subscriptions}`, `below ${threshold}% in every window`, v.available ? 'ok' : 'danger'),
      capacityTile('clock', 'Left this week', Number.isFinite(weekly) ? `${Math.round(weekly)}%` : '—', Number.isFinite(session) ? `5-hour windows: ${Math.round(session)}% left` : 'of the pool before it pauses', leftTone(weekly)),
      capacityTile('zap', 'Burn rate', burn24 === null ? '—' : `${burn24.toFixed(1)} / week`, burn24 === null ? 'not enough readings yet' : `subscriptions' weekly limits, last 24 h${burn7 === null ? '' : ` (7 d: ${burn7.toFixed(1)})`}`, 'ok'),
      capacityTile('chart', 'Runs out', v.runs_out_at ? resetLabel(v.runs_out_at).replace(/^Resets (at )?/, '') : burn24 === null ? '—' : 'Not this week', v.runs_out_at ? 'at the current rate' : 'at the current rate, before the next resets', v.runs_out_at ? 'warn' : 'ok'),
      capacityTile('pulse', 'Paused (7 days)', `${hours.toFixed(hours >= 10 ? 0 : 1)} h`, Number.isFinite(v.active_hours_7d) ? `task-hours; ${Math.round(v.active_hours_7d)} h worked` : 'task-hours', hours > 0 ? 'warn' : 'ok')));
}

// ---------------------------------------------------------------- sidebar

/**
 * Left sidebar: New task, navigation, active task list with status icons (UI-004, UI-005) and
 * the signed-in user at its foot, which opens the user menu.
 */
export function createSidebar(ctx) {
  // The logo goes home: every sandbox at a glance (UI-045).
  const brand = h('a', { class: 'sidebar-brand', href: href('/'), dataset: { route: '/' }, title: `All sandboxes (${HOME_KEYS})`, 'aria-keyshortcuts': HOME_ARIA },
    h('span', { class: 'brand-logo', 'aria-hidden': 'true' }, icon('logo', { size: 34 })),
    h('span', { class: 'brand-text' }, h('span', { class: 'brand-name' }, BRAND.name), h('span', { class: 'brand-by' }, BRAND.tagline)),
    h('span', { class: 'sr-only' }, ', all sandboxes'));
  const shortcut = NEW_TASK_KEYS;
  const newTask = h('a', { class: 'btn btn-newtask', href: '#', dataset: { route: '/' }, title: `New task (${shortcut})`, 'aria-keyshortcuts': NEW_TASK_ARIA },
    h('span', { class: 'newtask-plus', 'aria-hidden': 'true' }, icon('plus', { size: 16 })), h('span', null, 'New task'), h('span', { class: 'newtask-key', 'aria-hidden': 'true' }, shortcut));
  const nav = h('nav', { class: 'nav', 'aria-label': 'Sandbox' });
  // Links are built once and updated in place, so a focused link keeps focus across routes.
  const navLink = ([page, label, iconName]) => ({ page, el: h('a', { class: 'nav-item' }, h('span', { class: 'nav-slot' }, icon(iconName, { size: 20 })), h('span', null, label)) });
  const primaryLinks = NAV.map(navLink);
  const moreLinks = NAV_MORE.map(navLink);
  const moreGroup = h('div', { class: 'nav-more', id: 'nav-more' }, moreLinks.map((link) => link.el));
  const moreChevron = h('span', { class: 'nav-more-chevron' });
  let moreOpen = readJson(MORE_OPEN_KEY, false) === true; // the user's choice, kept
  let moreShown = false; // opened for the More page in view only, never kept
  let morePage = null; // the More page last shown, so it opens the group once per visit
  const moreToggle = h('button', { type: 'button', class: 'nav-item nav-toggle', 'aria-controls': 'nav-more', onClick: () => {
    moreOpen = !(moreOpen || moreShown);
    moreShown = false;
    writeJson(MORE_OPEN_KEY, moreOpen);
    paintMore();
  } }, h('span', { class: 'nav-slot' }, icon('more', { size: 20 })), h('span', null, 'More'), moreChevron);
  nav.append(...primaryLinks.map((link) => link.el), moreToggle, moreGroup);
  function paintMore() {
    const open = moreOpen || moreShown;
    // Focus in a group about to hide goes to More rather than to the top of the page.
    if (!open && moreGroup.contains(document.activeElement)) moreToggle.focus();
    moreGroup.hidden = !open;
    // Closed on one of its pages, More marks where the current page is, also for screen readers.
    const holdsCurrent = !open && Boolean(morePage);
    moreToggle.classList.toggle('active', holdsCurrent);
    if (holdsCurrent) moreToggle.setAttribute('aria-current', 'true'); else moreToggle.removeAttribute('aria-current');
    moreToggle.setAttribute('aria-expanded', String(open));
    setChildren(moreChevron, icon(open ? 'down' : 'right', { size: 12 }));
  }
  paintMore();
  const list = h('div', { class: 'task-list', 'aria-label': 'Active tasks' });
  const avatar = h('span', { class: 'avatar', 'aria-hidden': 'true' }, '?');
  const userName = h('span', { class: 'user-name' });
  const userButton = h('button', { type: 'button', class: 'user-btn', 'aria-label': 'User menu', 'aria-haspopup': 'menu', onClick: () => openUserMenu(ctx, userButton) }, avatar, userName);
  const el = h('aside', { class: 'sidebar', id: 'sidebar' },
    h('div', { class: 'sidebar-full' },
      h('div', { class: 'sidebar-top' }, brand, newTask),
      nav,
      list),
    h('div', { class: 'sidebar-foot' }, userButton),
  );

  // ---- width (UI-044): dragged on its right edge, kept per user, never so wide that the rail or the
  // centre (360 px) leaves the window. With a tool panel open the panel keeps its 300 px minimum, so
  // the sidebar gives way first; both grow back when the window widens (panels.js refit).
  const prefs = layoutPrefs(ctx);
  let wanted = prefs.get('sidebar'); // px, or null: the stylesheet's default (300 px, 200 px below 1024 px)
  const app = () => el.parentElement;
  const cssDefault = () => parseFloat(getComputedStyle(document.documentElement).getPropertyValue('--sidebar-w')) || 300;
  function range() {
    const shell = app();
    let other = 0;
    if (shell && !shell.classList.contains('no-sandbox')) {
      const rail = shell.querySelector('.rail');
      const panel = shell.querySelector('.panel');
      other += rail && !rail.hidden ? rail.getBoundingClientRect().width : 0;
      if (panel && !panel.hidden && !shell.classList.contains('panel-max')) other += SIDEBAR.panelMin;
    }
    return { min: SIDEBAR.min, max: Math.max(SIDEBAR.min, Math.min(SIDEBAR.max, window.innerWidth - other - SIDEBAR.centerMin)) };
  }
  /** Applies the wanted width within the room there is; on phones the sidebar is a drawer instead. */
  function fit() {
    const shell = app();
    if (!shell) return;
    if (isPhone()) { shell.style.removeProperty('--sidebar-w'); return; }
    const { min, max } = range();
    const base = wanted || cssDefault();
    const width = Math.round(Math.max(min, Math.min(max, base)));
    if (!wanted && width === Math.round(base)) shell.style.removeProperty('--sidebar-w');
    else shell.style.setProperty('--sidebar-w', `${width}px`);
    splitter.sync();
    if (ctx.refitPanel) ctx.refitPanel();
  }
  const splitter = createSplitter({
    label: 'Resize sidebar',
    className: 'splitter-side',
    width: () => el.getBoundingClientRect().width,
    resize: (px) => { const { min, max } = range(); wanted = Math.round(Math.max(min, Math.min(max, px))); fit(); },
    commit: () => { if (wanted) prefs.set('sidebar', wanted); },
    reset: () => { wanted = null; prefs.clear('sidebar'); fit(); },
    range,
  });
  el.append(splitter.el);
  ctx.fitSidebar = fit;
  window.addEventListener('resize', fit);

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
    // On the home screen New task starts in the sandbox used last (UI-045).
    const target = sandbox || (ctx.defaultSandbox ? ctx.defaultSandbox() : null);
    newTask.dataset.route = target ? `/s/${encodeURIComponent(target.slug)}` : '/';
    newTask.href = href(newTask.dataset.route);
    newTask.classList.toggle('disabled', !target);
    // Read access: the composer only explains; say so here as Home and the Tasks page do.
    const readOnly = Boolean(target && target.access && !allows(target.access, 'write'));
    if (readOnly) newTask.setAttribute('aria-disabled', 'true'); else newTask.removeAttribute('aria-disabled');
    newTask.title = readOnly ? 'Requires write access' : `New task (${shortcut})`;
    const routed = Boolean(route && ['page', 'gitTask', 'taskInfo', 'skill', 'workflow', 'automation', 'mcpServer', 'session'].includes(route.name));
    for (const { page, el: link } of [...primaryLinks, ...moreLinks]) {
      const path = slug ? `/s/${slug}/${page}` : '/';
      const active = routed && route.page === page;
      link.href = href(path);
      link.dataset.route = path;
      link.classList.toggle('active', active);
      if (active) link.setAttribute('aria-current', 'page'); else link.removeAttribute('aria-current');
    }
    // A page under More opens the group while it is in view, so the current page is not hidden;
    // the user's own choice stays as it was (SPEC-029 NAV-004).
    const current = routed && moreLinks.some((link) => link.page === route.page) ? route.page : null;
    if (current !== morePage) {
      morePage = current;
      moreShown = Boolean(current);
      paintMore();
    }
  }

  // Active tasks (UI-005), grouped so what needs the user comes first: "Needs you", "Working",
  // "Stopped" (collapsible). Each row says in words what is happening, not just a mark.
  const GROUPS = [
    { id: 'needs', name: 'Needs you', icon: 'bell' },
    { id: 'working', name: 'Working', icon: 'pulse' },
    // Finished with a tested pull request (#35).
    { id: 'done', name: 'Done', icon: 'check' },
    { id: 'ended', name: 'Stopped', icon: 'st-stopped' },
  ];
  const ENDED_OPEN_KEY = 'yulius.sidebarStoppedOpen';
  let endedOpen = readJson(ENDED_OPEN_KEY, true) !== false;
  let shape = '';
  let archivingAll = false; // "Archive all" runs: its button stays disabled until it ends

  const rowInfo = (task) => taskInfo(task, ctx.store.state.actionRequests, ctx.store.pullsOf(task.sandbox_id).get(task.id));

  function rowFor(task) {
    let row = rows.get(task.id);
    if (!row) {
      // One line: state icon, pull request, title, age. What the task needs is said by
      // the icon, the tooltip and (for screen readers) the hidden sentence after the title.
      const mark = h('span', { class: 'task-mark' });
      const chips = h('span', { class: 'task-chips' });
      const title = h('span', { class: 'task-title' });
      const sub = h('span', { class: 'sr-only' });
      const age = h('span', { class: 'task-age' });
      const link = h('a', { class: 'task-link' }, mark, chips, title, sub, age);
      const kebab = iconButton('more', 'Task actions', (event) => {
        event.preventDefault();
        event.stopPropagation();
        openTaskMenu(ctx, kebab, row.task);
      }, { class: 'task-kebab', 'aria-haspopup': 'menu' });
      // One click archives a task that can be (SPEC-062 QAR-001), as the menu's Archive does.
      const archive = iconButton('archive', 'Archive task', () => archiveTask(ctx, row.task), { class: 'task-quick-archive' });
      const el = h('div', { class: 'task-row', role: 'listitem' }, link, archive, kebab);
      el.addEventListener('contextmenu', (event) => {
        event.preventDefault();
        openTaskMenu(ctx, { x: event.clientX, y: event.clientY }, row.task);
      });
      link.addEventListener('click', () => document.body.classList.remove('drawer-open'));
      row = { el, link, mark, chips, title, age, sub, archive, task };
      rows.set(task.id, row);
    }
    row.task = task;
    row.info = rowInfo(task);
    // Repaint only a row whose content would change (PERF-02): the key holds all it shows.
    const active = Boolean(route && route.name === 'task' && route.number === task.number);
    row.pr = ctx.store.pullsOf(sandbox.id).get(task.id) || null;
    const key = JSON.stringify([task, row.info, active, relShort(task.updated_at), sandbox.slug, sandbox.access, row.pr]);
    if (row.key !== key) {
      row.key = key;
      paintRow(row, active);
    }
    return row;
  }

  function paintRow(row, active) {
    const task = row.task;
    const info = row.info;
    const path = `/s/${encodeURIComponent(sandbox.slug)}/t/${task.number}`;
    row.link.href = href(path);
    row.link.dataset.route = path;
    row.link.setAttribute('aria-current', active ? 'page' : 'false');
    row.el.classList.toggle('active', active);
    row.el.dataset.group = info.group;
    row.el.dataset.tone = info.tone;
    row.link.title = `#${task.number} ${task.title} — ${info.label}${info.detail ? `: ${info.detail}` : ''}${task.branch ? ` · ${task.branch}` : ''} · ${dateTime(task.updated_at)}`;
    setChildren(row.mark, statusMark(info, 16));
    setChildren(row.chips, row.pr ? prChip(row.pr, { link: false }) : null);
    row.title.textContent = task.title || `Task #${task.number}`;
    // Colour stays on the state icon; the words (tooltip, screen readers) say what to do.
    row.sub.textContent = `, ${activityLine(task, info)}`;
    row.age.textContent = relShort(task.updated_at);
    row.age.title = `Last change ${dateTime(task.updated_at)}`;
    const label = `Archive task #${task.number}`;
    row.archive.hidden = !taskActions(task, sandbox.access, row.pr).archive;
    row.el.classList.toggle('can-archive', !row.archive.hidden);
    row.archive.setAttribute('aria-label', label);
    row.archive.title = label;
  }

  const archivable = (groupRows) => groupRows.filter((row) => taskActions(row.task, sandbox.access, row.pr).archive).map((row) => row.task);

  /**
   * Done's "Archive all" (SPEC-062 QAR-002): every archivable Done task, one click, one Undo. While
   * it runs it stays focusable but inert (aria-disabled), so the keyboard stays on it.
   */
  function archiveAllButton(groupRows) {
    if (!archivable(groupRows).length) return null;
    const button = h('button', { type: 'button', class: 'btn btn-sm btn-ghost task-group-action', 'aria-disabled': archivingAll ? 'true' : null, title: 'Archive every Done task', onClick: async () => {
      const tasks = archivable(groupRows);
      if (archivingAll || !tasks.length) return;
      archivingAll = true;
      button.setAttribute('aria-disabled', 'true');
      try {
        await archiveAll(ctx, tasks, 'Done task');
      } finally {
        archivingAll = false;
        shape = '';
        renderTasks();
      }
    } }, icon('archive', { size: 14 }), 'Archive all');
    return button;
  }

  function groupHead(group, groupRows) {
    const size = groupRows.length;
    const label = [h('span', { class: 'nav-slot' }, icon(group.icon, { size: 16 })), h('span', { class: 'task-group-name' }, group.name), h('span', { class: 'task-group-count' }, String(size))];
    if (group.id === 'done') return h('div', { class: ['task-group-head', `group-${group.id}`] }, label, archiveAllButton(groupRows));
    if (group.id !== 'ended') return h('div', { class: ['task-group-head', `group-${group.id}`] }, label);
    return h('button', { type: 'button', class: ['task-group-head', 'group-ended'], 'aria-expanded': String(endedOpen), onClick: () => {
      endedOpen = !endedOpen;
      writeJson(ENDED_OPEN_KEY, endedOpen);
      shape = '';
      renderTasks();
    } }, label, icon(endedOpen ? 'down' : 'right', { size: 12 }));
  }

  /** A selector for the rebuilt copy of a group's head or "Archive all" (null for a row). */
  function sameControl(el) {
    const kind = ['task-group-action', 'task-group-head'].find((name) => el.classList.contains(name));
    const group = kind && el.closest('.task-group');
    const id = group && GROUPS.find((g) => group.classList.contains(`group-${g.id}`));
    return id ? `.task-group.group-${id.id} .${kind}` : null;
  }

  function renderTasks() {
    if (!sandbox) {
      shape = '';
      // The sandbox list failed to load: said here on pages that do not show it themselves (Home
      // and a sandbox's pages do), never as a toast.
      const error = ctx.store.state.sandboxesError;
      const outside = route && route.name !== 'home' && !route.slug;
      setChildren(list, error && outside
        ? loadError(`Couldn’t load your sandboxes: ${error.message}`, () => ctx.store.loadSandboxes().then(() => renderTasks(), () => renderTasks()))
        : h('div', { class: 'placeholder small' }, 'Open a sandbox to see its tasks.'));
      return;
    }
    const all = ctx.store.tasksOf(sandbox.id);
    if (all === null) {
      shape = '';
      const error = ctx.store.tasksError(sandbox.id);
      setChildren(list, error
        ? loadError(`Couldn’t load tasks: ${error.message}`, () => ctx.store.loadTasks(sandbox.id).catch(() => {}))
        : h('div', { class: 'placeholder placeholder-loading' }, 'Loading…'));
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
    const next = `${sandbox.access}|${GROUPS.map((g) => `${g.id}:${g.id === 'ended' && !endedOpen ? 'closed' : ''}:${grouped.get(g.id).map((r) => r.task.id).join(',')}`).join('|')}`;
    if (next === shape) return;
    shape = next;
    // The keyboard stays in the list across a rebuild (SPEC-062): on the same control (a group's
    // head or "Archive all", rebuilt here), on the same row if it is still listed, else on the
    // row next to the one that left, else on the first task.
    const focused = list.contains(document.activeElement) ? document.activeElement : null;
    const control = focused ? sameControl(focused) : null;
    const row = focused && focused.closest('.task-row');
    const neighbours = row ? [row.nextElementSibling, row.previousElementSibling].map((el) => el && el.querySelector('.task-link')) : [];
    setChildren(list, GROUPS.filter((g) => grouped.get(g.id).length).map((g) => h('section', { class: ['task-group', `group-${g.id}`], 'aria-label': g.name },
      groupHead(g, grouped.get(g.id)),
      g.id === 'ended' && !endedOpen ? null : h('div', { role: 'list' }, grouped.get(g.id).map((r) => r.el)))));
    if (focused && document.activeElement !== focused) {
      const target = [control && list.querySelector(control), focused, ...neighbours].find((el) => el && list.contains(el))
        || list.querySelector('a.task-link, button.task-group-head');
      if (target) target.focus({ preventScroll: true });
    }
  }

  const repaint = coalesce(() => renderTasks(), { minInterval: 150 });
  let unwatchPulls = null;
  const offs = [
    ctx.store.on('tasks', (sandboxId) => { if (sandbox && sandboxId === sandbox.id) repaint(); }),
    ctx.store.on('pulls', (sandboxId) => { if (sandbox && sandboxId === sandbox.id) repaint(); }),
    ctx.store.on('browser-action', (event) => { if (sandbox && event.sandbox_id === sandbox.id) repaint(); }),
  ];
  // Refresh relative times twice a minute (single bounded timer).
  const ageTimer = setInterval(() => {
    for (const row of rows.values()) row.age.textContent = relShort(row.task.updated_at);
  }, 30000);

  return {
    el,
    /** Sign-out: unsubscribe and stop the timer. */
    destroy() { offs.forEach((off) => off()); repaint.cancel(); clearInterval(ageTimer); if (unwatchPulls) unwatchPulls(); window.removeEventListener('resize', fit); if (ctx.fitSidebar === fit) ctx.fitSidebar = null; },
    fit,
    update(nextRoute, nextSandbox) {
      if (!nextSandbox || !sandbox || nextSandbox.id !== sandbox.id) {
        rows.clear();
        shape = '';
        // The rows show the sandbox's pull requests: keep them fresh while it is the one in view.
        if (unwatchPulls) unwatchPulls();
        unwatchPulls = nextSandbox ? ctx.store.watchPulls(nextSandbox.id) : null;
      }
      route = nextRoute;
      sandbox = nextSandbox;
      renderUser();
      renderNav();
      renderTasks();
    },
    refresh: renderTasks,
  };
}

