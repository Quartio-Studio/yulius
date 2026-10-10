// Secrets page (SPEC-001 VLT-001..009, SPEC-002 ADR-18): two sections.
// Team secrets: every member's shared credentials; a password, notes, a TOTP seed or code shows
// only after a fresh confirmation (password or GitHub sign-in in the last 10 minutes) and every
// reveal is audited. Agent secrets: values are write-only; agents use them through Yulius' proxy
// and browser filling and never see them.

import { enc } from '../api.js';
import { h, icon, badge, iconBadge, itemCard, toast, confirmDialog, errorToast, setChildren, iconButton, emptyState, openModal, field, invalid } from '../dom.js';
import { replaceQuietly } from '../router.js';
import { allows } from '../status.js';
import { relLong } from '../format.js';
import { pageFrame, loader } from './common.js';
import { BRAND } from '../brand.js';

/** The clipboard is cleared this long after a copy (VLT-002). */
const CLEAR_CLIPBOARD_MS = 30000;
const NAME_RE = /^[A-Z][A-Z0-9_]{0,63}$/;

/** A small text button; `label` is its accessible name when the text alone is ambiguous. */
function smallButton(text, label, onClick, attrs = {}) {
  return h('button', { type: 'button', class: ['btn btn-sm', attrs.class], 'aria-label': label || null, title: attrs.title || label || null, disabled: attrs.disabled, onClick }, text);
}

let clearTimer = null;
/** Copies a value and clears the clipboard after 30 s (when the page still has focus). */
async function copyValue(value, what) {
  await navigator.clipboard.writeText(value);
  clearTimeout(clearTimer);
  clearTimer = setTimeout(() => {
    if (document.hasFocus()) navigator.clipboard.writeText('').catch(() => {});
  }, CLEAR_CLIPBOARD_MS);
  toast(`${what} copied. The clipboard clears in 30 seconds.`, { kind: 'ok' });
}

/**
 * Runs `request`; when the server asks for a fresh confirmation, asks the person (password, or
 * GitHub sign-in again) and retries once. Resolves `null` when they cancel.
 */
async function withReauth(ctx, request) {
  try {
    return await request();
  } catch (error) {
    if (!error || error.code !== 'reauth_required') throw error;
    const confirmed = await confirmIdentity(ctx, (error.body && error.body.reauth) || { methods: [] });
    if (!confirmed) return null;
    return request();
  }
}

/** The confirmation dialog; resolves true once confirmed. */
function confirmIdentity(ctx, status) {
  const methods = status.methods || [];
  const password = h('input', { class: 'input', type: 'password', id: 'reauth-password', autocomplete: 'current-password', 'aria-describedby': 'reauth-hint' });
  const body = h('div', { class: 'stack' },
    h('p', { id: 'reauth-hint' }, 'Secrets show only within 10 minutes of confirming it’s you. Every reveal is recorded.'),
    methods.includes('password') ? field('Your password', password) : null,
    methods.includes('github') ? h('p', { class: 'muted small' }, 'Signing in with GitHub again confirms it’s you; you come back to this page.') : null,
    methods.length === 0 ? h('p', { class: 'muted' }, 'This account cannot confirm here. Sign out and sign in again, then reveal within 10 minutes.') : null,
  );
  const actions = [{ label: 'Cancel', value: false }];
  if (methods.includes('github')) {
    actions.push({ label: 'Continue with GitHub', kind: methods.includes('password') ? null : 'primary', run: () => {
      const next = window.location.pathname + window.location.search + window.location.hash;
      window.location.assign(`/api/auth/github/start?next=${encodeURIComponent(next)}`);
      return false;
    } });
  }
  if (methods.includes('password')) {
    actions.push({ label: 'Confirm', kind: 'primary', value: true, submit: true, run: async () => {
      if (!password.value) return invalid(password, 'Enter your password.');
      await ctx.api.post('/api/auth/reauth', { password: password.value });
      return true;
    } });
  }
  const modal = openModal({ title: 'Confirm it’s you', body, actions });
  return modal.result.then((v) => v === true);
}

// ---------------------------------------------------------------- team secrets

