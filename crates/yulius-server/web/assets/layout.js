// Global layout pieces (SPEC-001 §6.1): thin sandbox tab row with the Status panel, and the
// left sidebar with navigation, the active task list and the user menu at its foot.

import { DRAG_TAB_HINT, h, icon, iconButton, openMenu, setChildren, statusMark, popoverFocus, keepFocus, loadError, coalesce, pollPaused, prChip, skeleton, bar, skeletonOf } from './dom.js';
import { bytes, duration, dateTime, resetLabel, compact, accountEmail, plausibleEmail } from './format.js';
import { limitsNow, percentNow, placingModel } from './usage.js';
import { PROVIDERS, fallbackLine, modelLabel } from './models.js';
import { sparkline, usd } from './charts.js';
import { taskTime } from './tasktime.js';
import { taskInfo, taskActions, activityLine, allows, otherOwner } from './status.js';
import { href, placeFor } from './router.js';
import { openSandboxDialog, confirmRemoveSandbox, openMoveToGitHubDialog, openGuestInvitesDialog, openTaskMenu, archiveTask, archiveAll } from './actions.js';
import { createNotificationCenter } from './notifications.js';
import { BRAND } from './brand.js';
import { readJson, writeJson } from './store.js';
import { NEW_TASK_KEYS, NEW_TASK_ARIA, HOME_KEYS, HOME_ARIA } from './shortcuts.js';
import { createSplitter, layoutPrefs } from './splitter.js';
import { isPhone } from './theme.js';
import { INSTANCE_ROUTE } from './updates.js';
import { createTabDrag } from './tabdrag.js';

/** Whether this instance has its GitHub App (a playground can move to GitHub then, SPEC-176). */
function githubConnected(ctx) {
  const me = ctx.store.state.me || {};
  return !(me.instance && me.instance.github_configured === false);
}

/** Sidebar widths (UI-044): its range, and what it leaves the panel and the centre (as panels.js). */
const SIDEBAR = { min: 200, max: 480, panelMin: 300, centerMin: 360 };

