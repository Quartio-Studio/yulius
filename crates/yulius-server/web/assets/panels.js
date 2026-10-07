// Right rail (Browser, Editor, Shell, Messages) and its resizable panel, collapsed by
// default (UI-002, UI-015). Sandbox-scoped panels stay alive while hidden so the
// browser stream, VS Code and shell sessions are not torn down on every toggle.

import { enc } from './api.js';
import { h, icon, iconButton, setChildren, errorToast, placeholder } from './dom.js';
import { createTerminalView } from './terminal.js';
import { createBrowserPanel } from './browser.js';
import { messageView } from './actions.js';
import { allows } from './status.js';
import { readJson, writeJson } from './store.js';

const WIDTH_KEY = 'yulius.panelWidth';
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
    const button = h('button', { type: 'button', class: 'btn-icon rail-btn', 'aria-label': label, title: label, 'aria-pressed': 'false', onClick: () => toggle(kind) }, icon(iconName, { size: 18 }));
    const dot = h('span', { class: kind === 'messages' ? 'rail-badge' : 'rail-dot', hidden: true });
    button.append(dot);
    buttons.set(kind, button);
    badges.set(kind, dot);
    rail.append(button);
  }

  const title = h('span', { class: 'panel-title' });
  const headExtra = h('div', { class: 'row' });
  const body = h('div', { class: 'panel-body' });
  const handle = h('div', { class: 'panel-resize', role: 'separator', 'aria-orientation': 'vertical', 'aria-label': 'Resize panel', tabindex: '0' });
  const panel = h('section', { class: 'panel', hidden: true, 'aria-label': 'Tool panel' },
    handle,
    h('div', { class: 'panel-head' }, title, headExtra, iconButton('x', 'Close panel', () => close())),
    body);

  let kind = null;
  let sandbox = null;
  let task = null;
  const views = new Map(); // key → view

  // ---- width (default 45 % of the window, resizable, remembered per browser)
  function defaultWidth() {
    const saved = Number(readJson(WIDTH_KEY, 0));
    const fraction = saved > 0.15 && saved < 0.85 ? saved : 0.45;
    return Math.round(window.innerWidth * fraction);
  }
  function applyWidth(px) {
    const max = Math.max(320, window.innerWidth - 360);
    const width = Math.max(300, Math.min(max, px));
    panel.style.width = `${width}px`;
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
      writeJson(WIDTH_KEY, panel.getBoundingClientRect().width / window.innerWidth);
    };
    handle.addEventListener('pointermove', move);
    handle.addEventListener('pointerup', up);
  });
  handle.addEventListener('keydown', (event) => {
    if (event.key !== 'ArrowLeft' && event.key !== 'ArrowRight') return;
    event.preventDefault();
    const width = applyWidth(panel.getBoundingClientRect().width + (event.key === 'ArrowLeft' ? 40 : -40));
    writeJson(WIDTH_KEY, width / window.innerWidth);
  });

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
    if (panel.hidden) {
      panel.hidden = false;
      if (!panel.style.width) applyWidth(defaultWidth());
    }
    for (const [name, button] of buttons) {
      button.classList.toggle('active', name === k);
      button.setAttribute('aria-pressed', String(name === k));
    }
    if (view.show) view.show(opts);
    ctx.onPanelChange && ctx.onPanelChange(k);
  }

  function close() {
    panel.hidden = true;
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
    msgBadge.textContent = unread ? String(unread) : '';
    const requests = sandbox ? ctx.store.state.actionRequests.get(sandbox.id) : null;
    badges.get('browser').hidden = !(requests && requests.size);
    buttons.get('browser').title = requests && requests.size ? 'Browser — an agent needs your action' : 'Browser';
    const write = sandbox && allows(sandbox.access, 'write');
    for (const k of ['browser', 'editor', 'shell']) {
      const button = buttons.get(k);
      button.disabled = !sandbox;
      button.classList.toggle('readonly', Boolean(sandbox) && !write);
    }
    buttons.get('messages').disabled = !sandbox;
  }

  ctx.store.on('browser-action', updateBadges);
  ctx.store.on('task', (t) => { if (task && t.id === task.id) { task = t; updateBadges(); } });

  return {
    rail,
    panel,
    get kind() { return panel.hidden ? null : kind; },
    setContext(nextSandbox, nextTask) {
      const sandboxChanged = (nextSandbox && nextSandbox.id) !== (sandbox && sandbox.id);
      const taskChanged = (nextTask && nextTask.id) !== (task && task.id);
      sandbox = nextSandbox;
      task = nextTask;
      if (sandboxChanged) destroyWhere(() => true);
      else if (taskChanged) destroyWhere((key) => key.startsWith('messages:') || key.startsWith('editor:'));
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

// ---------------------------------------------------------------- editor

/**
 * VS Code iframe (/s/{sid}/editor/), lazily loaded, task worktree selectable (EDT-002).
 * `?worktree=<task id>` makes the server redirect to that task's worktree folder. A HEAD
 * preflight turns the server's 503 `unavailable` (openvscode-server missing or disabled in
 * contained mode) and 409 (repository not cloned yet) into a readable message instead of a
 * JSON error page inside the iframe.
 */
function createEditorView(ctx, sandbox, getTask) {
  const el = h('div', { class: 'panel-view' });
  if (!allows(sandbox.access, 'write')) {
    setChildren(el, h('div', { class: 'panel-msg' }, icon('lock', { size: 20 }), h('p', null, 'The editor requires write access on GitHub.')));
    return { el };
  }
  let frame = null;
  let checking = false;
  const task = getTask();
  const base = `/s/${enc(sandbox.id)}/editor/`;
  const target = h('select', { class: 'select', style: { height: '24px', width: 'auto', fontSize: '12px' }, 'aria-label': 'Folder' },
    h('option', { value: '' }, 'Sandbox'),
    task ? h('option', { value: 'task' }, `Task #${task.number} worktree`) : null);
  const src = () => (target.value === 'task' && task ? `${base}?worktree=${encodeURIComponent(task.id)}` : base);
  target.addEventListener('change', () => { if (frame) frame.src = src(); });
  const reload = iconButton('refresh', 'Reload editor', () => { if (frame) frame.src = src(); else load(); });
  const external = iconButton('external', 'Open editor in a new tab', () => window.open(src(), '_blank', 'noopener'));

  async function preflight() {
    try {
      const response = await window.fetch(base, { method: 'HEAD', credentials: 'same-origin', cache: 'no-store', redirect: 'manual' });
      if (response.status === 503) return 'The editor is not available on this server (openvscode-server is missing, or disabled in contained mode unless the server runs with --editor-uncontained).';
      if (response.status === 409) return 'The editor opens once the sandbox repository has been cloned.';
      if (response.status === 401 || response.status === 403) return 'You do not have access to the editor of this sandbox.';
      return null;
    } catch {
      return null; // network hiccup: let the iframe try
    }
  }

  async function load() {
    if (frame || checking) return;
    checking = true;
    setChildren(el, placeholder('Opening the editor…', 'loading'));
    const problem = ctx.mock ? null : await preflight();
    checking = false;
    if (problem) {
      setChildren(el, h('div', { class: 'panel-msg stack', style: { alignItems: 'center' } }, icon('warn', { size: 20 }), h('p', null, problem),
        h('button', { type: 'button', class: 'btn btn-sm', onClick: load }, 'Try again')));
      return;
    }
    frame = h('iframe', { class: 'editor-frame', title: `VS Code — ${sandbox.name}`, allow: 'clipboard-read; clipboard-write', referrerpolicy: 'same-origin' });
    if (ctx.mock) frame.srcdoc = mockEditorDoc(sandbox);
    else frame.src = src();
    setChildren(el, frame);
  }

  return {
    el,
    headExtra: () => [target, reload, external],
    show() { load(); },
  };
}

function mockEditorDoc(sandbox) {
  const name = String(sandbox.name).replace(/[^\w .-]/g, '');
  return `<!doctype html><meta charset="utf-8"><body style="margin:0;font:13px system-ui;background:#1e1e1e;color:#ccc;display:grid;grid-template-columns:48px 220px 1fr;height:100vh">
<div style="background:#333"></div><div style="background:#252526;padding:10px"><div style="opacity:.7;font-size:11px;text-transform:uppercase">Explorer — ${name}</div>
<div style="margin-top:8px;line-height:1.9">▸ docs<br>▸ src<br>&nbsp;&nbsp;main.rs<br>Cargo.toml<br>AGENTS.md</div></div>
<div style="padding:16px;font-family:ui-monospace,Menlo,monospace;white-space:pre;color:#9cdcfe">// Mock mode: openvscode-server is served at /s/{sandbox}/editor/ by the real server.
<span style="color:#c586c0">fn</span> <span style="color:#dcdcaa">main</span>() {
    println!(<span style="color:#ce9178">"hello from ${name}"</span>);
}</div></body>`;
}

// ---------------------------------------------------------------- shell

/**
 * General sandbox shell (UI-015): separate from the agent terminal, write access only.
 * `POST /api/sandboxes/{sid}/shells` with an empty body opens it in the sandbox repository,
 * `{"task_id"}` in that task's worktree; at most 4 live shells per sandbox (429).
 */
function createShellView(ctx, sandbox, getTask) {
  const el = h('div', { class: 'panel-view' });
  if (!allows(sandbox.access, 'write')) {
    setChildren(el, h('div', { class: 'panel-msg' }, icon('lock', { size: 20 }), h('p', null, 'The sandbox shell requires write access on GitHub.')));
    return { el };
  }
  let terminal = null;
  let starting = false;
  let where = '';
  const startButton = h('button', { type: 'button', class: 'btn btn-primary' }, 'Open shell');
  const idle = h('div', { class: 'panel-msg stack', style: { alignItems: 'center' } },
    h('p', null, 'A login shell in this sandbox with the containment of task runs. It never changes tasks or replaces the agent terminal.'),
    startButton);
  setChildren(el, idle);

  function folderSelect() {
    const task = getTask();
    if (!task && where === 'task') where = '';
    const select = h('select', { class: 'select', style: { height: '24px', width: 'auto', fontSize: '12px' }, 'aria-label': 'Shell folder' },
      h('option', { value: '' }, 'Sandbox repository'),
      task ? h('option', { value: 'task' }, `Task #${task.number} worktree`) : null);
    select.value = where;
    select.addEventListener('change', () => { where = select.value; });
    return select;
  }

  async function open() {
    if (starting) return;
    starting = true;
    startButton.classList.add('busy');
    try {
      const task = where === 'task' ? getTask() : null;
      const shell = task
        ? await ctx.api.post(`/api/sandboxes/${enc(sandbox.id)}/shells`, { task_id: task.id })
        : await ctx.api.post(`/api/sandboxes/${enc(sandbox.id)}/shells`);
      if (terminal) terminal.destroy();
      terminal = createTerminalView({
        transport: ctx.transport,
        path: `/api/shells/${enc(shell.id)}/terminal`,
        onLink: (uri, event) => ctx.openLink(uri, event),
        readOnlyLabel: () => 'Read-only',
        exitActions: () => [{ label: 'New shell', run: open }],
      });
      setChildren(el, terminal.el);
      terminal.start();
      terminal.focus();
    } catch (error) {
      errorToast(error, error.status === 429 ? 'Too many shells' : 'Could not open a shell');
    } finally {
      starting = false;
      startButton.classList.remove('busy');
    }
  }
  startButton.addEventListener('click', open);
  return {
    el,
    headExtra: () => [folderSelect(), iconButton('plus', 'New shell', open)],
    show() { if (terminal) setTimeout(() => terminal.refit(), 0); },
    destroy() { if (terminal) terminal.destroy(); },
  };
}

// ---------------------------------------------------------------- messages

/** Task messages (COM-005: users see messages) with a send box. */
function createMessagesView(ctx, sandbox, task) {
  const list = h('div', { class: 'msg-list', 'aria-live': 'polite' }, placeholder('Loading messages…', 'loading'));
  const input = h('textarea', { class: 'textarea', rows: 3, placeholder: `Message #${task.number} — delivered to the agent like a task message`, 'aria-label': 'Message', maxlength: 20000 });
  const send = h('button', { type: 'button', class: 'btn btn-primary btn-sm' }, icon('send', { size: 13 }), 'Send');
  const write = allows(sandbox.access, 'write');
  const compose = write
    ? h('div', { class: 'msg-compose' }, input, h('div', { class: 'row' }, h('span', { class: 'muted xs' }, 'Ctrl/⌘ + Enter to send'), h('span', { class: 'spacer' }), send))
    : h('div', { class: 'msg-compose muted small' }, 'Read-only (GitHub: read): you can read messages but not send them.');
  const el = h('div', { class: 'panel-view' }, list, compose);
  const messages = new Map();
  const myLogin = ctx.store.state.me && ctx.store.state.me.user ? ctx.store.state.me.user.login : null;

  function render() {
    const sorted = [...messages.values()].sort((a, b) => String(a.created_at).localeCompare(String(b.created_at)));
    if (sorted.length === 0) { setChildren(list, placeholder('No messages yet. Tasks message each other here; you can message the agent too.')); return; }
    const atBottom = list.scrollTop + list.clientHeight >= list.scrollHeight - 24;
    setChildren(list, sorted.map((m) => messageView(ctx, m, myLogin)));
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
      setChildren(list, placeholder(error.message, 'error'));
    }
  }

  async function submit() {
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
  }

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