function teamForm(secret) {
  const title = h('input', { class: 'input', value: secret ? secret.title : '', maxlength: 200, autofocus: true, placeholder: 'Shared Gmail' });
  const url = h('input', { class: 'input', value: secret && secret.url ? secret.url : '', placeholder: 'https://mail.google.com', autocomplete: 'off' });
  const username = h('input', { class: 'input', value: secret && secret.username ? secret.username : '', placeholder: 'team@company.com', autocomplete: 'off' });
  const password = h('input', { class: 'input mono', type: 'password', autocomplete: 'new-password', placeholder: secret && secret.has_password ? 'Unchanged' : '' });
  const notes = h('textarea', { class: 'textarea', rows: 3, placeholder: secret && secret.has_notes ? 'Unchanged' : 'Recovery codes, security answers…' });
  const totp = h('input', { class: 'input mono', autocomplete: 'off', placeholder: secret && secret.has_totp ? 'Unchanged' : 'Base32 seed or otpauth:// link' });
  const body = h('div', { class: 'stack' },
    field('Title', title),
    field('Address', url),
    field('User name', username),
    field('Password', password, secret ? 'Leave empty to keep the stored password.' : null),
    field('Notes', notes, 'Encrypted like the password.'),
    field('Two-factor seed (TOTP)', totp, `From the QR code’s “enter this key” text. ${BRAND.name} shows the current code.`),
  );
  const values = () => {
    const out = { title: title.value.trim(), url: url.value.trim(), username: username.value.trim() };
    if (!secret || password.value) out.password = password.value;
    if (!secret || notes.value) out.notes = notes.value;
    if (!secret || totp.value.trim()) out.totp = totp.value.trim();
    return out;
  };
  return { body, values, title };
}

function showTeamSecret(ctx, secret, revealed) {
  let countdown = null;
  const rows = [];
  if (revealed.password) {
    rows.push(field('Password', h('div', { class: 'row-wrap' },
      h('code', { class: 'mono secret-value' }, revealed.password),
      smallButton('Copy', 'Copy password', () => copyValue(revealed.password, 'Password').catch((e) => errorToast(e, 'Could not copy'))))));
  }
  if (revealed.notes) rows.push(field('Notes', h('pre', { class: 'mono secret-notes' }, revealed.notes)));
  const code = h('code', { class: 'mono secret-value' }, '······');
  const left = h('span', { class: 'muted small' });
  if (secret.has_totp) {
    const refresh = async () => {
      try {
        const current = await ctx.api.post(`/api/team-secrets/${enc(secret.id)}/totp`);
        code.textContent = current.code;
        let remaining = current.remaining;
        left.textContent = `${remaining} s`;
        clearInterval(countdown);
        countdown = setInterval(() => {
          remaining -= 1;
          if (remaining <= 0) { clearInterval(countdown); refresh(); } else left.textContent = `${remaining} s`;
        }, 1000);
      } catch (error) {
        clearInterval(countdown);
        left.textContent = error.code === 'reauth_required' ? 'Confirm again to see codes.' : error.message;
      }
    };
    refresh();
    rows.push(field('Current code', h('div', { class: 'row-wrap' }, code, left,
      smallButton('Copy', 'Copy code', () => copyValue(code.textContent, 'Code').catch((e) => errorToast(e, 'Could not copy'))))));
  }
  if (rows.length === 0) rows.push(h('p', { class: 'muted' }, 'This secret stores no password, notes or seed.'));
  openModal({ title: secret.title, body: h('div', { class: 'stack' }, ...rows), actions: [{ label: 'Done', kind: 'primary' }], onClose: () => clearInterval(countdown) });
}

