// The Agent (SPEC-035, formerly the Cockpit of SPEC-021 CKP-008): the user's own plain Claude Code
// session in this sandbox, in the right tool panel beside the page, for quick work that should not
// become a task. One per user and sandbox, opened (or started, resuming its last conversation) by
// POST /api/sandboxes/{sid}/cockpit; its terminal is the shell terminal protocol. There is no way
// to delete it: /clear starts over inside it.

import { enc } from './api.js';
import { h, icon, setChildren, emptyState, loadError } from './dom.js';
import { createTerminalView } from './terminal.js';
import { allows } from './status.js';
import { href } from './router.js';
import { attachTerminalDrop, shellAttachments } from './filedrop.js';
import { isPhone } from './theme.js';

/** The Agent's panel view: its quick-work notice above the session. Starts on the first show. */
export function createAgentView(ctx, sandbox) {
  const newTask = h('a', { href: href(`/s/${encodeURIComponent(sandbox.slug)}`), dataset: { route: `/s/${encodeURIComponent(sandbox.slug)}` } }, 'New task');
  // AGT-002: what the Agent is for, and where bigger work goes.
  const notice = h('div', { class: 'banner banner-info agent-note small', role: 'note', 'aria-label': 'About the Agent' },
    icon('info', { size: 16 }),
    h('span', { class: 'banner-text' },
      h('strong', null, 'For quick work'), ': setting up automations, asking a quick question. Anything more complex, such as writing code, belongs in a task, not here: ', newTask, '.'));
  const body = h('div', { class: 'agent-term' });
  const el = h('div', { class: 'agent' }, notice, body);
  let terminal = null;
  let alive = true;
  let opening = false;
  let started = false;
  let shown = false;
  let current = null;
  let recovering = false;
  const write = allows(sandbox.access, 'write');

  function drop() {
    if (terminal) terminal.destroy();
    terminal = null;
  }

  async function open() {
    if (opening || !alive) return;
    opening = true;
    drop();
    setChildren(body, h('div', { class: 'placeholder placeholder-loading', role: 'status' }, 'Starting your Agent…'));
    let shell;
    try {
      shell = await ctx.api.post(`/api/sandboxes/${enc(sandbox.id)}/cockpit`);
    } catch (err) {
      opening = false;
      if (alive) setChildren(body, h('div', { class: 'agent-msg' }, loadError(`Couldn’t start the Agent: ${err.message}`, open)));
      return;
    }
    opening = false;
    if (alive) mount(shell);
  }

  /**
   * The terminal lost its session (refused as not found, or the connection dropped): ask for the
   * Agent again, which starts it when its process is gone (a reboot), and follow it to its new id.
   * The same id means it still runs; the terminal reconnects by itself.
   */
  async function recover() {
    if (recovering || opening || !alive || !current) return;
    recovering = true;
    try {
      const shell = await ctx.api.post(`/api/sandboxes/${enc(sandbox.id)}/cockpit`);
      if (alive && current && shell.id !== current.id) mount(shell);
    } catch {
      // The terminal shows its own connection state and Retry.
    } finally {
      recovering = false;
    }
  }

  function focus() {
    // On phones a focused terminal raises the soft keyboard over the page (as in the task view).
    if (terminal && shown && !isPhone()) terminal.focus();
  }

  function mount(shell) {
    drop();
    current = shell;
    terminal = createTerminalView({
      transport: ctx.transport,
      path: `/api/shells/${enc(shell.id)}/terminal`,
      onLink: (uri, event) => ctx.openLink(uri, event),
      readOnlyLabel: () => 'Read-only',
      exitMessage: () => ({ tone: 'info', text: 'The Agent stopped. Its conversation is kept.' }),
      exitActions: () => [{ label: 'Start again', run: open }],
      onFatal: (message) => { if (message.code === 'not_found') recover(); },
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
    focus();
  }

  if (!write) {
    setChildren(body, h('div', { class: 'agent-msg' }, emptyState({
      iconName: 'spark',
      title: 'The Agent needs write access to this sandbox.',
      text: 'It runs Claude Code in the sandbox as you; GitHub gives you read access to this repository.',
    })));
  }

  return {
    el,
    show() {
      shown = true;
      // The session starts the first time the panel shows it, not when a sandbox opens.
      if (write && !started) { started = true; open(); return; }
      if (terminal) setTimeout(() => { if (terminal) { terminal.refit(); focus(); } }, 0);
    },
    hide() { shown = false; },
    destroy() {
      alive = false;
      drop();
    },
  };
}
