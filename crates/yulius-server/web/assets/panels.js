// Right rail (Agent, Browser, Editor, Shell) and its resizable panel; each tool keeps the width
// its user gave it (UI-044). The whole rail is hidden by default (F 2026-10-07) and shown with the
// top-bar Tools toggle, remembered per browser; opening a panel from a link shows it.
// Sandbox-scoped panels stay alive while hidden so the browser stream, editor and shell sessions
// are not torn down on every toggle. A panel can be maximized over the center.

import { enc } from './api.js';
import { h, icon, iconButton, setChildren, errorToast, singleFlight, closableTabKeys } from './dom.js';
import { createTerminalView } from './terminal.js';
import { attachTerminalDrop, shellAttachments } from './filedrop.js';
import { createBrowserPanel } from './browser.js';
import { createEditorView } from './editorview.js';
import { createAgentView } from './agent.js';
import { allows } from './status.js';
import { readJson, writeJson } from './store.js';
import { createSplitter, layoutPrefs } from './splitter.js';

/** The panel's width per browser before widths were kept per user (read once as a fallback). */
const WIDTH_KEY = 'yulius.panelWidth';
const RAIL_KEY = 'yulius.railVisible';
/** The panel's narrowest width and the room it leaves the centre (UI-042, UI-044). */
const PANEL_MIN = 300;
const CENTER_MIN = 360;
const MAX_KEY = 'yulius.panelMaximized';
/** Default panel width per kind (share of the work area); the editor needs room. */
const DEFAULT_FRACTION = { editor: 0.62 };
const KINDS = [
  // The user's own Claude Code session for quick work (SPEC-035): first, the tool used most.
  ['agent', 'spark', 'Agent'],
  ['browser', 'globe', 'Browser'],
  ['editor', 'code', 'Editor'],
  ['shell', 'terminal', 'Shell'],
];

