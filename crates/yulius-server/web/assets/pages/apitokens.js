// Connect your local agent (SPEC-247 RMT-001, RMT-040): the signed-in person's API tokens for
// Claude Code (or any agent) on their computer and the `yulius` CLI. A token is shown once, with
// the exact `claude mcp add` and `yulius remote login` commands; the list shows each token's
// scope and last use, and Revoke ends it at once.

import { enc } from '../api.js';
import { h, icon, badge, toast, confirmDialog, errorToast, setChildren, field } from '../dom.js';
import { relLong } from '../format.js';
import { copyButton } from './secrets.js';
import { BRAND } from '../brand.js';

const SCOPES = [
  { id: 'operate', name: 'Operate', hint: 'Everything you may do: import, onboard, tasks, CI, files, automations, secrets' },
  { id: 'read', name: 'Read only', hint: 'Lists, tasks, transcripts, CI and files; changes nothing' },
];

/** The instance's address as the person reaches it. */
function instanceUrl(ctx) {
  const me = ctx.store.state.me || {};
  const url = (me.instance && me.instance.public_url) || window.location.origin;
  return url.replace(/\/+$/, '');
}

/** The `claude mcp add` command for `token` (a placeholder before one is made). */
export function claudeCommand(url, token) {
  return `claude mcp add --transport http yulius ${url}/mcp --header "Authorization: Bearer ${token}"`;
}

/** One command with its copy button. */
function commandBox(label, text) {
  return h('div', { class: 'secret-box agent-command' },
    h('code', { class: 'mono secret-value', 'aria-label': label }, text),
    copyButton(`Copy ${label}`, text, label));
}

/** The "Connect your local agent" section's body. */
export function agentTokensBody(ctx) {
  const url = instanceUrl(ctx);
  const list = h('div', { class: 'agent-tokens', 'aria-live': 'polite' });
  const created = h('div', { class: 'agent-created' });
  const label = h('input', { type: 'text', class: 'input', maxlength: 80, placeholder: 'e.g. laptop Claude Code', autocomplete: 'off' });
  const scope = h('select', { class: 'select' }, SCOPES.map((s) => h('option', { value: s.id }, s.name)));
  const scopeHint = h('span', { class: 'field-hint' }, SCOPES[0].hint);
  scope.addEventListener('change', () => { scopeHint.textContent = (SCOPES.find((s) => s.id === scope.value) || SCOPES[0]).hint; });
  const createButton = h('button', { type: 'submit', class: 'btn btn-primary' }, icon('plus', { size: 14 }), 'Create token');

  const render = (tokens) => {
    if (!tokens.length) {
      setChildren(list, h('p', { class: 'muted small' }, 'No tokens yet.'));
      return;
    }
    setChildren(list, h('ul', { class: 'agent-token-list' }, tokens.map((t) => h('li', { class: 'agent-token' },
      h('div', { class: 'agent-token-text' },
        h('div', null, h('span', { class: 'agent-token-label' }, t.label), ' ', badge(t.scope === 'operate' ? 'Operate' : t.scope === 'read' ? 'Read only' : 'yulius login', t.scope === 'operate' ? 'ok' : '')),
        h('div', { class: 'muted xs' }, `Created ${relLong(t.created_at)} · ${t.last_used_at ? `last used ${relLong(t.last_used_at)}` : 'never used'}`)),
      h('button', { type: 'button', class: 'btn btn-sm', 'aria-label': `Revoke ${t.label}`, onClick: () => revoke(t) }, 'Revoke')))));
  };

  const load = async () => {
    try {
      render(await ctx.api.get('/api/me/tokens'));
    } catch (error) {
      setChildren(list, h('p', { class: 'muted small' }, `Couldn’t load your tokens: ${error.message}`));
    }
  };

  const revoke = async (token) => {
    const ok = await confirmDialog({
      title: `Revoke “${token.label}”?`,
      message: 'Every agent and script using it is refused from now on. This cannot be undone.',
      confirmLabel: 'Revoke',
      danger: true,
    });
    if (!ok) return;
    try {
      await ctx.api.del(`/api/me/tokens/${enc(token.id)}`);
      toast(`Revoked “${token.label}”.`, { kind: 'ok' });
      setChildren(created);
      await load();
    } catch (error) {
      errorToast(error, 'Couldn’t revoke the token');
    }
  };

  const form = h('form', { class: 'agent-token-form', onSubmit: async (event) => {
    event.preventDefault();
    if (!label.value.trim()) {
      label.focus();
      toast('Name the token first, e.g. where it is used.', { kind: 'warn' });
      return;
    }
    createButton.disabled = true;
    try {
      const made = await ctx.api.post('/api/me/tokens', { label: label.value.trim(), scope: scope.value });
      label.value = '';
      setChildren(created, h('div', { class: 'agent-created-box', role: 'status' },
        h('p', { class: 'small' }, h('strong', null, 'Copy it now: '), 'the token is shown only this once.'),
        commandBox('token', made.token),
        h('p', { class: 'small' }, `Add ${BRAND.name} to Claude Code on your computer:`),
        commandBox('Claude Code command', made.claude_command),
        h('p', { class: 'small' }, 'Or sign the yulius CLI in, then paste the token when it asks:'),
        commandBox('CLI command', made.cli_command)));
      await load();
    } catch (error) {
      errorToast(error, 'Couldn’t create the token');
    } finally {
      createButton.disabled = false;
    }
  } },
  field('Token name', label),
  h('label', { class: 'field' }, h('span', { class: 'field-label' }, 'Scope'), scope, scopeHint),
  h('div', { class: 'agent-token-submit' }, createButton));

  load();
  return h('div', { class: 'agent-connect' },
    h('p', { class: 'small' }, 'Drive this instance from Claude Code, or any agent, on your computer: “import my repo”, “make a task”, “restart #12”. The token acts as you, with your access to each project; only its fingerprint is stored.'),
    commandBox('example Claude Code command', claudeCommand(url, '<token>')),
    form,
    created,
    list);
}
