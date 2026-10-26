// The Superagent (SPEC-074 SUP-007; the Agent of SPEC-035, formerly the Cockpit of SPEC-021
// CKP-008): the user's own plain Claude Code session in this sandbox, in the right tool panel beside
// the page, for quick work that should not become a task. One per user and sandbox, opened (or started, resuming its last conversation) by
// POST /api/sandboxes/{sid}/cockpit; its terminal is the shell terminal protocol. There is no way
// to delete it: /clear starts over inside it.

import { enc } from './api.js';
import { h, icon, setChildren, emptyState, loadError, skeleton, toast, errorToast } from './dom.js';
import { createTerminalView } from './terminal.js';
import { allows } from './status.js';
import { href } from './router.js';
import { attachTerminalDrop, shellAttachments } from './filedrop.js';
import { isPhone } from './theme.js';

/** The Superagent's panel view: its quick-work notice above the session. Starts on the first show. */
export function createAgentView(ctx, sandbox) {
  const write = allows(sandbox.access, 'write');
  // On phones the panel covers the page: it closes so the composer shows.
  const newTask = write ? h('a', { href: href(`/s/${encodeURIComponent(sandbox.slug)}`), dataset: { route: `/s/${encodeURIComponent(sandbox.slug)}` }, onClick: () => { if (isPhone() && ctx.closePanel) ctx.closePanel(); } }, 'New task') : null;
  // AGT-002: what the Superagent is for, and where bigger work goes.
  const notice = h('div', { class: 'banner banner-info agent-note small', role: 'note', 'aria-label': 'About the Superagent' },
    icon('info', { size: 16 }),
    h('span', { class: 'banner-text' },
      h('strong', null, 'For quick work'), ': setting up automations, asking a quick question. Anything more complex, such as writing code, belongs in a task, not here', newTask ? [': ', newTask, '.'] : '.'));
  const body = h('div', { class: 'agent-term' });
  const requests = createRequests(ctx, sandbox);
  const el = h('div', { class: 'agent' }, notice, requests.el, body);
  let terminal = null;
  let alive = true;
  let opening = false;
  let started = false;
  let shown = false;
  let current = null;
  let recovering = false;
  /** Whether the last mount was to take the keyboard, and from where (for a recovery after it). */
  let asked = { focus: false, from: null };

  function drop() {
    if (terminal) terminal.destroy();
    terminal = null;
  }

  /** Opens the session; `focus` only when the person asked for the Superagent (not a new page or sandbox). */
  async function open({ focus: wantFocus = true } = {}) {
    if (opening || !alive) return;
    // Where the keyboard was: a slow start does not take it once the person has moved on.
    const from = document.activeElement;
    opening = true;
    drop();
    setChildren(body, skeleton('Starting your Superagent…', 3, { block: true }));
    let shell;
    try {
      shell = await ctx.api.post(`/api/sandboxes/${enc(sandbox.id)}/cockpit`);
    } catch (err) {
      opening = false;
      if (alive) setChildren(body, h('div', { class: 'agent-msg' }, loadError(`Couldn’t start the Superagent: ${err.message}`, () => open())));
      return;
    }
    opening = false;
    if (alive) mount(shell, { focus: wantFocus && stayed(from), from });
  }

  /**
   * The terminal lost its session (refused as not found, or the connection dropped): ask for the
   * Superagent again, which starts it when its process is gone (a reboot), and follow it to its new id.
   * The same id means it still runs; the terminal reconnects by itself.
   */
  async function recover() {
    if (recovering || opening || !alive || !current) return;
    recovering = true;
    try {
      const shell = await ctx.api.post(`/api/sandboxes/${enc(sandbox.id)}/cockpit`);
      // The keyboard stays where it is: in the Superagent if it was there (a refused session's terminal
      // takes it when shown, before it is refused), or was going there when the connection dropped
      // before the first screen; else with the page beside it.
      const keep = el.contains(document.activeElement) || (asked.focus && stayed(asked.from));
      if (alive && current && shell.id !== current.id) mount(shell, { focus: keep, from: asked.from });
    } catch {
      // The terminal shows its own connection state and Retry.
    } finally {
      recovering = false;
    }
  }

  /** The keyboard is still where it was when the start began (or nowhere, or in the Superagent). */
  const stayed = (from) => {
    const now = document.activeElement;
    return now === from || !now || now === document.body || el.contains(now);
  };

  function focus() {
    // On phones a focused terminal raises the soft keyboard over the page (as in the task view).
    if (terminal && shown && !isPhone()) terminal.focus();
  }

  function mount(shell, { focus: wantFocus = true, from = null } = {}) {
    drop();
    asked = { focus: wantFocus, from };
    current = shell;
    terminal = createTerminalView({
      transport: ctx.transport,
      path: `/api/shells/${enc(shell.id)}/terminal`,
      onLink: (uri, event) => ctx.openLink(uri, event),
      readOnlyLabel: () => 'Read-only',
      // A usage limit moved it to another subscription (SPEC-074 SUP-006), or an update restarts
      // it to get new tools (SPEC-186 SAV-006): it goes on, so the panel follows it instead of
      // offering Start again.
      exitMessage: (exit) => (exit.status === 'switched'
        ? { tone: 'info', text: 'Restarting the Superagent; its conversation continues…' }
        : { tone: 'info', text: 'The Superagent stopped. Its conversation is kept.' }),
      exitActions: (exit) => (exit.status === 'switched' ? [] : [{ label: 'Start again', run: () => open() }]),
      // Opened again rather than recovered: the server answers once the new one runs (it holds the
      // start), and a failure shows why with Try again instead of leaving "Moving…" on screen.
      onExit: (exit) => { if (exit.status === 'switched') open({ focus: el.contains(document.activeElement) }); },
      onFatal: (message) => { if (message.code === 'not_found') recover(); },
      // Once its first screen shows, the wish to focus from opening it is spent: a later recovery
      // leaves the keyboard where it is.
      onReady: () => { asked = { focus: false, from: null }; },
      onConnection: (status) => { if (status === 'reconnecting' || status === 'failed') recover(); },
      // Paths in its output open in the Editor, as in a terminal of the sandbox repository.
      fileLinks: {
        list: (dir) => ctx.api.get(`/api/sandboxes/${enc(sandbox.id)}/files?root=sandbox&path=${encodeURIComponent(dir)}`).then((listing) => listing.entries || []),
        roots: () => (shell.folder ? [shell.folder] : []),
        open: (file) => ctx.openFile && ctx.openFile(sandbox, { root: 'sandbox', ...file }),
      },
    });
    // Files dropped here (or a pasted image) reach Claude by path, as in a task's terminal.
    attachTerminalDrop(ctx, terminal, { label: 'Drop to send to Claude', upload: shellAttachments(ctx, sandbox.id, shell.id) });
    setChildren(body, terminal.el);
    terminal.start();
    if (wantFocus) focus();
  }

  if (!write) {
    setChildren(body, h('div', { class: 'agent-msg' }, emptyState({
      iconName: 'spark',
      title: 'The Superagent needs write access to this sandbox.',
      text: 'It runs Claude Code in the sandbox as you; GitHub gives you read access to this repository.',
    })));
  }

  return {
    el,
    show(opts = {}) {
      shown = true;
      // The session starts the first time the panel shows it, not when a sandbox opens.
      if (write && !started) { started = true; open({ focus: !opts.context }); return; }
      // Shown again for a new page (`context`), it only refits: the keyboard stays with the page.
      if (terminal) setTimeout(() => { if (terminal) { terminal.refit(); if (!opts.context) focus(); } }, 0);
    },
    hide() { shown = false; },
    destroy() {
      alive = false;
      requests.destroy();
      drop();
    },
  };
}