/** Create the rail + panel. setContext(sandbox, task) on navigation; open(kind, opts). */
export function createPanelHost(ctx) {
  const buttons = new Map();
  const badges = new Map();
  const rail = h('nav', { class: 'rail', 'aria-label': 'Tools' });
  for (const [kind, iconName, label] of KINDS) {
    // Named, not icon-only, so a newcomer sees what each tool is (phones show icons only).
    const button = h('button', { type: 'button', class: 'btn-icon rail-btn', 'aria-label': label, title: label, 'aria-pressed': 'false', onClick: () => { if (railForPanel) setRailVisible(true); toggle(kind); } }, icon(iconName, { size: 20 }), h('span', { class: 'rail-label' }, label));
    const dot = h('span', { class: 'rail-dot', hidden: true });
    button.append(dot);
    buttons.set(kind, button);
    badges.set(kind, dot);
    rail.append(button);
  }

  const title = h('span', { class: 'panel-title' });
  const headExtra = h('div', { class: 'row' });
  const body = h('div', { class: 'panel-body' });
  const prefs = layoutPrefs(ctx);
  // The splitter on the panel's left edge (UI-044): dragging left widens it.
  const splitter = createSplitter({
    label: 'Resize panel',
    className: 'panel-resize',
    grow: -1,
    width: () => panel.getBoundingClientRect().width,
    resize: (px) => applyWidth(px),
    commit: () => { if (kind) prefs.set(`panel-${kind}`, panel.getBoundingClientRect().width / room()); },
    reset: () => { if (kind) prefs.clear(`panel-${kind}`); applyWidth(defaultWidth(kind, { remembered: false })); },
    range: () => ({ min: PANEL_MIN, max: maxWidth() }),
  });
  const handle = splitter.el;
  const maximizeButton = iconButton('maximize', 'Maximize panel', () => setMaximized(!maximized));
  const panel = h('section', { class: 'panel', hidden: true, 'aria-label': 'Tool panel' },
    handle,
    h('div', { class: 'panel-head' }, title, headExtra, maximizeButton, iconButton('x', 'Close panel', () => close())),
    body);
  let maximized = false;
  let railVisible = Boolean(readJson(RAIL_KEY, false));
  // The rail shown only for a panel opened without it (the top-bar Agent button): it goes again
  // when that panel closes, and the remembered choice stays (SPEC-035 review).
  let railForPanel = false;

  /** Shows or hides the whole rail (and closes the panel when hiding). */
  function setRailVisible(visible, { persist = true } = {}) {
    if (persist) railForPanel = false;
    railVisible = visible;
    rail.hidden = !visible;
    const app = rail.closest('.app');
    if (app) app.classList.toggle('rail-hidden', !visible);
    if (!visible && !panel.hidden) close();
    if (persist) writeJson(RAIL_KEY, visible);
    if (ctx.onRailChange) ctx.onRailChange(visible);
    // The rail's width changes the room the sidebar may take (UI-044).
    if (ctx.fitSidebar) ctx.fitSidebar();
  }

  function setMaximized(value) {
    maximized = value;
    const app = rail.closest('.app');
    if (app) app.classList.toggle('panel-max', value && !panel.hidden);
    maximizeButton.replaceChildren(icon(value ? 'minimize' : 'maximize'));
    maximizeButton.title = value ? 'Restore panel size' : 'Maximize panel';
    maximizeButton.setAttribute('aria-label', maximizeButton.title);
    if (kind) {
      const saved = readJson(MAX_KEY, {}) || {};
      saved[kind] = value;
      writeJson(MAX_KEY, saved);
    }
    window.dispatchEvent(new Event('resize'));
  }

  let kind = null;
  let sandbox = null;
  let task = null;
  const views = new Map(); // key → view

  // ---- width (default 45 % of the work area, resizable, remembered per user in this browser). The
  // work area is the room the centre and the panel share: the window less the rail and the sidebar,
  // which stays on wider screens (on phones it is a drawer over the page).
  function room() {
    const sidebar = rail.closest('.app')?.querySelector('.sidebar');
    const side = sidebar && !window.matchMedia('(max-width: 767px)').matches ? sidebar.getBoundingClientRect().width : 0;
    return Math.max(1, window.innerWidth - rail.getBoundingClientRect().width - side);
  }
  /** The centre keeps at least 360 px next to the panel (where the window has room for both). */
  const maxWidth = () => Math.max(PANEL_MIN, room() - CENTER_MIN);
  /** Each tool opens at the width this user gave it last, else its default share of the work area. */
  function defaultWidth(k, { remembered = true } = {}) {
    const fallback = DEFAULT_FRACTION[k] || 0.45;
    if (!remembered) return Math.round(room() * fallback);
    const saved = prefs.get(`panel-${k}`);
    if (saved > 0.15 && saved < 0.85) return Math.round(room() * saved);
    // The width per browser kept before widths were per user and per tool: never below the default.
    const legacy = Number(readJson(WIDTH_KEY, 0));
    return Math.round(room() * (legacy > 0.15 && legacy < 0.85 ? Math.max(legacy, fallback) : fallback));
  }
  // The width last chosen (opening, dragging, keys); a narrower window shows less of it, and the panel
  // grows back to it when the window widens again.
  let wanted = 0;
  function applyWidth(px, { remember = true } = {}) {
    const width = Math.round(Math.max(PANEL_MIN, Math.min(maxWidth(), px)));
    if (remember) wanted = width;
    panel.style.width = `${width}px`;
    splitter.sync(); // the separator says how wide it is (WCAG 4.1.2)
    return width;
  }
  // A panel opened in a wide window kept its pixel width when the window narrowed (or the sidebar
  // widened), pushing the rail past the window's edge (F 2026-10-09): re-fit it. The sidebar calls
  // this after it changed its own width (UI-044).
  function refit() {
    if (panel.hidden || maximized || !wanted || window.matchMedia('(max-width: 767px)').matches) return;
    applyWidth(wanted, { remember: false });
  }
  window.addEventListener('resize', refit);
  ctx.refitPanel = refit;

  function viewKey(k) {
    return sandbox ? `${k}:${sandbox.id}` : `${k}:none`;
  }

  function createView(k) {
    if (!sandbox) return simpleView(h('div', { class: 'panel-msg' }, 'Open a sandbox first.'));
    if (k === 'agent') return createAgentView(ctx, sandbox);
    if (k === 'browser') return createBrowserPanel(ctx, sandbox);
    if (k === 'editor') return createEditorView(ctx, sandbox, () => task);
    if (k === 'shell') return createShellView(ctx, sandbox, () => task);
    return simpleView(h('div'));
  }

  function show(k, opts = {}) {
    kind = k;
    const key = viewKey(k);
    let view = views.get(key);
    if (!view) {
      view = createView(k);
      view.wrapper = h('div', { class: 'panel-view' }, view.el);
      views.set(key, view);
      body.append(view.wrapper);
    }
    for (const [otherKey, other] of views) other.wrapper.hidden = otherKey !== key;
    const label = KINDS.find(([name]) => name === k)[2];
    title.textContent = label;
    setChildren(headExtra, view.headExtra ? view.headExtra() : null);
    if (!railVisible) {
      railForPanel = opts.persistRail === false;
      setRailVisible(true, { persist: !railForPanel });
    }
    if (panel.hidden) {
      panel.hidden = false;
      // The panel's minimum comes out of the sidebar first, then the tool opens at its width.
      if (ctx.fitSidebar) ctx.fitSidebar();
      applyWidth(defaultWidth(k));
    }
    const savedMax = readJson(MAX_KEY, {}) || {};
    setMaximized(Boolean(savedMax[k]));
    for (const [name, button] of buttons) {
      button.classList.toggle('active', name === k);
      button.setAttribute('aria-pressed', String(name === k));
    }
    if (view.show) view.show(opts);
    ctx.onPanelChange && ctx.onPanelChange(k);
  }

  function close() {
    panel.hidden = true;
    const app = rail.closest('.app');
    if (app) app.classList.remove('panel-max');
    if (ctx.fitSidebar) ctx.fitSidebar(); // the sidebar grows back to its width
    kind = null;
    for (const button of buttons.values()) {
      button.classList.remove('active');
      button.setAttribute('aria-pressed', 'false');
    }
    for (const view of views.values()) if (view.hide) view.hide();
    ctx.onPanelChange && ctx.onPanelChange(null);
    if (railForPanel) {
      railForPanel = false;
      setRailVisible(false, { persist: false });
    }
  }

  function toggle(k, opts) {
    if (kind === k && !panel.hidden) close();
    else show(k, opts);
  }

  function destroyWhere(predicate) {
    for (const [key, view] of [...views]) {
      if (predicate(key)) {
        if (view.destroy) view.destroy();
        view.wrapper.remove();
        views.delete(key);
      }
    }
  }

  function updateBadges() {
    const requests = sandbox ? ctx.store.state.actionRequests.get(sandbox.id) : null;
    badges.get('browser').hidden = !(requests && requests.size);
    if (ctx.onToolsAttention) ctx.onToolsAttention(Boolean(requests && requests.size));
    buttons.get('browser').title = requests && requests.size ? 'Browser — an agent needs your action' : 'Browser';
    buttons.get('browser').setAttribute('aria-label', requests && requests.size ? 'Browser, an agent needs your action' : 'Browser');
    const write = sandbox && allows(sandbox.access, 'write');
    for (const [k] of KINDS) {
      const button = buttons.get(k);
      button.disabled = !sandbox;
      button.classList.toggle('readonly', Boolean(sandbox) && !write);
    }
  }

  const offs = [
    ctx.store.on('browser-action', updateBadges),
    ctx.store.on('task', (t) => { if (task && t.id === task.id) { task = t; updateBadges(); } }),
  ];

  return {
    rail,
    panel,
    /** Sign-out: drop every tool view (terminals, browser, editor) and unsubscribe. */
    destroy() { offs.forEach((off) => off()); window.removeEventListener('resize', refit); if (ctx.refitPanel === refit) ctx.refitPanel = null; destroyWhere(() => true); },
    get railVisible() { return railVisible; },
    setRailVisible,
    /** Applies the remembered rail visibility once the rail is in the document. */
    mounted() { setRailVisible(railVisible, { persist: false }); },
    get kind() { return panel.hidden ? null : kind; },
    setContext(nextSandbox, nextTask) {
      const sandboxChanged = (nextSandbox && nextSandbox.id) !== (sandbox && sandbox.id);
      const taskChanged = (nextTask && nextTask.id) !== (task && task.id);
      sandbox = nextSandbox;
      task = nextTask;
      if (sandboxChanged) destroyWhere(() => true);
      else if (taskChanged) {
        for (const [key, view] of views) if (key.startsWith('editor:') && view.taskChanged) view.taskChanged();
      }
      if (!sandbox && !panel.hidden) close();
      // Shown again for the new context, not opened by the person: the keyboard stays with the page.
      else if (!panel.hidden && kind && (sandboxChanged || taskChanged)) show(kind, { context: true });
      updateBadges();
    },
    open: show,
    toggle,
    close,
    updateBadges,
  };
}

