// Right rail (Agent, Browser, Editor, Shell, Notes) and its resizable panel; each tool keeps the width
// its user gave it (UI-044). The whole rail is hidden by default (F 2026-10-07) and shown with the
// top-bar Tools toggle, remembered per browser; opening a panel from a link shows it.
// Sandbox-scoped panels stay alive while hidden so the browser stream, editor and shell sessions
// are not torn down on every toggle, nor on a switch to another sandbox and back (SPEC-113): each
// sandbox keeps its open tool and, for the sandboxes used last, its views. A panel can be maximized
// over the center.

import { enc } from './api.js';
import { DRAG_TAB_HINT, h, icon, iconButton, openMenu, setChildren, errorToast, singleFlight, closableTabKeys } from './dom.js';
import { createTerminalView } from './terminal.js';
import { attachTerminalDrop, shellAttachments } from './filedrop.js';
import { createBrowserPanel } from './browser.js';
import { createEditorView } from './editorview.js';
import { chosenSuperagentTab, createAgentView, createSuperagentPanel } from './agent.js';
import { createNotesView } from './notes.js';
import { allows } from './status.js';
import { readJson, writeJson } from './store.js';
import { createSplitter, layoutPrefs } from './splitter.js';
import { createTabDrag, inOrder, movedBy } from './tabdrag.js';

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
  ['agent', 'spark', 'Superagent'],
  ['browser', 'globe', 'Browser'],
  ['editor', 'code', 'Editor'],
  ['shell', 'terminal', 'Shell'],
  // Auto-saved notepads for this sandbox and the team (SPEC-041).
  ['notes', 'note', 'Notes'],
];
/** Tool views kept for this many sandboxes, the one in view included (SPEC-113 KST-006). */
const KEEP_SANDBOXES = 3;
/**
 * Tools whose views a sandbox switch keeps. Notes are rebuilt instead: the Team and Me notepads are
 * the same in every sandbox, and two live copies in one tab would edit them against each other.
 */
const KEPT_KINDS = new Set(['agent', 'browser', 'editor', 'shell']);
/** The sandbox id of a view key (`<kind>:<sandbox id>`). */
const keySandbox = (key) => key.slice(key.indexOf(':') + 1);