/** Start options a request shows, in the composer's words. */
const OPTION_LABELS = {
  workflow: (v) => `Workflow: ${v}`,
  mode: (v) => `Mode: ${v}`,
  after_task: (v) => `After #${v}`,
  start_rule: (v) => `When ${v}`,
  supertask: (v) => (v ? 'Supertask: runs alone' : null),
  prepared: (v) => (v ? 'Prepared: waits for Start now' : null),
  start_at: (v) => `Starts at ${v}`,
  model: (v) => `Model: ${v}`,
  effort: (v) => `Effort: ${v}`,
};

/**
 * SPEC-335 XSR-008: what the person's Superagent asks to start in another sandbox, above its
 * session. Nothing reaches that sandbox until the person presses Create task; each request is
 * answered once, and goes when the Superagent's call ends.
 */
function createRequests(ctx, sandbox) {
  const el = h('div', { class: 'agent-requests', hidden: true });
  const path = `/api/sandboxes/${enc(sandbox.id)}/superagent/requests`;
  let alive = true;
  let reading = null;
  let again = false;
  const busy = new Set();

  async function load() {
    if (!allows(sandbox.access, 'write')) return;
    // One read at a time; a change announced during it brings one more.
    if (reading) { again = true; return; }
    reading = ctx.api.get(path).then(render, () => { /* the next event or resync reads again */ });
    try { await reading; } finally {
      reading = null;
      if (again && alive) { again = false; load(); }
    }
  }

  function render(list) {
    if (!alive) return;
    const items = Array.isArray(list) ? list : [];
    el.hidden = items.length === 0;
    setChildren(el, items.map(card));
  }

  function card(request) {
    const target = request.sandbox || {};
    const options = Object.entries(request.options || {})
      .map(([key, value]) => (OPTION_LABELS[key] ? OPTION_LABELS[key](value) : null))
      .filter(Boolean);
    const heading = `Your Superagent asks to start a task in ${target.slug}`;
    const pending = busy.has(request.id);
    const answer = (approve) => respond(request, approve);
    return h('section', { class: 'agent-request banner banner-warn small', role: 'group', 'aria-label': heading },
      h('div', { class: 'agent-request-head' }, icon('spark', { size: 16 }),
        h('strong', null, 'Your Superagent asks to start a task in ', h('span', { class: 'agent-request-target' }, target.slug)),
        target.repository ? h('span', { class: 'muted' }, ` (${target.repository})`) : null),
      request.title ? h('div', { class: 'agent-request-title' }, request.title) : null,
      h('pre', { class: 'agent-request-prompt', tabindex: '0', 'aria-label': 'Prompt' }, request.prompt || ''),
      options.length ? h('ul', { class: 'agent-request-options', 'aria-label': 'Start options' }, options.map((o) => h('li', null, o))) : null,
      h('div', { class: 'agent-request-actions' },
        h('button', { type: 'button', class: 'btn btn-sm btn-primary', disabled: pending, onClick: () => answer(true) }, 'Create task'),
        h('button', { type: 'button', class: 'btn btn-sm', disabled: pending, onClick: () => answer(false) }, 'Decline')));
  }

  async function respond(request, approve) {
    if (busy.has(request.id)) return;
    busy.add(request.id);
    const slug = request.sandbox && request.sandbox.slug;
    try {
      const answer = await ctx.api.post(`${path}/${enc(request.id)}`, { approve });
      if (approve && answer && answer.task) {
        const to = `/s/${encodeURIComponent(answer.sandbox || slug)}/t/${answer.task.number}`;
        toast(`Created #${answer.task.number} in ${answer.sandbox || slug}.`, { kind: 'ok', action: { label: 'Open', run: () => ctx.navigate(to) } });
      } else if (!approve) {
        toast('Declined: nothing was created.', { kind: 'info' });
      }
    } catch (err) {
      errorToast(err, approve ? `Couldn’t create the task in ${slug}` : 'Couldn’t decline');
    } finally {
      busy.delete(request.id);
      load();
    }
  }

  const offs = [
    ctx.store.on('superagent-requests', (event) => { if (event && event.sandbox_id === sandbox.id) load(); }),
    ctx.store.on('resync', () => load()),
  ];
  load();

  return {
    el,
    destroy() {
      alive = false;
      for (const off of offs) if (typeof off === 'function') off();
    },
  };
}