function simpleView(el) {
  return { el };
}

// ---------------------------------------------------------------- shell

/**
 * The user's own terminals in this sandbox (UI-015, UI-016, F 2026-10-09): a tab per terminal,
 * private to the user and the sandbox. Opening the panel shows a working terminal at once — the
 * one used last, or a new one when the user has none here; "+" opens another in the chosen
 * folder (the sandbox repository or the current task's worktree). Tabs close with × or Delete
 * and rename with a double-click or F2. Terminals keep running when the page reloads or the
 * server restarts (`GET /api/sandboxes/{sid}/shells` lists them again); the views of opened
 * tabs stay attached, so switching tabs is instant.
 */
function createShellView(ctx, sandbox, getTask) {
  const el = h('div', { class: 'panel-view' });
  if (!allows(sandbox.access, 'write')) {
    setChildren(el, h('div', { class: 'panel-msg' }, icon('lock', { size: 20 }), h('p', null, 'Terminals need write access to the repository on GitHub.')));
    return { el };
  }
  const activeKey = `yulius.shellActive.${sandbox.id}`;
  const tablist = h('div', { class: 'term-tabs', role: 'tablist', 'aria-label': 'Terminals' });
  const views = h('div', { class: 'term-views' });
  setChildren(el, tablist, views);
  /** Terminals in tab order: {id, name, task_id, created_at, view, slot, ended}. */
  let shells = [];
  let active = null;
  let loaded = false;
  let shown = false;
  let destroyed = false;
  let where = '';
  let renaming = null;

  const folderLabel = (shell) => {
    if (!shell.task_id) return 'Sandbox repository';
    const task = ctx.store.findTask ? ctx.store.findTask(shell.task_id) : null;
    return task ? `Task #${task.number} worktree` : 'Task worktree';
  };

  function folderSelect() {
    const task = getTask();
    if (!task && where === 'task') where = '';
    const select = h('select', { class: 'select select-sm', 'aria-label': 'Folder for new terminals', title: 'Where new terminals start' },
      h('option', { value: '' }, 'Sandbox repository'),
      task ? h('option', { value: 'task' }, `Task #${task.number} worktree`) : null);
    select.value = where;
    select.addEventListener('change', () => { where = select.value; });
    return select;
  }

  function renderTabs() {
    const focused = tablist.contains(document.activeElement) ? document.activeElement.dataset.id : null;
    const items = shells.map((shell) => {
      const selected = shell.id === active;
      const name = renaming === shell.id
        ? renameInput(shell)
        : h('span', { class: 'term-tab-name' }, shell.name);
      const tab = h('div', { class: ['term-tab', selected && 'active', shell.ended && 'term-tab-ended'], role: 'tab', id: `term-tab-${shell.id}`, tabindex: selected ? '0' : '-1', 'aria-selected': String(selected), 'aria-controls': shell.slot ? `term-panel-${shell.id}` : null, dataset: { id: shell.id }, title: `${shell.name} · ${folderLabel(shell)} — double-click or F2 to rename`, 'aria-keyshortcuts': 'F2 Delete Backspace' },
        name,
        // Not a button (see closableTabKeys): Delete or Backspace closes the focused tab.
        h('span', { class: 'term-tab-close', 'aria-hidden': 'true', title: `Close ${shell.name}`, onClick: (event) => { event.stopPropagation(); closeShell(shell); } }, '×'));
      tab.addEventListener('click', () => { if (renaming !== shell.id) activate(shell.id, { focus: true }); });
      tab.addEventListener('dblclick', () => startRename(shell));
      tab.addEventListener('auxclick', (event) => { if (event.button === 1) closeShell(shell); });
      tab.addEventListener('keydown', (event) => {
        if (renaming === shell.id) return;
        if (event.key === 'F2') { event.preventDefault(); startRename(shell); return; }
        closableTabKeys(event, { select: () => activate(shell.id, { focus: true }), close: () => closeShell(shell, { fromKeyboard: true }) });
      });
      return tab;
    });
    // The strip stays when it is empty: a terminal appearing in it never resizes the others.
    setChildren(tablist, items);
    if (focused && renaming === null) (items.find((item) => item.dataset.id === focused) || items.find((item) => item.tabIndex === 0))?.focus();
  }

  function renameInput(shell) {
    const input = h('input', { class: 'term-tab-rename', type: 'text', value: shell.name, maxlength: 40, 'aria-label': `Rename ${shell.name}` });
    let done = false;
    const finish = async (save) => {
      if (done) return;
      done = true;
      renaming = null;
      const name = input.value.trim();
      if (save && name && name !== shell.name) {
        const previous = shell.name;
        shell.name = name;
        renderTabs();
        try {
          const updated = await ctx.api.patch(`/api/sandboxes/${enc(sandbox.id)}/shells/${enc(shell.id)}`, { name });
          if (updated && updated.name) shell.name = updated.name;
        } catch (error) {
          shell.name = previous;
          errorToast(error, 'Couldn’t rename the terminal');
        }
      }
      renderTabs();
      document.getElementById(`term-tab-${shell.id}`)?.focus();
    };
    input.addEventListener('keydown', (event) => {
      event.stopPropagation();
      if (event.key === 'Enter') { event.preventDefault(); finish(true); }
      else if (event.key === 'Escape') { event.preventDefault(); finish(false); }
    });
    input.addEventListener('blur', () => finish(true));
    input.addEventListener('click', (event) => event.stopPropagation());
    setTimeout(() => { input.focus(); input.select(); }, 0);
    return input;
  }

  function startRename(shell) {
    if (shell.ended) return;
    renaming = shell.id;
    renderTabs();
  }

  function renderEmpty() {
    if (shells.length) return;
    setChildren(views, h('div', { class: 'panel-msg stack items-center' },
      h('p', null, 'No terminals open in this sandbox.'),
      h('button', { type: 'button', class: 'btn btn-primary', onClick: () => create({ focus: true }) }, icon('plus', { size: 14 }), 'New terminal')));
  }

  /** The editor folder of a shell: its task's worktree or the sandbox repository. */
  const shellRoot = (shell) => (shell.task_id ? `task:${shell.task_id}` : 'sandbox');

  /** Shows `id`'s terminal, attaching it the first time. */
  function activate(id, { focus = false } = {}) {
    const shell = shells.find((s) => s.id === id);
    if (!shell) return;
    active = id;
    writeJson(activeKey, id);
    if (!shell.view) {
      shell.view = createTerminalView({
        transport: ctx.transport,
        path: `/api/shells/${enc(shell.id)}/terminal`,
        onLink: (uri, event) => ctx.openLink(uri, event),
        readOnlyLabel: () => 'Read-only',
        exitMessage: (exit) => ({ tone: 'info', text: `This terminal ended${exit.code ? ` (exit ${exit.code})` : ''}.` }),
        exitActions: () => [{ label: 'New terminal', run: () => create({ focus: true }) }, { label: 'Close tab', run: () => removeTab(shell) }],
        // `exit` ends the shell: its tab goes, as in a local terminal (a failure stays to be read).
        onExit: (exit) => {
          shell.ended = true;
          if (!exit.code) removeTab(shell);
          else renderTabs();
        },
        onConnection: (status) => { if (status === 'reconnecting') syncList(); },
        // Paths of files in the terminal's folder (and below the directory bash reports) open in the
        // Editor (F 2026-10-10).
        fileLinks: {
          list: (dir) => ctx.api.get(`/api/sandboxes/${enc(sandbox.id)}/files?root=${encodeURIComponent(shellRoot(shell))}&path=${encodeURIComponent(dir)}`).then((listing) => listing.entries || []),
          roots: () => (shell.folder ? [shell.folder] : []),
          open: (file) => ctx.openFile && ctx.openFile(sandbox, { root: shellRoot(shell), ...file }),
        },
      });
      attachTerminalDrop(ctx, shell.view, { label: 'Drop to add to this terminal', upload: shellAttachments(ctx, sandbox.id, shell.id) });
      shell.slot = h('div', { class: 'term-slot', role: 'tabpanel', id: `term-panel-${shell.id}`, 'aria-labelledby': `term-tab-${shell.id}` }, shell.view.el);
    }
    if (!views.contains(shell.slot)) {
      if (!views.querySelector('.term-slot')) setChildren(views, null);
      views.append(shell.slot);
    }
    for (const other of shells) if (other.slot) other.slot.hidden = other.id !== id;
    shell.view.start();
    shell.view.refit();
    if (focus) shell.view.focus();
    renderTabs();
  }

  /** Drops a tab (its terminal ended or was closed); the neighbour is shown. */
  function removeTab(shell, { focus = shown } = {}) {
    const index = shells.indexOf(shell);
    if (index < 0) return;
    shells.splice(index, 1);
    if (shell.view) shell.view.destroy();
    if (shell.slot) shell.slot.remove();
    if (active === shell.id) {
      active = null;
      const next = shells[Math.min(index, shells.length - 1)];
      if (next) activate(next.id, { focus });
    }
    renderTabs();
    renderEmpty();
  }

  /** Opens a new terminal in the chosen folder and shows it. */
  const create = singleFlight(async ({ focus = true } = {}) => {
    try {
      const task = where === 'task' ? getTask() : null;
      const shell = await ctx.api.post(`/api/sandboxes/${enc(sandbox.id)}/shells`, task ? { task_id: task.id } : undefined);
      if (destroyed) return;
      shells.push({ ...shell });
      activate(shell.id, { focus });
    } catch (error) {
      errorToast(error, error.status === 429 ? 'Too many terminals' : 'Couldn’t open a terminal');
      renderEmpty();
    }
  });

  async function closeShell(shell, { fromKeyboard = false } = {}) {
    // Closed from the keyboard, focus stays on the tab strip (#81); otherwise it goes to the
    // terminal now shown.
    removeTab(shell, { focus: shown && !fromKeyboard });
    if (fromKeyboard) tablist.querySelector('[role="tab"][tabindex="0"]')?.focus();
    if (shell.ended) return;
    try {
      await ctx.api.del(`/api/sandboxes/${enc(sandbox.id)}/shells/${enc(shell.id)}`);
    } catch (error) {
      if (error.status !== 404) { errorToast(error, 'Couldn’t close the terminal'); syncList(); }
    }
  }

  /** The server's list (UI-016: this user's terminals in this sandbox only). */
  async function fetchList() {
    const list = await ctx.api.get(`/api/sandboxes/${enc(sandbox.id)}/shells`);
    return Array.isArray(list) ? list : [];
  }

  /** Opens the panel's terminals: the server's list, a new terminal when there is none. */
  const load = singleFlight(async () => {
    try {
      const list = await fetchList();
      if (destroyed) return;
      loaded = true;
      shells = list.map((shell) => ({ ...shell }));
      if (!shells.length) {
        renderTabs();
        await create({ focus: shown });
        return;
      }
      const remembered = readJson(activeKey, null);
      const pick = shells.find((shell) => shell.id === remembered) || shells[shells.length - 1];
      activate(pick.id, { focus: shown });
    } catch (error) {
      if (destroyed) return;
      setChildren(views, h('div', { class: 'panel-msg stack items-center' },
        h('p', null, `Couldn’t load your terminals: ${error.message}`),
        h('button', { type: 'button', class: 'btn', onClick: () => load() }, 'Try again')));
    }
  });

  /** Drops tabs whose terminal ended elsewhere (idle for a day, closed in another window). */
  let syncTimer = 0;
  function syncList() {
    clearTimeout(syncTimer);
    syncTimer = setTimeout(async () => {
      try {
        const list = await fetchList();
        if (destroyed) return;
        const ids = new Set(list.map((shell) => shell.id));
        for (const shell of [...shells]) if (!ids.has(shell.id) && !shell.ended) removeTab(shell);
        for (const fresh of list) {
          const known = shells.find((shell) => shell.id === fresh.id);
          if (known) known.name = fresh.name;
          else shells.push({ ...fresh });
        }
        renderTabs();
      } catch { /* the next reconnect tries again */ }
    }, 400);
  }

  return {
    el,
    headExtra: () => [folderSelect(), iconButton('plus', 'New terminal', () => create({ focus: true }))],
    show() {
      shown = true;
      if (!loaded) { load(); return; }
      const shell = shells.find((s) => s.id === active);
      if (shell && shell.view) setTimeout(() => { shell.view.refit(); shell.view.focus(); }, 0);
      // Opened again after its last terminal was closed: a new one, as on the first opening.
      else if (!shells.length) create({ focus: true });
    },
    hide() { shown = false; },
    destroy() {
      destroyed = true;
      clearTimeout(syncTimer);
      for (const shell of shells) if (shell.view) shell.view.destroy();
    },
  };
}

