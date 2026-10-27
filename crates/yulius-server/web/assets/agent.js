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

/** How long a new request's buttons wait before they take a click (one aimed at the terminal it pushed down). */
const ARM_MS = 600;
/** Characters a person cannot see that could hide or reorder a prompt's words: shown by their code. */
const INVISIBLE = /(?![\n\t])[\p{Cc}\p{Cf}\p{Co}\p{Cn}\u{034F}\u{115F}\u{1160}\u{17B4}\u{17B5}\u{180B}-\u{180F}\u{3164}\u{FFA0}\u{FE00}-\u{FE0F}\u{E0100}-\u{E01EF}]/gu;

/** A request's text as a person can check it: invisible characters named by their code (the server refuses them too). */
const shownText = (text) => String(text).replace(/\r\n?/g, '\n').replace(INVISIBLE, (c) => `⟨U+${c.codePointAt(0).toString(16).toUpperCase().padStart(4, '0')}⟩`);

/** The prompt as a person can check it: invisible characters named, long runs of empty lines counted. */
export function shownPrompt(text) {
  const named = shownText(text);
  return named.replace(/\n(?:[ \t]*\n){2,}/g, (run) => `\n[${run.split('\n').length - 2} empty lines]\n`);
}

/**
 * SPEC-335 XSR-008: what the person's Superagent asks to start in another sandbox, above its
 * session. Nothing reaches that sandbox until the person presses Create task; each request is
 * answered once, and goes when the Superagent's call ends. Create task takes a click only once the
 * whole prompt was in view (scrolled to its end) and the card has been there a moment.
 */
function createRequests(ctx, sandbox) {
  const el = h('div', { class: 'agent-requests', hidden: true });
  const path = `/api/sandboxes/${enc(sandbox.id)}/superagent/requests`;
  let alive = true;
  let reading = null;
  let again = false;
  const cards = new Map(); // request id → { el, destroy }

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

  /** Cards come and go; one already shown stays as it is (its scroll and its armed buttons). */
  function render(list) {
    if (!alive) return;
    const items = Array.isArray(list) ? list : [];
    const ids = new Set(items.map((r) => r.id));
    for (const [id, shown] of cards) if (!ids.has(id)) { shown.destroy(); cards.delete(id); }
    for (const request of items) {
      if (cards.has(request.id)) continue;
      const shown = card(request);
      cards.set(request.id, shown);
      el.append(shown.el);
    }
    el.hidden = cards.size === 0;
  }

  function card(request) {
    const target = request.sandbox || {};
    const slug = target.slug;
    const options = Object.entries(request.options || {})
      .map(([key, value]) => (OPTION_LABELS[key] ? OPTION_LABELS[key](typeof value === 'string' ? shownText(value) : value) : null))
      .filter(Boolean);
    const heading = `Your Superagent asks to start a task in ${slug}`;
    const prompt = h('pre', { class: 'agent-request-prompt', tabindex: '0', 'aria-label': 'Prompt' }, shownPrompt(request.prompt || ''));
    const hint = h('div', { class: 'agent-request-hint', hidden: true }, 'Scroll to the end of the prompt to create the task.');
    const create = h('button', { type: 'button', class: 'btn btn-sm btn-primary', disabled: true, onClick: () => respond(true) }, 'Create task');
    const decline = h('button', { type: 'button', class: 'btn btn-sm', disabled: true, onClick: () => respond(false) }, 'Decline');
    let armed = false;
    let read = false;
    let sending = false;
    const update = () => {
      create.disabled = !armed || !read || sending;
      decline.disabled = !armed || sending;
      hint.hidden = read;
    };
    // The whole prompt was in view: it fits, or it was scrolled to its end. A hidden panel shows none of it.
    const check = () => {
      if (read || !prompt.isConnected || prompt.clientHeight === 0) return;
      if (prompt.scrollTop + prompt.clientHeight >= prompt.scrollHeight - 2) { read = true; update(); }
    };
    prompt.addEventListener('scroll', check);
    const resized = typeof ResizeObserver === 'function' ? new ResizeObserver(check) : null;
    if (resized) resized.observe(prompt);
    const timer = setTimeout(() => { armed = true; check(); update(); }, ARM_MS);

    async function respond(approve) {
      if (sending) return;
      sending = true;
      update();
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
        sending = false;
        update();
        load();
      }
    }

    return {
      el: h('section', { class: 'agent-request banner banner-warn small', role: 'group', 'aria-label': heading },
        h('div', { class: 'agent-request-head' }, icon('spark', { size: 16 }),
          h('strong', null, 'Your Superagent asks to start a task in ', h('span', { class: 'agent-request-target' }, slug)),
          target.repository ? h('span', { class: 'muted' }, ` (${target.repository})`) : null),
        request.title ? h('div', { class: 'agent-request-title' }, shownText(request.title)) : null,
        prompt,
        options.length ? h('ul', { class: 'agent-request-options', 'aria-label': 'Start options' }, options.map((o) => h('li', null, o))) : null,
        hint,
        h('div', { class: 'agent-request-actions' }, create, decline)),
      destroy() {
        clearTimeout(timer);
        if (resized) resized.disconnect();
        this.el.remove();
      },
    };
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
      for (const shown of cards.values()) shown.destroy();
      cards.clear();
    },
  };
}