function mountTeam(ctx, host) {
  const list = h('div');
  const activity = h('div');
  const add = h('button', { type: 'button', class: 'btn btn-primary', onClick: () => edit(null) }, 'Add team secret');
  setChildren(host,
    h('div', { class: 'toolbar' }, h('span', { class: 'muted small' }, 'Shared by everyone on this instance. Passwords, notes and codes show after you confirm it’s you; every reveal is recorded.'), add),
    list,
    h('h2', { class: 'section-title' }, 'Activity'),
    activity);

  async function reveal(secret, purpose, fields) {
    const run = () => ctx.api.post(`/api/team-secrets/${enc(secret.id)}/reveal`, { purpose, fields });
    return withReauth(ctx, run);
  }

  function render(secrets) {
    if (secrets.length === 0) return emptyState({ iconName: 'lock', title: 'No team secrets yet', text: 'Keep shared logins here instead of chats and mail: the team’s mailboxes, infrastructure consoles, their two-factor codes.', action: h('button', { type: 'button', class: 'btn btn-primary', onClick: () => edit(null) }, 'Add team secret') });
    return h('div', { class: 'item-list' }, secrets.map((secret) => itemCard({
      iconName: 'key',
      title: secret.title,
      badges: [
        secret.has_totp ? iconBadge('clock', '2FA codes', '', `A TOTP seed is stored: ${BRAND.name} shows the current code`) : null,
        secret.rotate_due ? badge('rotate', 'warn', 'Changed more than 180 days ago') : null,
      ].filter(Boolean),
      description: h('span', null, [secret.username, secret.url].filter(Boolean).join(' · ') || 'No user name or address'),
      extra: h('span', { class: 'muted small' }, `Changed ${relLong(secret.value_updated_at)} · by @${secret.updated_by}`),
      controls: [
        secret.has_password ? smallButton('Copy password', `Copy password of ${secret.title}`, async () => {
          try {
            const values = await reveal(secret, 'copy', ['password']);
            if (values && values.password) await copyValue(values.password, 'Password');
          } catch (error) { errorToast(error, 'Could not copy'); }
          reloadActivity();
        }) : null,
        smallButton('Show', `Show ${secret.title}`, async () => {
          try {
            const values = await reveal(secret, 'show');
            if (values) showTeamSecret(ctx, secret, values);
          } catch (error) { errorToast(error, 'Could not reveal'); }
          reloadActivity();
        }),
        smallButton('Edit', `Edit ${secret.title}`, () => edit(secret)),
        iconButton('trash', `Delete ${secret.title}`, () => remove(secret), { class: 'btn-quiet-danger' }),
      ].filter(Boolean),
    })));
  }

  const reload = loader(list, () => ctx.api.get('/api/team-secrets'), render);
  const reloadActivity = loader(activity, () => ctx.api.get('/api/secrets/activity?limit=50'), (entries) => {
    if (!entries.length) return h('p', { class: 'muted small' }, 'Nothing yet.');
    return h('ul', { class: 'plain-list activity-list' }, entries.map((e) => h('li', { class: 'small' },
      h('span', { class: 'mono' }, e.actor), ' ', h('span', null, ACTIONS[e.action] || e.action), ' ', h('strong', null, e.detail || ''), ' ', h('span', { class: 'muted' }, `· ${relLong(e.at)}`))));
  });

  function edit(secret) {
    const form = teamForm(secret);
    openModal({ title: secret ? `Edit ${secret.title}` : 'Add team secret', body: form.body, actions: [
      { label: 'Cancel', value: false },
      { label: secret ? 'Save' : 'Add', kind: 'primary', submit: true, run: async () => {
        const values = form.values();
        if (!values.title) return invalid(form.title, 'A title is required.');
        if (secret) await ctx.api.patch(`/api/team-secrets/${enc(secret.id)}`, values);
        else await ctx.api.post('/api/team-secrets', values);
        toast(secret ? 'Saved.' : 'Team secret added.', { kind: 'ok' });
        reload();
        reloadActivity();
        return true;
      } },
    ] });
  }

  async function remove(secret) {
    if (!(await confirmDialog({ title: `Delete “${secret.title}”?`, message: 'Everyone loses access to it here. Rotate it at its service if it might have been shared elsewhere.', confirmLabel: 'Delete', danger: true }))) return;
    try {
      await ctx.api.del(`/api/team-secrets/${enc(secret.id)}`);
      toast('Deleted.', { kind: 'ok' });
      reload();
      reloadActivity();
    } catch (error) { errorToast(error, 'Could not delete'); }
  }
}

const ACTIONS = {
  'vault.team.create': 'added',
  'vault.team.update': 'changed',
  'vault.team.delete': 'deleted',
  'vault.team.reveal': 'revealed',
  'vault.team.copy': 'copied',
  'vault.team.totp': 'read a code of',
  'vault.reauth': 'confirmed identity',
  'vault.reauth.failed': 'failed to confirm identity',
  'vault.agent.create': 'added agent secret',
  'vault.agent.update': 'changed agent secret',
  'vault.agent.delete': 'deleted agent secret',
  'vault.agent.use': 'used',
  'vault.agent.fill': 'typed',
  'vault.agent.redacted': 'had redacted',
};

// ---------------------------------------------------------------- agent secrets

