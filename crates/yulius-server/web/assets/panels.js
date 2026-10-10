// Right rail (Browser, Editor, Shell, Messages) and its resizable panel. The whole rail is
// hidden by default (F 2026-10-07) and shown with the top-bar Tools toggle, remembered per
// browser; opening a panel from a link shows it. Sandbox-scoped panels stay alive while hidden
// so the browser stream, VS Code and shell sessions are not torn down on every toggle. A panel
// can be maximized over the center.

import { enc } from './api.js';
import { h, icon, iconButton, setChildren, errorToast, placeholder, singleFlight, keepFocus, closableTabKeys } from './dom.js';
import { createTerminalView } from './terminal.js';
import { createBrowserPanel } from './browser.js';
import { createEditorView } from './editorview.js';
import { messageView } from './actions.js';
import { allows } from './status.js';
import { readJson, writeJson } from './store.js';

const WIDTH_KEY = 'yulius.panelWidth';
const RAIL_KEY = 'yulius.railVisible';
const MAX_KEY = 'yulius.panelMaximized';
/** Default panel width per kind (fraction of the window); VS Code needs room. */
const DEFAULT_FRACTION = { editor: 0.62 };
const KINDS = [
  ['browser', 'globe', 'Browser'],
  ['editor', 'code', 'Editor'],
  ['shell', 'terminal', 'Shell'],
  ['messages', 'mail', 'Messages'],
];