/** Create the rail + panel. setContext(sandbox, task) on navigation; open(kind, opts). */
export function createPanelHost(ctx) {
  const buttons = new Map();
  const badges = new Map();
  const rail = h('nav', { class: 'rail', 'aria-label': 'Tools' });
  for (const [kind, iconName, label] of KINDS) {
    // Named, not icon-only, so a newcomer sees what each tool is (phones show icons only).
    const button = h('button', { type: 'button', class: 'btn-icon rail-btn', dataset: { kind }, 'aria-label': label, title: label, 'aria-pressed': 'false', onClick: () => railClick(kind) }, icon(iconName, { size: 20 }), h('span', { class: 'rail-label' }, label));
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
  // The rail shown only for a panel opened without it (a Superagent link, a sandbox's tool coming
  // back): it goes again when that panel closes, and the remembered choice stays (SPEC-035 review).
  let railForPanel = false;

  /** Shows or hides the whole rail (and closes the panel when hiding). */
  function setRailVisible(visible, { persist = true } = {}) {
    if (persist) railForPanel = false;
    railVisible = visible;
    rail.hidden = !visible;
    const app = rail.closest('.app');
    if (app) app.classList.toggle('rail-hidden', !visible);
    // The tools hidden: the one waiting for its sandbox is not wanted any more.
    if (!visible && chosen) { chosen = null; press(); }
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
  let recent = []; // sandbox ids whose views are kept, the one used last first
  const toolOf = new Map(); // sandbox id → the tool it had open, or null (SPEC-113 KST-005)
  const arriving = new Set(); // sandbox ids whose page is loading, not the context yet (SPEC-170)
  // The tool the person chose while a sandbox's page loads and none is the context yet, how, and
  // for which sandbox (`{ kind, opts, sandbox }`): it opens there when it arrives, and is not
  // wanted any more once another sandbox's page starts loading (SPEC-450).
  let chosen = null;
  // The page's one Global Superagent view, held by whichever Superagent panel shows its tab (SPEC-428).
  let globalAgent = null;
  const sharedGlobal = () => globalAgent || (globalAgent = createAgentView(ctx, null));
  // The Superagent tab in front in every sandbox of this page: the stored choice once, then whatever
  // a sandbox's panel shows (SAT-002, review round 2).
  let agentTab = chosenSuperagentTab();

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
    // This sandbox's Superagent and the person's Global one, as tabs (SPEC-428); outside a sandbox
    // only the Global one (SPEC-427 GSA-006).
    if (k === 'agent') return createSuperagentPanel(ctx, sandbox, { global: sharedGlobal, tab: () => agentTab, onTab: (tab) => { agentTab = tab; } });
    if (!sandbox) return simpleView(h('div', { class: 'panel-msg' }, 'Open a sandbox first.'));
    if (k === 'browser') return createBrowserPanel(ctx, sandbox);
    // The selected task, while it is one of this view's sandbox (the view outlives a switch).
    const own = sandbox;
    const taskHere = () => (task && task.sandbox_id === own.id ? task : null);
    if (k === 'editor') return createEditorView(ctx, sandbox, taskHere);
    if (k === 'shell') return createShellView(ctx, sandbox, taskHere);
    if (k === 'notes') return createNotesView(ctx, sandbox);
    return simpleView(h('div'));
  }

  function show(k, opts = {}) {
    kind = k;
    if (sandbox) toolOf.set(sandbox.id, k);
    const key = viewKey(k);
    let view = views.get(key);
    if (!view) {
      view = createView(k);
      view.wrapper = h('div', { class: 'panel-view' }, view.el);
      views.set(key, view);
      body.append(view.wrapper);
    }
    for (const [otherKey, other] of views) {
      const hiding = otherKey !== key && !other.wrapper.hidden;
      other.wrapper.hidden = otherKey !== key;
      // Another tool in front: the one it replaces is hidden as when the panel closes (its presence
      // leaves, its stream stops; SPEC-127).
      if (hiding && other.hide) other.hide();
    }
    const label = KINDS.find(([name]) => name === k)[2];
    title.textContent = view.title || label;
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
    press();
    if (view.resume) view.resume();
    if (view.show) view.show(opts);
  }

  /** The rail says which tool is open, or chosen and waiting for its sandbox. */
  function press() {
    const pressed = chosen ? chosen.kind : (panel.hidden ? null : kind);
    for (const [name, button] of buttons) {
      button.classList.toggle('active', name === pressed);
      button.setAttribute('aria-pressed', String(name === pressed));
    }
  }

  /**
   * Opens tool `k` for the person (rail, link). While a sandbox's page loads and no sandbox is
   * the context yet, it waits for that sandbox, the one whose page started loading last: opened
   * now it would be the tool of no sandbox ("Open a sandbox first.", the Global Superagent), shown
   * again for the sandbox as its context's, without the keyboard (SPEC-170 TKS-002, SPEC-428
   * SAT-005).
   */
  function open(k, opts = {}) {
    if (sandbox || !arriving.size) { show(k, opts); return; }
    chosen = { kind: k, opts, sandbox: [...arriving].pop() };
    press();
  }

  function close() {
    panel.hidden = true;
    const app = rail.closest('.app');
    if (app) app.classList.remove('panel-max');
    if (ctx.fitSidebar) ctx.fitSidebar(); // the sidebar grows back to its width
    kind = null;
    if (sandbox) toolOf.set(sandbox.id, null);
    press();
    for (const view of views.values()) if (view.hide) view.hide();
    if (railForPanel) {
      railForPanel = false;
      setRailVisible(false, { persist: false });
    }
  }

  /** A tool opened from the rail itself makes the shown rail the person's Tools choice. */
  function railClick(k) {
    const opening = kind !== k || panel.hidden;
    if (railForPanel && opening) setRailVisible(true);
    toggle(k);
  }

  function toggle(k, opts) {
    // Chosen again while it waits for its sandbox: not wanted after all.
    if (chosen && chosen.kind === k) { chosen = null; press(); }
    else if (kind === k && !panel.hidden) close();
    else open(k, opts);
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

  /**
   * Destroys the views of these sandboxes and forgets their tool. An editor holding unsaved text
   * is kept past the bound (the person was told it stays in this tab) until its sandbox is gone.
   */
  function dropSandboxes(ids, { gone = false } = {}) {
    const dropped = new Set(ids);
    destroyWhere((key) => dropped.has(keySandbox(key)) && (gone || !(views.get(key).unsaved)));
    if (gone) for (const id of dropped) toolOf.delete(id);
  }

  /** A sandbox removed, or no longer visible to the person: its views and sockets go (KST-006). */
  function pruneGone() {
    const known = new Set(ctx.store.state.sandboxes.map((s) => s.id));
    const gone = [...new Set([...views.keys()].map(keySandbox))].filter((id) => id !== 'none' && !known.has(id));
    recent = recent.filter((id) => known.has(id));
    if (gone.length) dropSandboxes(gone, { gone: true });
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
      // Outside a sandbox only the Superagent, the Global one, is there (SPEC-427 GSA-006); the
      // tools of a sandbox whose page loads can be chosen already.
      button.disabled = !sandbox && !arriving.size && k !== 'agent';
      // Readers still edit the team's notes.
      button.classList.toggle('readonly', Boolean(sandbox) && !write && k !== 'notes');
    }
  }

  const offs = [
    ctx.store.on('browser-action', updateBadges),
    ctx.store.on('sandboxes', pruneGone),
    ctx.store.on('task', (t) => { if (task && t.id === task.id) { task = t; updateBadges(); } }),
  ];

  return {
    rail,
    panel,
    /** Sign-out: drop every tool view (terminals, browser, editor) and unsubscribe. */
    destroy() {
      offs.forEach((off) => off());
      window.removeEventListener('resize', refit);
      if (ctx.refitPanel === refit) ctx.refitPanel = null;
      destroyWhere(() => true);
      if (globalAgent) globalAgent.destroy();
      globalAgent = null;
    },
    get railVisible() { return railVisible; },
    setRailVisible,
    /** Applies the remembered rail visibility once the rail is in the document. */
    mounted() { setRailVisible(railVisible, { persist: false }); },
    get kind() { return panel.hidden ? null : kind; },
    /** A sandbox's page started loading: setContext comes once it has, unless the person left first. */
    arrive(next) {
      // Left for another sandbox: the tool that waited for this one's page is not wanted there
      // (what it was opened with, a tab or a file, is this sandbox's).
      if (chosen && chosen.sandbox !== next.id) { chosen = null; press(); }
      arriving.delete(next.id); // the one arriving last is last
      arriving.add(next.id);
      if (!sandbox) updateBadges();
    },
    setContext(nextSandbox, nextTask) {
      // A sandbox left while its page loaded never was the context: it comes back with no tool,
      // unless it kept one from an earlier visit (SPEC-170 TKS-001, #489).
      for (const id of arriving) {
        if (id !== (nextSandbox && nextSandbox.id) && id !== (sandbox && sandbox.id) && !toolOf.has(id)) toolOf.set(id, null);
      }
      arriving.clear();
      const wish = chosen;
      chosen = null;
      const sandboxChanged = (nextSandbox && nextSandbox.id) !== (sandbox && sandbox.id);
      const taskChanged = (nextTask && nextTask.id) !== (task && task.id);
      const left = sandbox;
      sandbox = nextSandbox;
      task = nextTask;
      if (sandboxChanged) {
        // The sandbox left keeps its views, hidden (the browser stream stops, the editor lets its
        // Monaco models go); its notes are rebuilt when it is back. Past KEEP_SANDBOXES the views of
        // the sandbox used longest ago go (SPEC-113 KST-006).
        if (left) {
          // What it shows when it is back, also when nothing was open (KST-005).
          toolOf.set(left.id, panel.hidden ? null : kind);
          for (const [key, view] of views) {
            if (keySandbox(key) !== left.id) continue;
            if (view.hide) view.hide();
            if (view.suspend) view.suspend();
          }
          destroyWhere((key) => keySandbox(key) === left.id && !KEPT_KINDS.has(key.slice(0, key.indexOf(':'))));
        }
        if (sandbox) recent = [sandbox.id, ...recent.filter((id) => id !== sandbox.id)];
        dropSandboxes(recent.splice(KEEP_SANDBOXES));
      }
      if (sandbox && (sandboxChanged || taskChanged)) {
        const editor = views.get(`editor:${sandbox.id}`);
        if (editor && editor.taskChanged) editor.taskChanged();
      }
      // The Global Superagent in front: it stays open from page to page, in and out of sandboxes,
      // and a sandbox entered starts no session of its own (SPEC-428 SAT-005).
      const globalOpen = !panel.hidden && kind === 'agent' && (!left || agentTab === 'global');
      if (wish && sandbox) {
        // Chosen while this sandbox's page loaded (`arrive` ended the wait for any other):
        // opened now, as the person asked then.
        show(wish.kind, wish.opts);
      } else if (!sandbox) {
        if (globalOpen && sandboxChanged) show('agent', { context: true });
        else if (!panel.hidden && kind !== 'agent') close();
        else if (!panel.hidden && sandboxChanged) close();
      } else if (globalOpen && sandboxChanged) {
        show('agent', { context: true, tab: 'global' });
      } else if (sandboxChanged && toolOf.has(sandbox.id)) {
        // Back in a sandbox: the tool it had open, or none (KST-005). A rail the person hid is shown
        // for that tool only, as for a Superagent link.
        const tool = toolOf.get(sandbox.id);
        if (tool) show(tool, { context: true, persistRail: false });
        else if (!panel.hidden) close();
      // Shown again for the new context, not opened by the person: the keyboard stays with the page.
      } else if (!panel.hidden && kind && (sandboxChanged || taskChanged)) show(kind, { context: true });
      // A tool chosen for a sandbox left for a page outside one is not pressed any more.
      if (wish) press();
      updateBadges();
    },
    open,
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
 * tabs stay attached, so switching tabs is instant. The tabs move by drag, Alt+←/→ and their menu,
 * in an order this browser keeps per sandbox; a new terminal comes last (SPEC-434 MTB-002).
 */
function createShellView(ctx, sandbox, getTask) {
  const el = h('div', { class: 'panel-view' });
  if (!allows(sandbox.access, 'write')) {
    setChildren(el, h('div', { class: 'panel-msg' }, icon('lock', { size: 20 }), h('p', null, 'Terminals need write access to the repository on GitHub.')));
    return { el };
  }
  const activeKey = `yulius.shellActive.${sandbox.id}`;
  const orderKey = `yulius.shellOrder.${sandbox.id}`;
  const tablist = h('div', { class: 'term-tabs', role: 'tablist', 'aria-label': 'Terminals' });
  const views = h('div', { class: 'term-views' });
  setChildren(el, tablist, views);
  /** Terminals in tab order: {id, name, task_id, created_at, view, slot, ended}. */
  let shells = [];
  let active = null;
  let loaded = false;
  let shown = false;
  // Shown by the person (it takes the keyboard), not again for a new page (SPEC-170 TKS-002).
  let focusWanted = false;
  let destroyed = false;
  let where = '';
  let renaming = null;
  let tabsStale = false; // a redraw held back while a tab is pressed
  const drag = createTabDrag(tablist, {
    keyOf: (tab) => tab.dataset.id,
    commit: (order) => reorder(order),
    cancelled: () => { if (tabsStale) renderTabs(); },
  });

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

  /** Draws the strip, unless a tab is pressed (then once it is let go); answers the tabs drawn. */
  function renderTabs() {
    if (drag.dragging) { tabsStale = true; return []; }
    tabsStale = false;
    const focused = tablist.contains(document.activeElement) ? document.activeElement.dataset.id : null;
    const items = shells.map((shell) => {
      const selected = shell.id === active;
      const name = renaming === shell.id
        ? renameInput(shell)
        : h('span', { class: 'term-tab-name' }, shell.name);
      const tab = h('div', { class: ['term-tab', selected && 'active', shell.ended && 'term-tab-ended'], role: 'tab', id: `term-tab-${shell.id}`, tabindex: selected ? '0' : '-1', 'aria-selected': String(selected), 'aria-controls': shell.slot ? `term-panel-${shell.id}` : null, dataset: { id: shell.id }, title: `${shell.name} · ${folderLabel(shell)} — double-click or F2 to rename\n${DRAG_TAB_HINT}`, 'aria-keyshortcuts': 'F2 Delete Backspace Alt+ArrowLeft Alt+ArrowRight' },
        h('span', { class: 'tab-grip', 'aria-hidden': 'true' }, icon('grip', { size: 12 })),
        name,
        // Not a button (see closableTabKeys): Delete or Backspace closes the focused tab.
        h('span', { class: 'term-tab-close', 'aria-hidden': 'true', title: `Close ${shell.name}`, onClick: (event) => { event.stopPropagation(); closeShell(shell); } }, '×'));
      tab.addEventListener('click', () => { if (!drag.clickConsumed() && renaming !== shell.id) activate(shell.id, { focus: true }); });
      // × closes and a tab being renamed edits its name: neither drags.
      tab.addEventListener('pointerdown', (event) => { if (renaming !== shell.id && !event.target.closest('.term-tab-close')) drag.press(event, tab); });
      tab.addEventListener('contextmenu', (event) => {
        if (renaming === shell.id) return;
        event.preventDefault();
        const index = shells.indexOf(shell);
        // Moving a tab without dragging (WCAG 2.5.7), as the Notes and sandbox tabs do.
        openMenu({ x: event.clientX, y: event.clientY }, [
          { label: 'Move left', icon: 'left', disabled: index === 0, onSelect: () => move(shell, -1) },
          { label: 'Move right', icon: 'right', disabled: index === shells.length - 1, onSelect: () => move(shell, 1) },
          { separator: true },
          { label: 'Rename', disabled: shell.ended, onSelect: () => startRename(shell) },
          { label: 'Close', onSelect: () => closeShell(shell) },
        ]);
      });
      tab.addEventListener('dblclick', () => startRename(shell));
      tab.addEventListener('auxclick', (event) => { if (event.button === 1) closeShell(shell); });
      tab.addEventListener('keydown', (event) => {
        if (renaming === shell.id) return;
        if (event.key === 'F2') { event.preventDefault(); startRename(shell); return; }
        if (event.altKey && !event.ctrlKey && !event.metaKey && (event.key === 'ArrowLeft' || event.key === 'ArrowRight')) {
          event.preventDefault();
          move(shell, event.key === 'ArrowLeft' ? -1 : 1);
          return;
        }
        closableTabKeys(event, { select: () => activate(shell.id, { focus: true }), close: () => closeShell(shell, { fromKeyboard: true }) });
      });
      return tab;
    });
    // The strip stays when it is empty: a terminal appearing in it never resizes the others.
    setChildren(tablist, items);
    if (focused && renaming === null) (items.find((item) => item.dataset.id === focused) || items.find((item) => item.tabIndex === 0))?.focus();
    return items;
  }

  /** Puts the tabs in `order` (terminal ids) and keeps it for this sandbox. */
  function reorder(order) {
    shells = inOrder(shells, order, (shell) => shell.id);
    writeJson(orderKey, shells.map((shell) => shell.id));
    return renderTabs();
  }

  /** Moves `shell` one place left (-1) or right (1); the keyboard stays on its tab. */
  function move(shell, step) {
    const order = movedBy(shells.map((s) => s.id), shell.id, step);
    if (order) reorder(order);
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
    // A closed terminal leaves the order kept (its id never comes back).
    if (readJson(orderKey, null)) writeJson(orderKey, shells.map((s) => s.id));
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
      shells = inOrder(list.map((shell) => ({ ...shell })), readJson(orderKey, null), (shell) => shell.id);
      if (!shells.length) {
        renderTabs();
        await create({ focus: shown && focusWanted });
        return;
      }
      const remembered = readJson(activeKey, null);
      const pick = shells.find((shell) => shell.id === remembered) || shells[shells.length - 1];
      activate(pick.id, { focus: shown && focusWanted });
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
    show(opts = {}) {
      shown = true;
      // Shown again for a new task or sandbox, not opened by the person: the keyboard stays with
      // the page, as for the Superagent and Notes (SPEC-056, #439).
      focusWanted = !opts.context;
      if (!loaded) { load(); return; }
      const shell = shells.find((s) => s.id === active);
      if (shell && shell.view) setTimeout(() => { shell.view.refit(); if (focusWanted) shell.view.focus(); }, 0);
      // Opened again after its last terminal was closed: a new one, as on the first opening.
      else if (!shells.length) create({ focus: focusWanted });
    },
    hide() { shown = false; },
    destroy() {
      destroyed = true;
      clearTimeout(syncTimer);
      for (const shell of shells) if (shell.view) shell.view.destroy();
    },
  };
}