function agentForm(ctx, secret) {
  const instanceAdmin = ctx.isInstanceAdmin();
  const scope = h('select', { class: 'select', disabled: Boolean(secret) || !instanceAdmin },
    h('option', { value: 'sandbox' }, 'This sandbox'),
    h('option', { value: 'instance' }, 'Every sandbox'));
  if (secret) scope.value = secret.scope;
  const name = h('input', { class: 'input mono', value: secret ? secret.name : '', placeholder: 'STRIPE_API_KEY', autocomplete: 'off', autofocus: true });
  const description = h('input', { class: 'input', value: secret ? secret.description : '', placeholder: 'Stripe test-mode key for the checkout' });
  const hosts = h('input', { class: 'input mono', value: secret ? secret.hosts.join(', ') : '', placeholder: 'api.stripe.com', autocomplete: 'off' });
  const injection = h('select', { class: 'select' },
    h('option', { value: 'bearer' }, 'Authorization: Bearer <value>'),
    h('option', { value: 'basic' }, 'Authorization: Basic <user name:value>'),
    h('option', { value: 'header' }, 'A header of its own'),
    h('option', { value: 'none' }, 'No header (website login, placeholders)'));
  injection.value = secret ? secret.injection : 'bearer';
  const header = h('input', { class: 'input mono', value: secret && secret.header_name ? secret.header_name : '', placeholder: 'X-Api-Key' });
  const headerField = field('Header name', header);
  const sync = () => { headerField.hidden = injection.value !== 'header'; };
  injection.addEventListener('change', sync);
  sync();
  const username = h('input', { class: 'input', autocomplete: 'off', placeholder: secret && secret.has_username ? 'Unchanged' : 'For Basic credentials and website logins' });
  const value = h('input', { class: 'input mono', type: 'password', autocomplete: 'new-password', placeholder: secret ? 'Unchanged' : '' });
  const totp = h('input', { class: 'input mono', autocomplete: 'off', placeholder: secret && secret.has_totp ? 'Unchanged' : 'Optional: for website two-factor codes' });
  const body = h('div', { class: 'stack' },
    field('Scope', scope, instanceAdmin ? null : 'Secrets for every sandbox are added by instance administrators.'),
    field('Name', name, 'Agents refer to it by this name; their code reads $NAME, which holds a placeholder.'),
    field('What it is for', description, 'Agents read this to know when to use it.'),
    field('Hosts', hosts, 'The only destinations it is sent to or typed into, comma-separated (*.example.com for subdomains).'),
    field('Added to requests as', injection),
    headerField,
    field('User name', username),
    field('Value', value, secret ? 'Leave empty to keep it. Nobody can read it back.' : 'At least 8 characters. Nobody can read it back, agents included.'),
    field('Two-factor seed (TOTP)', totp),
  );
  const values = () => {
    const out = {
      name: name.value.trim(),
      description: description.value.trim(),
      hosts: hosts.value.split(/[\s,]+/).filter(Boolean),
      injection: injection.value,
      header_name: injection.value === 'header' ? header.value.trim() : null,
    };
    if (!secret) out.scope = scope.value;
    if (username.value.trim() || !secret) out.username = username.value.trim() || null;
    if (value.value) out.value = value.value;
    if (totp.value.trim()) out.totp = totp.value.trim();
    return out;
  };
  return { body, values, name, hosts, value, header };
}

