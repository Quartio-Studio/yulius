// The Cockpit (SPEC-021 CKP-008): the user's own plain Claude Code session in this sandbox, for
// quick questions and work that should not become a task. One per user and sandbox, opened (or
// started, resuming its last conversation) by POST /api/sandboxes/{sid}/cockpit; its terminal is
// the shell terminal protocol. There is no way to delete it: /clear starts over inside it.

import { enc } from '../api.js';
import { h, icon, setChildren, emptyState, loadError } from '../dom.js';
import { createTerminalView } from '../terminal.js';
import { allows } from '../status.js';
import { attachTerminalDrop, shellAttachments } from '../filedrop.js';
import { isPhone } from '../theme.js';

/** Mount the Cockpit page. */
export function mountCockpitPage(ctx, sandbox) {
  const heading = h('h1', { class: 'cockpit-title' }, h('span', { class: 'nav-slot', 'aria-hidden': 'true' }, icon('gauge', { size: 18 })), 'Cockpit');
  const sub = h('span', { class: 'page-sub cockpit-sub' }, 'your own Claude Code session for quick questions, not a task · /clear starts over');
  const body = h('div', { class: 'cockpit-term' });
  const el = h('div', { class: 'cockpit' }, h('div', { class: 'cockpit-head' }, heading, sub), body);
  let terminal = null;
  let alive = true;
  let opening = false;
  let current = null;
  let recovering = false;

  function drop() {
    if (terminal) terminal.destroy();
    terminal = null;
  }

  async function open() {
    if (opening || !alive) return;
    opening = true;
    drop();
    setChildren(body, h('div', { class: 'placeholder placeholder-loading', role: 'status' }, 'Starting your Cockpit…'));
    let shell;
    try {
      shell = await ctx.api.post(`/api/sandboxes/${enc(sandbox.id)}/cockpit`);
    } catch (err) {
      opening = false;
      if (alive) setChildren(body, h('div', { class: 'cockpit-msg' }, loadError(`Couldn’t start the Cockpit: ${err.message}`, open)));
      return;
    }
    opening = false;
    if (alive) mount(shell);
  }

  /**
   * The terminal lost its Cockpit (refused as not found, or the connection dropped): ask for the
   * Cockpit again, which starts it when its process is gone (a reboot), and follow it to its new
   * id. The same id means it still runs; the terminal reconnects by itself.
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

  function mount(shell) {
    drop();
    current = shell;
    terminal = createTerminalView({
      transport: ctx.transport,
      path: `/api/shells/${enc(shell.id)}/terminal`,
      onLink: (uri, event) => ctx.openLink(uri, event),
      readOnlyLabel: () => 'Read-only',
      exitMessage: () => ({ tone: 'info', text: 'The Cockpit stopped. Its conversation is kept.' }),
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
    // On phones a focused terminal raises the soft keyboard over the page (as in the task view).
    if (!isPhone()) terminal.focus();
  }

  if (allows(sandbox.access, 'write')) open();
  else {
    setChildren(body, h('div', { class: 'cockpit-msg' }, emptyState({
      iconName: 'gauge',
      title: 'The Cockpit needs write access to this sandbox.',
      text: 'It runs Claude Code in the sandbox as you; GitHub gives you read access to this repository.',
    })));
  }

  return {
    el,
    key: `cockpit:${sandbox.id}`,
    destroy() {
      alive = false;
      drop();
    },
  };
}