/** Create the rail + panel. setContext(sandbox, task) on navigation; open(kind, opts). */
export function createPanelHost(ctx) {
  const buttons = new Map();
  const badges = new Map();
  const rail = h('nav', { class: 'rail', 'aria-label': 'Tools' });
  for (const [kind, iconName, label] of KINDS) {
    // Named, not icon-only, so a newcomer sees what each tool is (phones show icons only).
    const button = h('button', { type: 'button', class: 'btn-icon rail-btn', 'aria-label': label, title: label, 'aria-pressed': 'false', onClick: () => toggle(kind) }, icon(iconName, { size: 20 }), h('span', { class: 'rail-label' }, label));
    const dot = h('span', { class: kind === 'messages' ? 'rail-badge' : 'rail-dot', hidden: true });
    button.append(dot);
    buttons.set(kind, button);
    badges.set(kind, dot);
    rail.append(button);
  }

  const title = h('span', { class: 'panel-title' });
  const headExtra = h('div', { class: 'row' });
  const body = h('div', { class: 'panel-body' });
  const handle = h('div', { class: 'panel-resize', role: 'separator', 'aria-orientation': 'vertical', 'aria-label': 'Resize panel', 'aria-valuemin': '0', 'aria-valuemax': '100', tabindex: '0', title: 'Drag, use the arrow keys, or double-click to reset' });
  const maximizeButton = iconButton('maximize', 'Maximize panel', () => setMaximized(!maximized));
  const panel = h('section', { class: 'panel', hidden: true, 'aria-label': 'Tool panel' },
    handle,
    h('div', { class: 'panel-head' }, title, headExtra, maximizeButton, iconButton('x', 'Close panel', () => close())),
    body);
  let maximized = false;
  let railVisible = Boolean(readJson(RAIL_KEY, false));

  /** Shows or hides the whole rail (and closes the panel when hiding). */
  function setRailVisible(visible, { persist = true } = {}) {
    railVisible = visible;
    rail.hidden = !visible;
    const app = rail.closest('.app');
    if (app) app.classList.toggle('rail-hidden', !visible);
    if (!visible && !panel.hidden) close();
    if (persist) writeJson(RAIL_KEY, visible);
    if (ctx.onRailChange) ctx.onRailChange(visible);
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

  // ---- width (default 45 % of the work area, resizable, remembered per browser). The work area is
  // the room the centre and the panel share: the window less the rail and the sidebar, which stays
  // on wider screens (on phones it is a drawer over the page).
  function room() {
    const sidebar = rail.closest('.app')?.querySelector('.sidebar');
    const side = sidebar && !window.matchMedia('(max-width: 767px)').matches ? sidebar.getBoundingClientRect().width : 0;
    return Math.max(1, window.innerWidth - rail.getBoundingClientRect().width - side);
  }
  function defaultWidth(k) {
    const saved = Number(readJson(WIDTH_KEY, 0));
    const fallback = DEFAULT_FRACTION[k] || 0.45;
    const fraction = saved > 0.15 && saved < 0.85 ? Math.max(saved, fallback) : fallback;
    return Math.round(room() * fraction);
  }
  // The width last chosen (opening, dragging, keys); a narrower window shows less of it, and the panel
  // grows back to it when the window widens again.
  let wanted = 0;
  function applyWidth(px, { remember = true } = {}) {
    // The centre keeps at least 360 px next to the panel.
    const max = Math.max(320, room() - 360);
    const width = Math.max(300, Math.min(max, px));
    if (remember) wanted = width;
    panel.style.width = `${width}px`;
    // The splitter says where it is (WCAG 4.1.2): the panel's share of the work area.
    const share = Math.round((width / room()) * 100);
    handle.setAttribute('aria-valuenow', String(share));
    handle.setAttribute('aria-valuetext', `Panel ${share}% of the work area`);
    return width;
  }
  handle.addEventListener('pointerdown', (event) => {
    event.preventDefault();
    handle.setPointerCapture(event.pointerId);
    handle.classList.add('dragging');
    document.body.classList.add('resizing');
    const railLeft = rail.getBoundingClientRect().left;
    const move = (e) => applyWidth(railLeft - e.clientX);
    const up = (e) => {
      handle.releasePointerCapture(e.pointerId);
      handle.classList.remove('dragging');
      document.body.classList.remove('resizing');
      handle.removeEventListener('pointermove', move);
      handle.removeEventListener('pointerup', up);
      writeJson(WIDTH_KEY, panel.getBoundingClientRect().width / room());
    };
    handle.addEventListener('pointermove', move);
    handle.addEventListener('pointerup', up);
  });
  // A double-click resets the width: a way to resize without dragging (WCAG 2.5.7).
  handle.addEventListener('dblclick', () => {
    const width = applyWidth(defaultWidth(kind));
    writeJson(WIDTH_KEY, width / room());
  });
  handle.addEventListener('keydown', (event) => {
    if (event.key !== 'ArrowLeft' && event.key !== 'ArrowRight') return;
    event.preventDefault();
    const width = applyWidth(panel.getBoundingClientRect().width + (event.key === 'ArrowLeft' ? 40 : -40));
    writeJson(WIDTH_KEY, width / room());
  });
  // A panel opened in a wide window kept its pixel width when the window narrowed (or the sidebar
  // widened at a breakpoint), pushing the rail past the window's edge (F 2026-10-09): re-fit it.
  function refit() {
    if (panel.hidden || maximized || !wanted || window.matchMedia('(max-width: 767px)').matches) return;
    applyWidth(wanted, { remember: false });
  }
  window.addEventListener('resize', refit);

  function viewKey(k) {
    if (k === 'messages') return task ? `messages:${task.id}` : 'messages:none';
    return sandbox ? `${k}:${sandbox.id}` : `${k}:none`;
  }

  function createView(k) {
    if (!sandbox) return simpleView(h('div', { class: 'panel-msg' }, 'Open a sandbox first.'));
    if (k === 'browser') return createBrowserPanel(ctx, sandbox);
    if (k === 'editor') return createEditorView(ctx, sandbox, () => task);
    if (k === 'shell') return createShellView(ctx, sandbox, () => task);
    if (k === 'messages') return task ? createMessagesView(ctx, sandbox, task) : simpleView(h('div', { class: 'panel-msg' }, 'Select a task to see its messages.'));
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
    title.textContent = k === 'messages' && task ? `Messages · #${task.number}` : label;
    setChildren(headExtra, view.headExtra ? view.headExtra() : null);
    if (!railVisible) setRailVisible(true);
    if (panel.hidden) {
      panel.hidden = false;
      applyWidth(Math.max(defaultWidth(k), panel.getBoundingClientRect().width || 0));
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
    kind = null;
    for (const button of buttons.values()) {
      button.classList.remove('active');
      button.setAttribute('aria-pressed', 'false');
    }
    for (const view of views.values()) if (view.hide) view.hide();
    ctx.onPanelChange && ctx.onPanelChange(null);
  }

  function toggle(k) {
    if (kind === k && !panel.hidden) close();
    else show(k);
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
    const unread = task && task.unread_messages ? task.unread_messages : 0;
    const msgBadge = badges.get('messages');
    msgBadge.hidden = !unread;
    msgBadge.textContent = unread > 99 ? '99+' : unread ? String(unread) : '';
    const requests = sandbox ? ctx.store.state.actionRequests.get(sandbox.id) : null;
    badges.get('browser').hidden = !(requests && requests.size);
    if (ctx.onToolsAttention) ctx.onToolsAttention(Boolean(requests && requests.size) || Boolean(unread));
    buttons.get('browser').title = requests && requests.size ? 'Browser — an agent needs your action' : 'Browser';
    buttons.get('browser').setAttribute('aria-label', requests && requests.size ? 'Browser, an agent needs your action' : 'Browser');
    buttons.get('messages').setAttribute('aria-label', unread ? `Messages, ${unread} unread` : 'Messages');
    const write = sandbox && allows(sandbox.access, 'write');
    for (const k of ['browser', 'editor', 'shell']) {
      const button = buttons.get(k);
      button.disabled = !sandbox;
      button.classList.toggle('readonly', Boolean(sandbox) && !write);
    }
    buttons.get('messages').disabled = !sandbox;
  }

  const offs = [
    ctx.store.on('browser-action', updateBadges),
    ctx.store.on('task', (t) => { if (task && t.id === task.id) { task = t; updateBadges(); } }),
  ];

  return {
    rail,
    panel,
    /** Sign-out: drop every tool view (terminals, browser, editor) and unsubscribe. */
    destroy() { offs.forEach((off) => off()); window.removeEventListener('resize', refit); destroyWhere(() => true); },
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
        destroyWhere((key) => key.startsWith('messages:'));
        for (const [key, view] of views) if (key.startsWith('editor:') && view.taskChanged) view.taskChanged();
      }
      if (!sandbox && !panel.hidden) close();
      else if (!panel.hidden && kind && (sandboxChanged || taskChanged)) show(kind);
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
          errorToast(error, 'Could not rename the terminal');
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
      errorToast(error, error.status === 429 ? 'Too many terminals' : 'Could not open a terminal');
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
      if (error.status !== 404) { errorToast(error, 'Could not close the terminal'); syncList(); }
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

// ---------------------------------------------------------------- messages

/** Task messages (COM-005: users see messages) with a send box. */
function createMessagesView(ctx, sandbox, task) {
  const list = h('div', { class: 'msg-list', role: 'log', 'aria-live': 'polite', 'aria-label': 'Messages' }, placeholder('Loading messages…', 'loading'));
  const input = h('textarea', { class: 'textarea', rows: 3, placeholder: `Message #${task.number} — delivered to the agent like a task message`, 'aria-label': 'Message', maxlength: 20000 });
  const send = h('button', { type: 'button', class: 'btn btn-primary btn-sm' }, icon('send', { size: 13 }), 'Send');
  const write = allows(sandbox.access, 'write');
  const compose = write
    ? h('div', { class: 'msg-compose' }, input, h('div', { class: 'row' }, h('span', { class: 'muted xs' }, 'Ctrl/⌘ + Enter to send'), h('span', { class: 'spacer' }), send))
    : h('div', { class: 'msg-compose muted small' }, 'Read-only (GitHub: read): you can read messages but not send them.');
  const el = h('div', { class: 'panel-view' }, list, compose);
  const messages = new Map();
  const myLogin = ctx.store.state.me && ctx.store.state.me.user ? ctx.store.state.me.user.login : null;

  // New messages are appended, not the whole list rebuilt: focus and reading position stay, and the
  // log announces only what is new.
  const rendered = new Map(); // message id → element
  function render() {
    const sorted = [...messages.values()].sort((a, b) => String(a.created_at).localeCompare(String(b.created_at)));
    if (sorted.length === 0) { rendered.clear(); setChildren(list, placeholder('No messages yet. Tasks message each other here; you can message the agent too.')); return; }
    const atBottom = list.scrollTop + list.clientHeight >= list.scrollHeight - 24;
    const fresh = sorted.filter((m) => !rendered.has(m.id));
    const last = sorted.filter((m) => rendered.has(m.id)).pop();
    if (rendered.size && fresh.every((m) => !last || String(m.created_at) >= String(last.created_at))) {
      for (const m of fresh) { const el = messageView(ctx, m, myLogin); rendered.set(m.id, el); list.append(el); }
    } else {
      rendered.clear();
      keepFocus(list, () => setChildren(list, sorted.map((m) => { const el = messageView(ctx, m, myLogin); rendered.set(m.id, el); return el; })));
    }
    if (atBottom) list.scrollTop = list.scrollHeight;
  }

  async function load() {
    try {
      const data = await ctx.api.get(`/api/tasks/${enc(task.id)}/messages`);
      messages.clear();
      for (const m of Array.isArray(data) ? data : []) messages.set(m.id, m);
      render();
      list.scrollTop = list.scrollHeight;
    } catch (error) {
      rendered.clear();
      setChildren(list, placeholder(error.message, 'error'));
    }
  }

  const submit = singleFlight(async () => {
    const text = input.value.trim();
    if (!text) return;
    send.disabled = true;
    try {
      const message = await ctx.api.post(`/api/tasks/${enc(task.id)}/messages`, { body: text });
      if (message && message.id) messages.set(message.id, message);
      input.value = '';
      render();
      list.scrollTop = list.scrollHeight;
    } catch (error) {
      errorToast(error, 'Message not sent');
    } finally {
      send.disabled = false;
    }
  });

  send.addEventListener('click', submit);
  input.addEventListener('keydown', (event) => { if (event.key === 'Enter' && (event.metaKey || event.ctrlKey)) { event.preventDefault(); submit(); } });
  const off = [
    ctx.store.on('message', ({ task_id: taskId, message }) => {
      const involved = taskId === task.id || (message && (message.to_task_id === task.id || (message.from && message.from.task_id === task.id)));
      if (involved && message && message.id) { messages.set(message.id, message); render(); }
    }),
    ctx.store.on('resync', load),
  ];
  load();
  return {
    el,
    show() { if (write) setTimeout(() => input.focus(), 0); },
    destroy() { off.forEach((fn) => fn()); },
  };
}