function mountAgents(ctx, sandbox, host) {
  if (!sandbox) {
    setChildren(host, emptyState({ iconName: 'lock', title: 'Open a sandbox', text: 'Agent secrets belong to a sandbox (or to every sandbox). Open one to see and add them.' }));
    return;
  }
  const write = allows(sandbox.access, 'write');
  const list = h('div');
  const add = h('button', { type: 'button', class: 'btn btn-primary', disabled: !write, title: write ? null : 'Requires write access', onClick: () => edit(null) }, 'Add agent secret');
  setChildren(host,
    h('div', { class: 'banner banner-info' }, icon('shield'), h('div', { class: 'banner-text small' },
      `Agents use these without seeing them: ${BRAND.name} adds a secret to HTTPS requests for its hosts and types it into sign-in pages of those hosts. Anything that would show a value to an agent — command output, files, web pages, tool results — shows `,
      h('code', null, '[secret:NAME]'), ' instead.')),
    h('div', { class: 'toolbar' }, h('span', { class: 'muted small' }, `For ${sandbox.name} and every sandbox.`), add),
    list);

  function render(secrets) {
    if (secrets.length === 0) return emptyState({ iconName: 'key', title: 'No agent secrets yet', text: 'Add an API key or a login an agent needs; bind it to the hosts it belongs to.', action: write ? h('button', { type: 'button', class: 'btn btn-primary', onClick: () => edit(null) }, 'Add agent secret') : null });
    return h('div', { class: 'item-list' }, secrets.map((secret) => itemCard({
      iconName: 'key',
      title: h('span', { class: 'mono' }, secret.name),
      badges: [
        secret.scope === 'instance' ? iconBadge('globe', 'Every sandbox', 'violet') : iconBadge('folder', 'This sandbox', ''),
        secret.rotate_due ? badge('rotate', 'warn', 'Changed more than 180 days ago') : null,
      ].filter(Boolean),
      description: secret.description || h('span', { class: 'muted' }, 'No description'),
      extra: h('div', { class: 'stack-tight' },
        h('div', { class: 'chips' }, secret.hosts.map((name) => iconBadge('globe', name, '', 'Bound host'))),
        h('span', { class: 'muted small' }, `${secret.injection_text} · ${secret.last_used_at ? `used ${relLong(secret.last_used_at)}` : 'not used yet'} · changed ${relLong(secret.value_updated_at)}`)),
      controls: [
        smallButton('Edit', `Edit ${secret.name}`, () => edit(secret), { disabled: !secret.can_manage, title: secret.can_manage ? `Edit ${secret.name}` : 'You cannot change this secret' }),
        iconButton('trash', `Delete ${secret.name}`, () => remove(secret), { class: 'btn-quiet-danger', disabled: !secret.can_manage }),
      ],
    })));
  }

  const reload = loader(list, () => ctx.api.get(`/api/sandboxes/${enc(sandbox.id)}/agent-secrets`), render);

  function edit(secret) {
    const form = agentForm(ctx, secret);
    openModal({ title: secret ? `Edit ${secret.name}` : 'Add agent secret', wide: true, body: form.body, actions: [
      { label: 'Cancel', value: false },
      { label: secret ? 'Save' : 'Add', kind: 'primary', submit: true, run: async () => {
        const values = form.values();
        if (!NAME_RE.test(values.name)) return invalid(form.name, 'Capital letters, digits and _, starting with a letter.');
        if (values.hosts.length === 0) return invalid(form.hosts, 'Bind it to at least one host.');
        if (!secret && !values.value) return invalid(form.value, 'Enter the value.');
        if (values.value && [...values.value].length < 8) return invalid(form.value, 'At least 8 characters: shorter values cannot be recognised and hidden.');
        if (values.injection === 'header' && !values.header_name) return invalid(form.header, 'Name the header.');
        if (secret) await ctx.api.patch(`/api/agent-secrets/${enc(secret.id)}`, values);
        else await ctx.api.post(`/api/sandboxes/${enc(sandbox.id)}/agent-secrets`, values);
        toast(secret ? 'Saved.' : 'Agent secret added. New agent sessions get its placeholder.', { kind: 'ok' });
        reload();
        return true;
      } },
    ] });
  }

  async function remove(secret) {
    if (!(await confirmDialog({ title: `Delete ${secret.name}?`, message: 'Agents can no longer use it; requests to its hosts go out without it.', confirmLabel: 'Delete', danger: true }))) return;
    try {
      await ctx.api.del(`/api/agent-secrets/${enc(secret.id)}`);
      toast('Deleted.', { kind: 'ok' });
      reload();
    } catch (error) { errorToast(error, 'Could not delete'); }
  }
}

/** Mount the Secrets page: `/s/<slug>/secrets` (with the sandbox's agent secrets) or `/secrets`. */
export function mountSecretsPage(ctx, sandbox, route) {
  const frame = pageFrame({ title: 'Secrets', sub: 'team logins for people, keys agents use without seeing them' });
  const base = sandbox ? `/s/${encodeURIComponent(sandbox.slug)}/secrets` : '/secrets';
  const host = h('div');
  let current = route.query.get('section') === 'agents' ? 'agents' : '';
  const tabs = h('div', { class: 'tl-tabs ftabs', role: 'radiogroup', 'aria-label': 'Section' });
  const options = [{ value: '', label: 'Team secrets', icon: 'users' }, { value: 'agents', label: 'Agent secrets', icon: 'key' }];
  function renderTabs() {
    const focused = tabs.contains(document.activeElement);
    setChildren(tabs, options.map((o) => h('button', {
      type: 'button', role: 'radio', 'aria-checked': String(o.value === current), tabindex: o.value === current ? '0' : '-1', class: ['tl-tab', o.value === current && 'active'],
      onClick: () => {
        if (current === o.value) return;
        current = o.value;
        replaceQuietly(current ? `${base}?section=${current}` : base);
        renderTabs();
        show();
      },
      onKeydown: (event) => {
        if (!['ArrowRight', 'ArrowLeft', 'ArrowUp', 'ArrowDown'].includes(event.key)) return;
        event.preventDefault();
        const other = [...tabs.children].find((b) => b !== event.currentTarget);
        if (other) other.click();
      },
    }, h('span', { class: 'tl-tab-icon' }, icon(o.icon, { size: 14 })), o.label)));
    if (focused) tabs.querySelector('[aria-checked="true"]')?.focus();
  }
  renderTabs();
  frame.body.append(tabs, host);
  function show() {
    if (current === 'agents') mountAgents(ctx, sandbox, host);
    else mountTeam(ctx, host);
  }
  show();
  return { el: frame.el, key: `secrets:${sandbox ? sandbox.id : ''}` };
}