// The sandbox pages used every day (SPEC-029); CI and CD (SPEC-134) are one page with two tabs
// (SPEC-151), and the Pipeline's stages (SPEC-137) are a tab of the Dashboard (SPEC-161).
const NAV = [
  ['tasks', 'Tasks', 'tasks'],
  ['git', 'Git', 'branch'],
  ['ci', 'CI/CD', 'check'],
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

/**
 * Thin sandbox tab row: tabs (or a dropdown on phones), "+", Status, notifications, tools toggle.
 * The Superagent is first in the tools rail, right under the toggle (SPEC-132).
 */
export function createTabRow(ctx) {
  const tabs = h('nav', { class: 'tabrow-tabs', 'aria-label': 'Sandboxes' });
  const select = h('select', { class: 'select tab-select', 'aria-label': 'Sandbox', onChange: () => {
    if (select.value === '__add') {
      select.value = currentSlug || (home ? '__home' : '');
      openSandboxDialog(ctx);
    } else if (select.value === '__home') {
      ctx.navigate('/');
    } else if (select.value) {
      ctx.navigate(placeFor(`/s/${encodeURIComponent(select.value)}`, 'sandbox'));
    }
  } });
  const hamburger = iconButton('menu', 'Open navigation', () => document.body.classList.toggle('drawer-open'), { class: 'hamburger', 'aria-controls': 'sidebar', 'aria-expanded': 'false' });
  const addButton = iconButton('plus', 'Add sandbox: create or import a repository', () => openSandboxDialog(ctx), { class: 'tab-add' });
  // One name for one fact (#147): sessions are "isolated" in an OS sandbox, or "not isolated".
  const contained = h('span', { class: 'chip chip-warn', hidden: true, title: 'Not isolated: agent sessions on this server run without an OS sandbox, so an agent can reach files outside its sandbox.' }, icon('warn', { size: 12 }), h('span', { class: 'chip-text' }, 'Not isolated'));
  const eventDot = h('span', { class: 'ev-dot', title: 'Live updates connected' });
  const eventText = h('span', { class: 'sr-only' }, ', live updates connected');
  const statusButton = h('button', { type: 'button', class: 'btn btn-status', 'aria-expanded': 'false', 'aria-haspopup': 'dialog', title: 'Status: machine and subscription usage' }, eventDot, h('span', { class: 'status-text' }, 'Status'), eventText, icon('down', { size: 12 }));
  // Shows/hides the right tool rail (Superagent, browser, editor, shell, notes); hidden by default.
  const toolsButton = iconButton('tools', 'Show tools (Superagent, browser, editor, shell, notes)', () => ctx.toggleRail && ctx.toggleRail(), { class: 'tools-toggle' });
  const toolsDot = h('span', { class: 'rail-dot', hidden: true });
  toolsButton.append(toolsDot);
  toolsButton.style.position = 'relative';
  // The button's name carries the attention dot too ("…, an agent needs your action").
  let toolsAttention = false;
  let railVisible = false;
  const labelTools = () => {
    toolsButton.title = railVisible ? 'Hide tools' : 'Show tools (Superagent, browser, editor, shell, notes)';
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

  // A playground's guest (SPEC-410) adds no sandbox and reads no instance status; a badge says
  // their access is a guest's, and until when.
  const guest = ctx.guest && ctx.guest();
  const guestBadge = guest ? h('span', { class: 'chip chip-guest', title: `Guest access to ${guest.slug} until ${dateTime(guest.expires_at)}` }, icon('user', { size: 12 }), h('span', { class: 'chip-text' }, 'Guest')) : null;
  const el = h('header', { class: 'tabrow' },
    hamburger,
    tabs,
    select,
    guest ? null : addButton,
    h('div', { class: 'tabrow-right' }, contained, guestBadge, guest ? null : statusButton, notifications.button, toolsButton),
  );

  let currentSlug = null;
  let home = false;
  let tabsStale = false; // a redraw held back while a tab is pressed
  // Dragging a sandbox tab along the row, as the Notes tabs (tabdrag.js, SPEC-336); "All sandboxes"
  // (no `data-id`) stays first.
  const drag = createTabDrag(tabs, {
    selector: '.tab[data-id]',
    keyOf: (tab) => tab.dataset.id,
    commit: (order) => {
      // A drag back to where it started pins no order: sandboxes added later keep the server's.
      if (order.join() === ctx.store.state.sandboxes.map((sb) => sb.id).join()) renderTabs();
      else ctx.store.setSandboxOrder(order);
      return [...tabs.querySelectorAll('.tab[data-id]')];
    },
    cancelled: () => { if (tabsStale) renderTabs(); },
  });

  function renderTabs() {
    // Counts refresh often: the row is not redrawn under a pressed tab, but once it is let go.
    if (drag.dragging) { tabsStale = true; return; }
    tabsStale = false;
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
        // Back to the screen left in that sandbox (SPEC-113); its own tab opens the composer.
        dataset: { route: `/s/${encodeURIComponent(sandbox.slug)}`, restore: 'sandbox', id: sandbox.id },
        // The pointer drag moves it (tabdrag.js), never the browser's own link drag.
        draggable: 'false',
        title: `${sandbox.repo ? `${sandbox.repo.owner}/${sandbox.repo.name}` : sandbox.name} · ${sandbox.access} access\n${DRAG_TAB_HINT}`,
        'aria-current': sandbox.slug === currentSlug ? 'page' : null,
      },
      // The grip says the tab drags, as on the Notes tabs (SPEC-167); decorative, the name stays the tab's.
      h('span', { class: 'tab-grip', 'aria-hidden': 'true' }, icon('grip', { size: 12 })),
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
          { label: 'Open', onSelect: () => ctx.navigate(placeFor(`/s/${encodeURIComponent(sandbox.slug)}`, 'sandbox')) },
          sandbox.repo && sandbox.repo.url ? { label: 'Open on GitHub', onSelect: () => window.open(sandbox.repo.url, '_blank', 'noopener,noreferrer') } : null,
          // A playground becomes a GitHub sandbox once GitHub is connected (SPEC-176).
          sandbox.playground && githubConnected(ctx) && allows(sandbox.access, 'admin') ? { label: 'Move to GitHub…', onSelect: () => openMoveToGitHubDialog(ctx, sandbox) } : null,
          // Teammates try it without GitHub through an expiring link (SPEC-410).
          sandbox.playground && allows(sandbox.access, 'admin') && !(ctx.guest && ctx.guest()) ? { label: 'Invite guests…', onSelect: () => openGuestInvitesDialog(ctx, sandbox) } : null,
          { separator: true },
          // Reordering without dragging (WCAG 2.5.7): the menu moves a tab one place.
          { label: 'Move left', icon: 'left', disabled: index === 0, onSelect: () => move(-1) },
          { label: 'Move right', icon: 'right', disabled: index === sandboxes.length - 1, onSelect: () => move(1) },
          { separator: true },
          { label: `Remove from ${BRAND.name}…`, danger: true, disabled: !allows(sandbox.access, 'admin'), title: allows(sandbox.access, 'admin') ? null : 'Requires repository admin', onSelect: () => confirmRemoveSandbox(ctx, sandbox) },
        ]);
      });
      a.addEventListener('pointerdown', (event) => drag.press(event, a));
      // The click that ends a drag opens nothing (the router skips a prevented click).
      a.addEventListener('click', (event) => { if (drag.clickConsumed()) event.preventDefault(); });
      return a;
    });
    keepFocus(tabs, () => setChildren(tabs, homeTab, anchors)); // counts refresh often; focus stays
    const active = tabs.querySelector('.tab.active');
    if (active) active.scrollIntoView({ block: 'nearest', inline: 'nearest' });

    setChildren(select,
      h('option', { value: '__home' }, 'All sandboxes'),
      currentSlug || home ? null : h('option', { value: '' }, 'Choose sandbox…'),
      sandboxes.map((s) => h('option', { value: s.slug }, `${s.name}${s.counts && s.counts.needs_attention ? ' (!)' : ''}`)),
      guest ? null : h('option', { value: '__add' }, '+ Add sandbox…'));
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
    destroy() { offs.forEach((off) => off()); drag.cancel(); statusPanel.close(); },
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
  // A playground's guest has no account here beyond their visit (SPEC-410): who and until when.
  const guest = ctx.guest && ctx.guest();
  if (guest) {
    openMenu(anchor, [
      { header: `${user.display_name} · guest until ${dateTime(guest.expires_at)}` },
      { label: 'Sign out', icon: 'logout', onSelect: () => ctx.signOut() },
    ]);
    return;
  }
  const superadmin = Boolean(user.is_superadmin);
  const instanceAdmin = superadmin || Boolean(user.is_instance_admin);
  // Outside a sandbox (Home, Settings…) the per-sandbox entries use the sandbox used last.
  const sandbox = ctx.currentSandbox() || (ctx.defaultSandbox ? ctx.defaultSandbox() : null);
  const items = [
    { header: `${user.display_name || user.login || 'Signed in'}${user.login ? ` · @${user.login}` : ''}${superadmin ? ' · superadmin' : ''}` },
    { label: 'Archived tasks', icon: 'archive', onSelect: () => ctx.navigate('/archived') },
    { label: 'CLI device approval', icon: 'key', onSelect: () => ctx.navigate('/device') },
    { label: 'Team secrets', icon: 'lock', onSelect: () => ctx.navigate(sandbox ? `/s/${encodeURIComponent(sandbox.slug)}/secrets?section=team` : '/secrets') },
  ];
  if (instanceAdmin) {
    items.push({ separator: true }, { header: 'Administration' });
    items.push({ label: 'Users', icon: 'users', onSelect: () => ctx.navigate('/admin/users') });
    items.push({ label: 'Subscriptions', icon: 'user', onSelect: () => ctx.navigate('/admin/accounts') });
    if (superadmin) items.push({ label: 'GitHub connection', icon: 'github', onSelect: () => ctx.navigate('/admin/github') });
    // The instance page: the version and its updates, also beside this button (SPEC-133 UPB-007).
    if (superadmin) items.push({ label: 'Version and updates', icon: 'download', onSelect: () => ctx.navigate(INSTANCE_ROUTE) });
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
    el = h('div', { class: 'status-pop', role: 'dialog', 'aria-label': 'Status' }, skeleton('Loading status…', 3, { block: true }));
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

/** One machine tile: icon and name, a big value, a bar and a detail line (or several). */
function statTile(iconName, name, value, percent, detail, why = null) {
  return h('div', { class: ['stat-tile', `stat-${toneOf(percent)}`], title: why },
    h('div', { class: 'stat-head' }, icon(iconName, { size: 15 }), h('span', null, name)),
    h('div', { class: 'tile-value' }, value),
    usageBar(percent, name),
    [detail || ' '].flat().map((line) => h('div', { class: 'stat-detail' }, line)),
    why ? h('span', { class: 'sr-only' }, why) : null);
}

/**
 * The Sessions tile. The count can pass the limit for a while: a Done task's idle session holds no
 * slot, and one that works again counts again without passing the start door (SES-101); a restart
 * with a lower limit keeps the runs it finds. Then the tile shows the limit full and marks the
 * overshoot instead of "9 / 8" (SPEC-334).
 */
function sessionsTile({ live, limit, queued }) {
  const queue = queued ? `${queued} queued` : null;
  if (!Number.isFinite(live)) return statTile('terminal', 'Sessions', '—', null, queue || 'agent sessions running');
  if (!(limit > 0) || live <= limit) {
    return statTile('terminal', 'Sessions', `${live}${limit ? ` / ${limit}` : ''}`, pct(live, limit), queue || 'agent sessions running');
  }
  const over = live - limit;
  const why = `${over} ${over === 1 ? 'session' : 'sessions'} more than the limit of ${limit}: a Done task that works again counts again, and runs started before a restart with a lower limit keep running. Queued tasks start once fewer than ${limit} run.`;
  // One line each, so a phone's narrow tile cuts neither off.
  return statTile('terminal', 'Sessions', [`${limit} / ${limit}`, h('span', { class: 'tile-over' }, `+${over}`)], 100,
    [`${over} over the limit`, queue].filter(Boolean), why);
}

/**
 * A collapsible status section whose open state survives the panel's refresh. `summary` is one
 * text or its parts, joined with " · ": a narrow panel wraps it between parts, never inside one (#549).
 */
function statusSection(key, iconName, title, summary, body, openByDefault) {
  const parts = [summary].flat();
  // Each part carries its own " ·", so a line breaks only after a dot and ends at the right edge.
  const shown = parts.flatMap((part, i) => [i ? ' ' : null, h('span', { class: 'status-summary-part' }, i < parts.length - 1 ? `${part} ·` : part)]);
  return rememberOpen(h('details', { class: 'status-section status-fold' },
    h('summary', null, icon('right', { size: 12 }), icon(iconName, { size: 15 }), h('h2', null, title), h('span', { class: 'status-summary-text' }, shown)),
    h('div', { class: 'status-fold-body' }, body)), key, openByDefault);
}

/**
 * Render GET /api/status (`StatusView`: vm, sessions, sandboxes, accounts, contained) so it reads
 * in seconds: one health line, machine tiles, then sandboxes and accounts as collapsible lists.
 * VM fields are null when unknown (CPU needs two readings) and render as "—"/"unknown", never as
 * zero. Account last_error/in_use_by are present for admins only (their presence marks an admin's
 * view); every signed-in person gets the id (SPEC-447).
 */
export function renderStatus(data) {
  const vm = data.vm || {};
  const memPct = pct(vm.memory_used_bytes, vm.memory_total_bytes);
  const diskPct = pct(vm.disk_used_bytes, vm.disk_total_bytes);
  const sessions = data.sessions || {};
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
  // What each subscription's windows say now, not the status its last reading stored (SPEC-473).
  const stateNow = new Map(accounts.map((a) => [a, limitsNow(a).state]));
  // Every subscription at a limit: tasks wait for the first window reset (ROT-010). A limit may
  // be the default model's alone (SPEC-464), so the line does not say that nothing runs.
  if (accounts.length && accounts.every((a) => stateNow.get(a) === 'limited')) {
    // The default model alone is used up while its fallback still runs: tasks go on with that
    // (SPEC-482).
    const model = PROVIDERS[0].models[0].id;
    const placed = placingModel(accounts, model);
    health = { tone: 'warn', icon: 'clock', text: placed === model ? 'Every subscription is at a usage limit: tasks continue when a window resets' : fallbackLine(model, placed) };
  }
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

  const ready = accounts.filter((a) => stateNow.get(a) === 'ok').length;
  const limited = accounts.filter((a) => stateNow.get(a) === 'limited').length;
  const accountSummary = [`${ready} ready`, limited ? `${limited} at the limit` : null, troubled.length ? `${troubled.length} need attention` : null].filter(Boolean);

  return [
    h('div', { class: ['status-health', `health-${health.tone}`] }, icon(health.icon, { size: 16 }), h('span', null, health.text)),
    h('div', { class: 'status-section' },
      h('h2', null, 'Machine'),
      h('div', { class: 'stat-tiles' },
        statTile('cpu', 'CPU', percentText(vm.cpu_percent), vm.cpu_percent, vm.cpu_count ? `${vm.cpu_count} cores` : null),
        statTile('memory', 'Memory', percentText(memPct), memPct, vm.memory_total_bytes ? `${bytes(vm.memory_used_bytes)} of ${bytes(vm.memory_total_bytes)}` : null),
        statTile('disk', 'Disk', diskPct === null ? '—' : percentText(diskPct), diskPct, vm.disk_total_bytes ? `${bytes(vm.disk_used_bytes)} of ${bytes(vm.disk_total_bytes)}` : null),
        sessionsTile(sessions)),
      h('div', { class: 'status-facts' }, facts)),
    sandboxes.length ? statusSection('sandboxes', 'folder', 'Sandboxes', `${liveTotal} running in ${busy} of ${sandboxes.length}`, h('div', { class: 'status-rows' }, sandboxRows), false) : null,
    statusSection('accounts', 'user', 'Subscriptions', accounts.length ? accountSummary : 'none yet',
      accounts.length === 0 ? h('div', { class: 'muted small' }, 'No subscriptions yet.') : h('div', { class: 'account-list' }, [...troubled, ...accounts.filter((a) => !troubled.includes(a))].map((a) => accountUsage(a, { foldable: true, open: troubled.includes(a), showEmail: a.in_use_by !== undefined }))), troubled.length > 0 || accounts.length <= 3),
  ];
}

/** Initials for an account's avatar: from the email's name part, else from the label. */
function accountInitials(account) {
  const source = account.email ? account.email.split('@')[0] : (account.label || account.id || '?');
  const parts = String(source).split(/[^A-Za-z0-9]+/).filter(Boolean);
  if (parts.length === 0) return '?';
  return (parts.length > 1 ? parts[0][0] + parts[1][0] : parts[0].slice(0, 2)).toUpperCase();
}

// Whether the main mouse button is down, and what waits for it to come up.
let pressed = false;
let watchingPresses = false;
const afterPressRuns = [];

/** The press is over: what waited for it runs once the click that the release ends is delivered. */
function pressOver() {
  if (!pressed) return;
  pressed = false;
  const runs = afterPressRuns.splice(0);
  if (runs.length) setTimeout(() => { for (const run of runs) run(); }, 0);
}

/**
 * Follows the main mouse button from now on (once per page), by the mouse's own events and not
 * the pointer's: a field loses the keyboard with the mousedown, and a tap's mousedown comes
 * when its pointer is already up. A release may come anywhere, and a press can end where no
 * release is seen (a drag, a context menu, the window losing the pointer): the next move of a
 * mouse whose button is up says so too, so nothing waits on a press that is over.
 */
function watchPresses() {
  if (watchingPresses) return;
  watchingPresses = true;
  document.addEventListener('mousedown', (event) => { if (event.button === 0) pressed = true; }, true);
  document.addEventListener('mouseup', (event) => { if (event.button === 0) pressOver(); }, true);
  for (const type of ['dragend', 'contextmenu']) document.addEventListener(type, pressOver, true);
  document.addEventListener('mousemove', (event) => { if (pressed && !(event.buttons & 1)) pressOver(); }, true);
  window.addEventListener('blur', pressOver);
}

/**
 * Runs `run` now, or, while the mouse button is down (a tap's included), once the press has
 * ended and its click is delivered. A field that is left by a press on a control does its work
 * then: a refusal's line or a saved value drawn at once would move or replace that control
 * before the button comes up, and the click would be lost (#1174).
 */
function afterPress(run) {
  if (pressed) afterPressRuns.push(run);
  else run();
}

/**
 * One line of a subscription's head that its manager edits in place: the name (SPEC-244 SUB-001)
 * or the email under it (SPEC-463 EML-001). The pencil (or a click on the text) turns the line
 * into a field; Enter or leaving the field saves through `save`, Escape cancels. A refused value
 * keeps the field open with the message; the keyboard stays in the field after Enter, is never
 * pulled back to a field that was left, and goes to the pencil once a key ends the edit (not
 * when the person went on while a save was on its way). A field left by a press on a control saves,
 * refuses or closes only once that press has ended (`afterPress`), so the control is clicked.
 * The window losing the focus is not leaving the field. A field that closes says so (the
 * bubbling event `yulius:edit-closed`), whatever ended the edit: a page that kept a refresh
 * from drawing over it reads it then. A field refused as it was left stays open, set aside
 * (`data-set-aside`) until the keyboard comes back to it: the person can return to what they
 * typed, but it no longer keeps what they do next off the page.
 *
 * `line` says what differs: `holder` and `pencil` (classes), `key` (which line of which
 * subscription: the pencil and the field carry it as `data-focus-key`, so a list drawn again
 * finds the control whatever the subscription is called then and wherever its card went),
 * `action()` (the pencil's name),
 * `text()` (the element shown), `field()` (the field's attributes), `stored()` (the value now),
 * `check(value)` (what to say to a value refused before it is sent, else null), `removes` (an
 * empty value removes what is stored, which takes Enter: an emptied field that is left closes
 * as it was) and `failed` (what to say when a failed save gives no message).
 */
function editableLine(line, save) {
  watchPresses();
  const holder = h('span', { class: line.holder });
  const show = (focus = false) => {
    const pencil = iconButton('pencil', line.action(), edit, { class: line.pencil, dataset: { focusKey: line.key } });
    // The text is a second, pointer-only way in (read as text): the pencil is the keyboard's one
    // tab stop.
    const text = line.text();
    text.addEventListener('click', edit);
    setChildren(holder, text,
      pencil);
    if (focus) pencil.focus();
  };
  function edit() {
    const input = h('input', { ...line.field(), dataset: { focusKey: line.key } });
    const error = h('span', { class: 'field-error xs', role: 'alert', id: `${line.key}:refusal` });
    // `closed` once the field is gone: removing a focused field fires its blur, which must not
    // save what Escape cancelled.
    let busy = false;
    let closed = false;
    // `left` once the keyboard has been in the field and gone on: the field's own claim on it
    // (the timer below) must not take it back from where the person went.
    let left = false;
    // Focus goes back to the pencil only when a key ended the edit: a person who clicked
    // elsewhere (another subscription's pencil among them) keeps their focus there.
    const close = (focus) => {
      closed = true;
      show(focus);
      holder.dispatchEvent(new CustomEvent('yulius:edit-closed', { bubbles: true }));
    };
    const finish = async (keep, fromKey) => {
      if (busy || closed) return;
      const value = input.value.trim();
      if (!keep || value === line.stored()) { close(fromKey); return; }
      // Removing takes Enter: an emptied field that was left closes as it was (SPEC-463 EML-002).
      if (!value && line.removes && !fromKey) { close(false); return; }
      // After a key the keyboard stays with the edit (the field when refused, the pencil when
      // done), unless the person went on meanwhile, while a save was on its way: then it stays
      // where they are, as when they left the field.
      const stays = () => fromKey && document.activeElement === input;
      const refuse = (message) => {
        error.textContent = message;
        // The refusal is the field's own (as `invalid` in dom.js says it for a form's control).
        input.setAttribute('aria-invalid', 'true');
        input.setAttribute('aria-describedby', error.id);
        if (!stays()) input.dataset.setAside = 'true';
      };
      const refused = line.check(value);
      if (refused) { refuse(refused); return; }
      busy = true;
      try {
        await save(value);
        busy = false;
        close(stays());
      } catch (err) {
        busy = false;
        refuse((err && err.message) || line.failed);
      }
    };
    input.addEventListener('keydown', (event) => {
      if (event.key === 'Enter') { event.preventDefault(); finish(true, true); }
      else if (event.key === 'Escape') { event.preventDefault(); finish(false, true); }
    });
    input.addEventListener('focus', () => { delete input.dataset.setAside; });
    input.addEventListener('blur', () => {
      // The window lost the focus, not the field, which is still the page's focused element:
      // the person went to fetch what to type and comes back to the field as it is.
      if (document.activeElement === input) return;
      left = true;
      afterPress(() => finish(true, false));
    });
    setChildren(holder, input, error);
    setTimeout(() => { if (!left && !closed) { input.focus(); input.select(); } }, 0);
  }
  show();
  return holder;
}

/** A subscription's name, edited in place (SPEC-244 SUB-001); an empty one is refused. */
function renamableName(account, rename) {
  const name = () => account.label || account.id;
  return editableLine({
    holder: 'account-name-edit',
    pencil: 'account-rename',
    key: `account:${account.id}:name`,
    action: () => `Rename ${name()}`,
    text: () => h('strong', { class: 'ellipsis account-name-text', title: `Rename ${name()}` }, name()),
    field: () => ({ class: 'input account-rename-input', type: 'text', value: account.label || '', maxlength: 80, 'aria-label': `New name for ${name()}` }),
    stored: () => account.label,
    check: (value) => (value ? null : 'Give the subscription a name.'),
    failed: 'Couldn’t rename the subscription.',
  }, rename);
}

/**
 * A subscription's email, typed in place under its name (SPEC-463 EML-001): Claude gives none to
 * a `setup-token` sign-in, so the line reads "email unknown" until an administrator types it.
 * What is not an address is refused here, before it is sent; Enter in the empty field removes
 * the email (EML-002). The field is a text field with an email keyboard: an `email` field
 * rewrites what was typed (a domain outside ASCII comes back as punycode).
 */
function editableEmail(account, setEmail) {
  const name = () => account.label || account.id;
  return editableLine({
    holder: 'account-email-edit',
    pencil: 'account-rename',
    key: `account:${account.id}:email`,
    action: () => `Change the email of ${name()}`,
    text: () => h('span', { class: ['account-sub', 'account-email-text', account.email ? null : 'account-email-unknown'], title: `Change the email of ${name()}` }, accountEmail(account.email)),
    field: () => ({ class: 'input account-rename-input account-email-input', type: 'text', inputmode: 'email', value: account.email || '', maxlength: 254, placeholder: 'name@example.com', autocomplete: 'off', autocapitalize: 'off', spellcheck: 'false', 'aria-label': `Email of ${name()}` }),
    stored: () => account.email || '',
    check: (value) => (!value || plausibleEmail(value) ? null : 'Type an address like name@example.com.'),
    removes: true,
    failed: 'Couldn’t change the email.',
  }, setEmail);
}

// The cards on the page that follow the clock (SPEC-473 RFR-012): what a card says depends on
// the time (a window resets, a reading ages), so each second every shown card works out what it
// says and is drawn again when that changed. One timer for all, stopped with the last card.
const clockCards = new Set();
let clockTimer = null;
/** How long a card that was never put on the page is kept waiting for it. */
const CLOCK_GRACE_MS = 60_000;

function followClock(el, paint) {
  clockCards.add({ el, paint, shown: false, since: Date.now() });
  if (!clockTimer) clockTimer = setInterval(tickClock, 1000);
}

function tickClock() {
  const now = Date.now();
  for (const card of clockCards) {
    if (card.el.isConnected) {
      card.shown = true;
      if (!document.hidden) card.paint(now);
    } else if (card.shown || now - card.since > CLOCK_GRACE_MS) {
      clockCards.delete(card);
    }
  }
  if (!clockCards.size) {
    clearInterval(clockTimer);
    clockTimer = null;
  }
}

/** The usage windows a card can show, by their key in the API's usage. */
const USAGE_KEYS = ['primary', 'secondary', 'weekly_opus', 'weekly_sonnet', 'weekly_fable'];

/**
 * One account, shared by the Status panel (foldable: a row that expands) and the Accounts page:
 * an avatar with initials, label, email for administrators (`showEmail`; "email unknown" until
 * Claude gave one or an administrator typed it) and plan (when the provider reports it), its
 * state when it is not ready, how old its reading is, which windows are used up, and each usage
 * window as a horizontal row like Claude's own usage page. With `rename` and `setEmail` (the
 * Accounts page) the name and the email are edited in place.
 *
 * What depends on the time (the state, the age, the used-up windows, a window past its reset) is
 * worked out when the card is drawn and again every second while it is on the page, and drawn
 * again only when it changed (SPEC-473): a card stops saying "Limit reached" the moment its
 * window resets. The name and the email are not part of that, so a field being typed in stays.
 *
 * `refresh` (the Accounts page) is `{ busy(), run() }`: a Refresh button beside the age, off and
 * reading "Reading…" while `busy()`. `note()` gives a sentence the page has about this card (a
 * refresh that failed, or read nothing new). The page tells the card that either changed with a
 * `yulius:usage-changed` event on the card.
 */
export function accountUsage(account, { foldable = false, open = false, rename = null, setEmail = null, showEmail = false, refresh = null, note = null } = {}) {
  const provider = account.provider === 'anthropic' ? 'Claude' : account.provider;
  const label = () => account.label || account.id;
  // Usage not read yet: a shimmering bar where the reset time will be (SPEC-092).
  const reading = () => [h('span', { class: 'skeleton-line', 'aria-hidden': 'true' }), h('span', { class: 'sr-only' }, 'Reading usage…')];
  // A window past its reset began anew and nobody read it since: not its old percent (RFR-011).
  const usageRow = (w, fallback, at) => {
    const name = limitName(w && w.window_minutes, fallback);
    const used = percentNow(w, at);
    const reset = Boolean(w) && Number.isFinite(w.used_percent) && used === null;
    return h('div', { class: 'usage-row' },
      h('div', { class: 'usage-name' }, h('span', null, name), h('span', { class: 'usage-reset', title: w && w.resets_at ? dateTime(w.resets_at) : '' }, reset ? 'Reset since this reading' : w && w.resets_at ? resetLabel(w.resets_at, at) : Number.isFinite(used) ? '' : reading())),
      usageBar(used, name),
      h('span', { class: ['usage-pct', `tone-${toneOf(used)}`] }, Number.isFinite(used) ? `${Math.round(used)}% used` : '—'));
  };
  // Folded, both windows show at a glance: a short name, a bar named in full and the percent (SPEC-172).
  const glance = (usage, at) => {
    const windows = [[usage.primary, '5 h', '5-hour window'], [usage.secondary, 'Week', 'This week']].map(([w, short, name]) => ({ used: percentNow(w, at), short, name }));
    return h('span', { class: 'account-glance', title: windows.map((g) => `${g.name}: ${Number.isFinite(g.used) ? `${Math.round(g.used)}% used` : 'not known yet'}`).join(' · ') },
      windows.map((g) => h('span', { class: 'glance-line' },
        h('span', { class: 'glance-name' }, g.short),
        usageBar(g.used, g.name),
        h('span', { class: ['usage-pct', `tone-${toneOf(g.used)}`] }, percentText(g.used)))));
  };
  // The three parts the clock draws again; `display: contents`, so the head's row and the body's
  // column lay their children out as before.
  const stateHost = h('span', { class: 'account-live' });
  const glanceHost = foldable ? h('span', { class: 'account-live' }) : null;
  const liveHost = h('div', { class: 'account-live' });
  const head = [
    h('span', { class: 'avatar account-avatar', 'aria-hidden': 'true' }, accountInitials(account)),
    h('span', { class: 'account-who' },
      h('span', { class: 'account-name' },
        rename && !foldable ? renamableName(account, rename) : h('strong', { class: 'ellipsis', title: account.label || account.id }, account.label || account.id),
        account.plan ? h('span', { class: 'plan-badge', title: `${provider} plan` }, account.plan) : null),
      // The email is for administrators (UI-022); others see the provider as before.
      !showEmail ? h('span', { class: 'account-sub' }, provider)
        : setEmail && !foldable ? editableEmail(account, setEmail)
          : h('span', { class: ['account-sub', account.email ? null : 'account-email-unknown'], title: account.email ? `${provider} account` : 'Shown after the subscription’s next sign-in, when Claude shares it' }, accountEmail(account.email))),
    stateHost,
    glanceHost,
    foldable ? icon('down', { size: 14 }) : null,
  ];
  const foot = h('div', { class: 'account-foot' });
  const body = h('div', { class: 'account-body' }, liveHost, foot);
  const el = foldable
    ? rememberOpen(h('details', { class: 'account-fold' }, h('summary', { class: 'account-head' }, head), body), `account:${account.id}`, open)
    : h('div', { class: 'account-card' }, h('div', { class: 'account-head' }, head), body);

  let drawn = null;
  const paint = (at) => {
    const usage = account.usage || {};
    const now = limitsNow(account, at);
    const busy = Boolean(refresh && refresh.busy());
    const said = note ? note() : null;
    // Everything the card says that the time, a refresh or a new reading can change.
    const says = JSON.stringify([now, busy, said, account.status, account.last_error, account.in_use_by,
      USAGE_KEYS.map((key) => [percentNow(usage[key], at), usage[key] && usage[key].resets_at ? resetLabel(usage[key].resets_at, at) : null])]);
    if (says === drawn) return;
    drawn = says;
    const state = ACCOUNT_STATES[now.state] || { icon: 'warn', tone: 'muted', label: now.state || 'Unknown' };
    const outOfDate = `${now.age}, may be out of date`;
    // A subscription that needs a sign-in and was never read has no reading to date.
    const dated = Boolean(usage.observed_at) || now.state === 'ok' || now.state === 'limited';
    // A usage reading that failed for now ("usage: …", one plain sentence) is a quiet note, not a
    // broken account: tasks still run on it and the last known usage stays (ROT-009, #27). What
    // the page knows about its own refresh comes first (SPEC-473 RFR-013).
    const usageNote = Boolean(account.last_error) && account.last_error.startsWith('usage:') && !['relogin', 'error'].includes(account.status);
    const quiet = said || (usageNote ? account.last_error.slice('usage:'.length).trim() : null);
    keepFocus(el, () => {
      setChildren(stateHost, state.tone !== 'ok' ? h('span', { class: ['account-state', `state-${state.tone}`], title: state.label }, icon(state.icon, { size: 12 }), h('span', { class: 'account-state-label' }, state.label)) : null);
      // Folded, a reading that may be out of date is marked beside the glance: an icon, with
      // the words for a screen reader and on hover (RFR-009).
      if (glanceHost) setChildren(glanceHost, now.stale ? h('span', { class: 'account-stale', title: outOfDate }, icon('clock', { size: 12 }), h('span', { class: 'sr-only' }, outOfDate)) : null, glance(usage, at));
      setChildren(liveHost,
        dated || refresh ? h('div', { class: 'account-reading', dataset: { stale: String(now.stale) } },
          dated ? h('span', { class: 'account-age', title: usage.observed_at ? dateTime(usage.observed_at) : null }, now.stale ? icon('clock', { size: 13 }) : null, h('span', null, now.stale ? `${now.age} · may be out of date` : now.age)) : h('span'),
          // Off while it reads without losing the keyboard: aria-disabled, not disabled. Its
          // name is what it shows plus whose it is ("Refresh usage of Claude #1"), so it says
          // "Reading…" to a screen reader too.
          refresh ? h('button', { type: 'button', class: 'btn btn-sm account-refresh', 'aria-busy': busy ? 'true' : null, 'aria-disabled': busy ? 'true' : null, dataset: { focusKey: `account:${account.id}:refresh` }, onClick: () => { if (!refresh.busy()) refresh.run(); } },
            icon('refresh', { size: 13 }), h('span', null, busy ? 'Reading…' : 'Refresh', h('span', { class: 'sr-only' }, ` usage of ${label()}`))) : null) : null,
        now.lines.map((line) => h('div', { class: 'account-limit', dataset: { window: line.key } }, icon('clock', { size: 13 }), h('span', null, line.text))),
        quiet ? h('div', { class: 'account-note' }, icon('clock', { size: 13 }), h('span', null, quiet)) : null,
        // Claude refusing a subscription (cancelled, downgraded, revoked) marks it for a new sign-in
        // with Claude's words, said as why (SPEC-447 MOV-005).
        account.last_error && !usageNote ? h('div', { class: 'account-error' }, icon('warn', { size: 13 }), h('span', null, account.status === 'relogin' ? `Claude refused it: ${account.last_error.trim().replace(/\.$/, '')}. Sign in again.` : account.last_error)) : null,
        usageRow(usage.primary, '5-hour window', at),
        usageRow(usage.secondary, 'This week', at),
        // Per-model weekly caps, when the plan has them (they bind only tasks of that model).
        usage.weekly_opus ? usageRow({ ...usage.weekly_opus, window_minutes: null }, 'Opus this week', at) : null,
        usage.weekly_sonnet ? usageRow({ ...usage.weekly_sonnet, window_minutes: null }, 'Sonnet this week', at) : null,
        // Fable runs on a weekly allowance of usage credits no reading shows: the row appears once
        // Claude reported it used up, full until its reset (SPEC-464).
        usage.weekly_fable ? usageRow({ ...usage.weekly_fable, window_minutes: null }, 'Fable this week', at) : null);
      setChildren(foot, account.in_use_by ? h('span', { class: 'status-count live' }, h('span', { class: 'live-dot' }), `${account.in_use_by} ${account.in_use_by === 1 ? 'task' : 'tasks'} on this subscription`) : h('span', null, 'No tasks right now'));
    });
  };
  paint(Date.now());
  followClock(el, paint);
  el.addEventListener('yulius:usage-changed', () => paint(Date.now()));
  return el;
}

/** A number tile of the capacity panel: name, value, detail lines and its 7-day chart (no meter). */
function capacityTile(iconName, name, value, detail, tone = 'ok', chart = null) {
  const lines = (Array.isArray(detail) ? detail : [detail]).filter(Boolean);
  return h('div', { class: ['stat-tile', `stat-${tone}`] },
    h('div', { class: 'stat-head' }, icon(iconName, { size: 15 }), h('span', null, name)),
    h('div', { class: 'tile-value' }, value),
    chart,
    (lines.length ? lines : [' ']).map((line) => h('div', { class: 'stat-detail' }, line)));
}

/** A 7-day chart of a capacity figure ("Tokens, last 7 days: first …"), or null without data. */
function weekChart(name, values, format) {
  return Array.isArray(values) ? sparkline(values, { label: `${name}, last 7 days`, format }) : null;
}

const spent = (t) => (t ? (t.input || 0) + (t.output || 0) : null);

/** The token and API-cost tiles of the pool (SPEC-110 TUS-006). */
function tokenTiles(tokens) {
  const t = tokens || {};
  const week = t.last_7d || null;
  const day = t.last_24h || null;
  const perDay = Array.isArray(t.per_day) ? t.per_day : null;
  const cost = week && Number.isFinite(week.cost_usd) ? week.cost_usd : null;
  const unpriced = t.unpriced_runs || 0;
  const unknown = t.unknown_usage_runs || 0;
  return [
    capacityTile('chart', 'Tokens (7 days)', week ? compact(spent(week)) : '—', week ? [
      `${compact(week.input)} in · ${compact(week.output)} out · ${compact(week.cache_read)} from cache`,
      `last 24 h: ${compact(spent(day))}${unknown ? ` · ${unknown} ${unknown === 1 ? 'run' : 'runs'} reported no usage` : ''}`,
    ] : 'not known yet', 'ok', perDay && weekChart('Tokens', perDay.map(spent), compact)),
    capacityTile('zap', 'API cost (7 days)', cost === null ? '—' : `≈ ${usd(cost)}`, [
      'at Claude API list prices',
      cost === null ? (week && spent(week) ? 'no run’s model has a price' : null)
        : `${day && Number.isFinite(day.cost_usd) ? `last 24 h ≈ ${usd(day.cost_usd)}` : 'last 24 h: —'}${unpriced ? ` · ${unpriced} ${unpriced === 1 ? 'run' : 'runs'} of unknown models not counted` : ''}`,
    ], 'ok', perDay && weekChart('API cost', perDay.map((d) => (Number.isFinite(d.cost_usd) ? d.cost_usd : null)), usd)),
  ];
}

/** Subscriptions' weekly limits used per week at `burn` points per hour (ADR-17 §2.4). */
function perWeek(burn) {
  return Number.isFinite(burn) ? (burn * 168) / 100 : null;
}

/**
 * The subscription pool from GET /api/capacity (ROT-013): one recommendation line and the
 * numbers behind it — available now, what is left this week, the burn rate, when the pool runs
 * out at that rate, the tokens it used and their cost at API list prices (SPEC-110), and the hours
 * tasks waited, with a 7-day chart where the history means something. Shared by the Accounts page and the Dashboard;
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
  // While no subscription can run the default model, tasks run its fallback (SPEC-482): the
  // panel says so, and "Available now" counts the subscriptions that run the fallback.
  const fallback = v.fallback_model || null;
  return h('div', { class: 'stack capacity-panel' },
    h('div', { class: ['status-health', `health-${add > 0 ? 'warn' : 'ok'}`], role: 'status' }, icon(add > 0 ? 'warn' : 'check', { size: 16 }), h('span', null, line)),
    fallback ? h('div', { class: ['status-health', 'health-warn', 'capacity-fallback'], role: 'status' }, icon('clock', { size: 16 }), h('span', null, fallbackLine(PROVIDERS[0].models[0].id, fallback))) : null,
    h('div', { class: 'stat-tiles' },
      capacityTile('user', 'Available now', `${Number.isFinite(v.available) ? v.available : 0} of ${subscriptions}`, fallback ? `for ${modelLabel(fallback)}, below ${threshold}% in every window` : `below ${threshold}% in every window`, v.available ? 'ok' : 'danger'),
      capacityTile('clock', 'Left this week', Number.isFinite(weekly) ? `${Math.round(weekly)}%` : '—', Number.isFinite(session) ? `5-hour windows: ${Math.round(session)}% left` : 'of the pool before it pauses', leftTone(weekly)),
      capacityTile('zap', 'Burn rate', burn24 === null ? '—' : `${burn24.toFixed(1)} / week`, burn24 === null ? 'not enough readings yet' : `subscriptions' weekly limits, last 24 h${burn7 === null ? '' : ` (7 d: ${burn7.toFixed(1)})`}`, 'ok',
        weekChart('Burn rate', (v.burn_per_day || []).map(perWeek), (x) => `${x.toFixed(1)} / week`)),
      capacityTile('chart', 'Runs out', v.runs_out_at ? resetLabel(v.runs_out_at).replace(/^Resets (at )?/, '') : burn24 === null ? '—' : 'Not this week', v.runs_out_at ? 'at the current rate' : 'at the current rate, before the next resets', v.runs_out_at ? 'warn' : 'ok'),
      tokenTiles(v.tokens),
      capacityTile('pulse', 'Paused (7 days)', `${hours.toFixed(hours >= 10 ? 0 : 1)} h`, Number.isFinite(v.active_hours_7d) ? `task-hours; ${Math.round(v.active_hours_7d)} h worked` : 'task-hours', hours > 0 ? 'warn' : 'ok',
        weekChart('Paused', v.paused_hours_per_day, (x) => `${x.toFixed(1)} h`))));
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
  // A playground's guest (SPEC-410) sees its work only: Tasks, Git and Sessions.
  const forGuest = ctx.guest && ctx.guest() ? ([page]) => ['tasks', 'git', 'sessions'].includes(page) : () => true;
  const primaryLinks = NAV.filter(forGuest).map(navLink);
  const moreLinks = NAV_MORE.filter(forGuest).map(navLink);
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
    if (shell) {
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
      // Back to the page's filters (SPEC-113); the link to the page in view opens it plain.
      if (slug) link.dataset.restore = 'page'; else delete link.dataset.restore;
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
      // One line: state icon, task number, pull request, title, age. What the task needs is said
      // by the icon, the tooltip and (for screen readers) the hidden sentence after the title.
      // A bare #N is the task's, the chip's "PR #N" its pull request's (SPEC-122).
      const mark = h('span', { class: 'task-mark' });
      const num = h('span', { class: 'task-num' });
      const chips = h('span', { class: 'task-chips' });
      const title = h('span', { class: 'task-title' });
      const sub = h('span', { class: 'sr-only' });
      const age = h('span', { class: 'task-age' });
      // Someone else's task (SPEC-070, SPEC-179): its owner's initials, the words in the tooltip.
      const owner = h('span', { class: 'task-owner', 'aria-hidden': 'true', hidden: true });
      const link = h('a', { class: 'task-link' }, mark, num, chips, title, sub, owner, age);
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
      row = { el, link, mark, num, chips, title, age, sub, owner, archive, task };
      rows.set(task.id, row);
    }
    row.task = task;
    row.info = rowInfo(task);
    // Repaint only a row whose content would change (PERF-02): the key holds all it shows.
    const active = Boolean(route && route.name === 'task' && route.number === task.number);
    row.pr = ctx.store.pullsOf(sandbox.id).get(task.id) || null;
    row.time = taskTime(task, row.info);
    const key = JSON.stringify([task, row.info, active, row.time.text, sandbox.slug, sandbox.access, row.pr, ctx.store.state.me?.user?.login]);
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
    const owner = otherOwner(task, ctx.store.state.me?.user?.login);
    row.link.title = `Task #${task.number} ${task.title} — ${info.label}${info.detail ? `: ${info.detail}` : ''}${row.pr && !info.label.includes(`PR #${row.pr.number}`) ? ` · PR #${row.pr.number}` : ''}${task.branch ? ` · ${task.branch}` : ''}${task.account_label ? ` · on ${task.account_label}` : ''}${owner ? ` · owned by @${owner}` : ''} · ${row.time.text}`;
    row.owner.hidden = !owner;
    row.owner.textContent = owner ? initials(owner).toUpperCase() : '';
    if (owner) row.owner.title = `@${owner} owns this task`;
    else row.owner.removeAttribute('title');
    setChildren(row.mark, statusMark(info, 16));
    setChildren(row.num, h('span', { class: 'sr-only' }, 'Task '), `#${task.number}`);
    row.num.title = `Task #${task.number}`;
    setChildren(row.chips, row.pr ? prChip(row.pr, { link: false }) : null);
    row.title.textContent = task.title || `Task #${task.number}`;
    // Colour stays on the state icon; the words (tooltip, screen readers) say what to do.
    row.sub.textContent = `, ${activityLine(task, info)}${owner ? `, owned by @${owner}` : ''}`;
    // What the time measures depends on the group, and the words say it (SPEC-087 TIM-003).
    row.age.textContent = row.time.text;
    row.age.title = row.time.title;
    const label = `Archive task #${task.number}`;
    const archivable = taskActions(task, sandbox.access, row.pr, ctx.store.state.me?.user?.login).archive;
    // The keyboard on a quick Archive that hides (the task was continued) goes to its row's link,
    // never to the page (#331); a rebuild after this keeps it there.
    if (!archivable && document.activeElement === row.archive) row.link.focus({ preventScroll: true });
    row.archive.hidden = !archivable;
    row.el.classList.toggle('can-archive', !row.archive.hidden);
    row.archive.setAttribute('aria-label', label);
    row.archive.title = label;
  }

  const archivable = (groupRows) => groupRows.filter((row) => taskActions(row.task, sandbox.access, row.pr, ctx.store.state.me?.user?.login).archive).map((row) => row.task);

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
    } }, icon('archive', { size: 14 }), h('span', { class: 'task-group-action-label' }, 'Archive all'));
    return button;
  }

  function groupHead(group, groupRows) {
    const size = groupRows.length;
    const label = [h('span', { class: 'nav-slot' }, icon(group.icon, { size: 16 })), h('span', { class: 'task-group-name' }, group.name), h('span', { class: 'task-group-count' }, String(size))];
    if (group.id === 'done') return h('div', { class: ['task-group-head', `group-${group.id}`] }, label, archiveAllButton(groupRows));
    // How the open tasks wait for each other, on the Tasks page's Graph (SPEC-164).
    if (group.id === 'working') {
      const path = `/s/${encodeURIComponent(sandbox.slug)}/tasks?view=graph`;
      return h('div', { class: ['task-group-head', `group-${group.id}`] }, label,
        h('a', { class: ['btn-icon', 'task-group-action', 'task-group-graph'], href: href(path), dataset: { route: path }, 'aria-label': 'Show as a graph', title: 'Show as a graph: the queue, chains and rules' }, icon('workflow', { size: 14 })));
    }
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
        // A group band over task rows, as the list will be (SPEC-120); `.task-row` stays real tasks'.
        : skeletonOf('Loading tasks…', h('div', { class: 'skeleton-task-group' },
          h('div', { class: 'task-group-head' }, bar('lg')),
          [70, 55, 80, 45, 62].map((width) => h('div', { class: 'skeleton-task-row' },
            h('span', { class: 'task-mark' }, h('span', { class: 'skeleton-tile skeleton-dot' })),
            h('span', { class: 'task-num' }, bar('sm')), h('span', { class: 'task-title' }, bar(width)), h('span', { class: 'task-age' }, bar('sm')))))));
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
  // Refresh the times twice a minute (single bounded timer).
  const ageTimer = setInterval(() => {
    for (const row of rows.values()) {
      row.time = taskTime(row.task, row.info);
      row.age.textContent = row.time.text;
      row.age.title = row.time.title;
    }
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

