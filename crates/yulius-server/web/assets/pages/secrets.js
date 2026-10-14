// Secrets page (SPEC-001 VLT-001..009, SPEC-002 ADR-20, SPEC-040): two sections.
// Team secrets: a title, a value and notes shared by every member. Agent secrets: agents use them
// through Yulius' proxy and browser filling and never see them. People read a value only after
// entering their own two-factor code (set up once, mandatory; it unlocks the vault for 10
// minutes), and every reveal is audited.

import { enc } from '../api.js';
import { h, icon, badge, iconBadge, itemCard, toast, confirmDialog, errorToast, setChildren, iconButton, emptyState, openModal, field, invalid } from '../dom.js';
import { replaceQuietly } from '../router.js';
import { allows } from '../status.js';
import { relLong } from '../format.js';
import { pageFrame, loader, createButton } from './common.js';
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
 * Runs `request`; when the server asks for a two-factor code, unlocks the vault (setting
 * two-factor up first if needed) and retries once. Resolves `null` when the person cancels.
 */
export async function withReauth(ctx, request) {
  try {
    return await request();
  } catch (error) {
    if (!error || error.code !== 'reauth_required') throw error;
    const unlocked = await unlockVault(ctx, (error.body && error.body.reauth) || { methods: [] });
    if (!unlocked) return null;
    return request();
  }
}

/**
 * Unlocks the vault: the code dialog; first the identity confirmation when codes are locked
 * after too many wrong ones, and the setup when two-factor is not on (a pending setup can be
 * finished with a code from the key added earlier).
 */
async function unlockVault(ctx, status) {
  if (status.two_factor === 'locked') {
    if (!(await confirmIdentity(ctx, status, 'Too many wrong codes. Confirm it’s you, then enter a code again.'))) return false;
    return enterCode(ctx, null);
  }
  if (status.two_factor === 'on') return enterCode(ctx, null);
  if (status.two_factor === 'pending') {
    const result = await enterCode(ctx, null, true);
    if (result !== 'restart') return result;
  }
  if (!status.identity_fresh && !(await confirmIdentity(ctx, status, 'Setting up two-factor needs a recent sign-in. Confirm it’s you first.'))) return false;
  const setup = await ctx.api.post('/api/auth/two-factor');
  return enterCode(ctx, setup);
}

/**
 * The code dialog; with `setup` (a new seed), it also shows the key to add to the app; with
 * `pending`, it offers a new key instead (resolves `'restart'`).
 */
function enterCode(ctx, setup, pending = false) {
  const code = h('input', { class: 'input mono', id: 'unlock-code', inputmode: 'numeric', autocomplete: 'one-time-code', maxlength: 6, 'aria-describedby': 'unlock-hint' });
  let hint = 'Enter the code from your authenticator app. Secrets stay readable for 10 minutes; every reveal is recorded.';
  if (setup) hint = 'Reading secrets needs two-factor. Add this key to your authenticator app (1Password, Google Authenticator…), then enter the code it shows. You do this once.';
  else if (pending) hint = 'Finish setting up two-factor: enter the code your authenticator app shows for the key you added, or get a new key.';
  const body = h('div', { class: 'stack' },
    h('p', { id: 'unlock-hint' }, hint),
    setup ? h('div', { class: 'stack-tight' },
      h('span', { class: 'field-label' }, 'Key'),
      h('div', { class: 'row-wrap' },
        h('code', { class: 'mono secret-value' }, setup.secret),
        smallButton('Copy', 'Copy key', () => copyValue(setup.secret, 'Key').catch((e) => errorToast(e, 'Couldn’t copy'))))) : null,
    setup ? h('a', { href: setup.uri, class: 'small' }, 'Open in an authenticator app') : null,
    field('Code from your app', code),
  );
  const actions = [{ label: 'Cancel', value: false }];
  if (pending) actions.push({ label: 'New key', value: 'restart' });
  actions.push({ label: 'Unlock', kind: 'primary', value: true, submit: true, run: async () => {
    const value = code.value.replace(/\s+/g, '');
    if (!/^\d{6}$/.test(value)) return invalid(code, 'Enter the 6-digit code.');
    await ctx.api.post('/api/auth/unlock', { code: value });
    return true;
  } });
  const modal = openModal({ title: setup || pending ? 'Set up two-factor' : 'Enter your two-factor code', body, actions });
  return modal.result.then((v) => (v === 'restart' ? 'restart' : v === true));
}

/** Confirms identity (to set two-factor up, or after too many wrong codes); resolves true once confirmed. */
function confirmIdentity(ctx, status, why) {
  const methods = status.methods || [];
  const password = h('input', { class: 'input', type: 'password', id: 'reauth-password', autocomplete: 'current-password', 'aria-describedby': 'reauth-hint' });
  const body = h('div', { class: 'stack' },
    h('p', { id: 'reauth-hint' }, why),
    methods.includes('password') ? field('Your password', password) : null,
    methods.includes('github') ? h('p', { class: 'muted small' }, 'Signing in with GitHub again confirms it’s you; you come back to this page.') : null,
    methods.length === 0 ? h('p', { class: 'muted' }, 'This account cannot confirm here. Sign out and sign in again, then set up two-factor within 10 minutes.') : null,
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

/** The dots a masked value shows: a fixed run, so they do not tell the value's length. */
const MASK = '••••••••••••';

/**
 * A revealed value, masked until its eye button is pressed (SPEC-114). While masked the value is
 * not in the page at all, so a selection, find-in-page or an extension cannot read it. Hovering
 * the box (or focus in it) shows a quick copy after the eye, which `copy` serves.
 */
function maskedValue(value, copy) {
  const text = h('code', { class: 'mono secret-value' });
  const eye = h('button', { type: 'button', class: 'btn-icon secret-eye', 'aria-label': 'Show value', 'aria-pressed': 'false', onClick: () => draw(eye.getAttribute('aria-pressed') !== 'true') });
  function draw(shown) {
    eye.setAttribute('aria-pressed', String(shown));
    eye.title = shown ? 'Hide value' : 'Show value';
    setChildren(eye, icon(shown ? 'eye-off' : 'eye'));
    text.classList.toggle('secret-masked', !shown);
    if (shown) setChildren(text, value);
    else setChildren(text, h('span', { 'aria-hidden': 'true' }, MASK), h('span', { class: 'sr-only' }, 'Value hidden'));
  }
  draw(false);
  return h('div', { class: 'secret-box' }, text, eye, iconButton('copy', 'Copy', copy, { class: 'secret-quick-copy' }));
}

/** Shows a revealed value (masked until its eye is pressed) and notes. */
function showRevealed(title, revealed) {
  const rows = [];
  if (revealed.value) {
    const copy = () => copyValue(revealed.value, 'Value').catch((e) => errorToast(e, 'Couldn’t copy'));
    rows.push(field('Value', h('div', { class: 'row-wrap' },
      maskedValue(revealed.value, copy),
      smallButton('Copy', 'Copy value', copy))));
  }
  if (revealed.notes) rows.push(field('Notes', h('pre', { class: 'mono secret-notes' }, revealed.notes)));
  if (rows.length === 0) rows.push(h('p', { class: 'muted' }, 'This secret stores no value or notes.'));
  openModal({ title, body: h('div', { class: 'stack' }, ...rows), actions: [{ label: 'Done', kind: 'primary' }] });
}

// ---------------------------------------------------------------- team secrets

/** `loadNotes` reveals the stored notes (after a two-factor code) so they can be edited. */
function teamForm(secret, loadNotes) {
  const title = h('input', { class: 'input', value: secret ? secret.title : '', maxlength: 200, autofocus: true, placeholder: 'Shared Gmail' });
  const value = h('input', { class: 'input mono', type: 'password', autocomplete: 'new-password', placeholder: secret && secret.has_value ? 'Unchanged' : '' });
  const stored = Boolean(secret && secret.has_notes);
  const notes = h('textarea', { class: 'textarea', rows: 3, readOnly: stored, placeholder: stored ? 'Stored notes stay as they are' : 'User name, address, recovery codes…' });
  const body = h('div', { class: 'stack' },
    field('Title', title),
    field('Value', value, secret ? 'Leave empty to keep the stored value.' : null),
    field('Notes', notes, stored ? 'To change them, load them first: what you save replaces them.' : 'Encrypted like the value.'),
    stored ? h('div', null, smallButton('Edit current notes', `Edit current notes of ${secret.title}`, async (event) => {
      try {
        const values = await loadNotes();
        if (!values) return;
        notes.value = values.notes || '';
        notes.placeholder = '';
        notes.readOnly = false;
        event.target.hidden = true;
        notes.focus();
      } catch (error) { errorToast(error, 'Couldn’t reveal'); }
    })) : null,
  );
  const values = () => {
    const out = { title: title.value.trim() };
    if (!secret || value.value) out.value = value.value;
    if (!secret || notes.value) out.notes = notes.value;
    return out;
  };
  return { body, values, title };
}

function mountTeam(ctx, host, actions) {
  const list = h('div');
  const activity = h('div');
  setChildren(actions, createButton('Add team secret', () => edit(null)));
  setChildren(host,
    h('p', { class: 'muted small' }, 'Shared by everyone on this instance. Values and notes show after your two-factor code; every reveal is recorded.'),
    list,
    h('h2', { class: 'section-title' }, 'Activity'),
    activity);

  async function reveal(secret, purpose, fields) {
    const run = () => ctx.api.post(`/api/team-secrets/${enc(secret.id)}/reveal`, { purpose, fields });
    return withReauth(ctx, run);
  }

  function render(secrets) {
    if (secrets.length === 0) return emptyState({ iconName: 'lock', title: 'No team secrets yet', text: 'Keep shared logins here instead of chats and mail: the team’s mailboxes, infrastructure consoles, recovery codes.', action: createButton('Add team secret', () => edit(null)) });
    return h('div', { class: 'item-list' }, secrets.map((secret) => itemCard({
      iconName: 'key',
      title: secret.title,
      badges: [
        secret.rotate_due ? badge('rotate', 'warn', 'Changed more than 180 days ago') : null,
      ].filter(Boolean),
      description: h('span', null, [secret.has_value && 'Value', secret.has_notes && 'notes'].filter(Boolean).join(' and ') || 'Empty'),
      extra: h('span', { class: 'muted small' }, `Changed ${relLong(secret.value_updated_at)} · by @${secret.updated_by}`),
      controls: [
        secret.has_value ? smallButton('Copy value', `Copy value of ${secret.title}`, async () => {
          try {
            const values = await reveal(secret, 'copy', ['value']);
            if (values && values.value) await copyValue(values.value, 'Value');
          } catch (error) { errorToast(error, 'Couldn’t copy'); }
          reloadActivity();
        }) : null,
        smallButton('Show', `Show ${secret.title}`, async () => {
          try {
            const values = await reveal(secret, 'show');
            if (values) showRevealed(secret.title, values);
          } catch (error) { errorToast(error, 'Couldn’t reveal'); }
          reloadActivity();
        }),
        smallButton('Edit', `Edit ${secret.title}`, () => edit(secret)),
        iconButton('trash', `Delete ${secret.title}`, () => remove(secret), { class: 'btn-quiet-danger' }),
      ].filter(Boolean),
    })));
  }

  const reload = loader(list, 'the team secrets', () => ctx.api.get('/api/team-secrets'), render, { shape: 'items' });
  const reloadActivity = loader(activity, 'the secrets activity', () => ctx.api.get('/api/secrets/activity?limit=50'), (entries) => {
    if (!entries.length) return h('p', { class: 'muted small' }, 'Nothing yet.');
    return h('ul', { class: 'plain-list activity-list' }, entries.map((e) => h('li', { class: 'small' },
      h('span', { class: 'mono' }, e.actor), ' ', h('span', null, ACTIONS[e.action] || e.action), ' ', h('strong', null, e.detail || ''), ' ', h('span', { class: 'muted' }, `· ${relLong(e.at)}`))));
  });

  function edit(secret) {
    const form = teamForm(secret, () => reveal(secret, 'show', ['notes']));
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
    } catch (error) { errorToast(error, 'Couldn’t delete'); }
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
  'vault.2fa.setup': 'started setting up two-factor',
  'vault.2fa.confirm': 'turned on two-factor',
  'vault.2fa.locked': 'was locked out of codes after 10 wrong ones',
  'vault.2fa.reset': 'reset the two-factor of',
  'vault.unlock': 'unlocked the secrets',
  'vault.unlock.failed': 'entered a wrong two-factor code',
  'vault.agent.reveal': 'revealed agent secret',
  'vault.agent.copy': 'copied agent secret',
  'vault.agent.create': 'added agent secret',
  'vault.agent.update': 'changed agent secret',
  'vault.agent.delete': 'deleted agent secret',
  'vault.agent.use': 'used',
  'vault.agent.fill': 'typed',
  'vault.agent.redacted': 'had redacted',
  'vault.agent.request': 'asked for hosts of',
  'vault.agent.allow': 'allowed hosts of',
  'vault.agent.dismiss': 'dismissed hosts of',
};

// ---------------------------------------------------------------- agent secrets

/**
 * The add dialog asks for a name, a value and a description (SPEC-121): agents find the hosts and
 * how to send it, and a person allows the hosts they ask for. An instance administrator picks the
 * scope under More options. Editing keeps the hosts, and the request style of an older secret
 * that has one.
 */
function agentForm(ctx, secret) {
  const name = h('input', { class: 'input mono', 'aria-label': 'Name', value: secret ? secret.name : '', placeholder: 'VIDOC_API_KEY', autocomplete: 'off', autofocus: true });
  const value = h('input', { class: 'input mono', type: 'password', 'aria-label': 'Value', autocomplete: 'new-password', placeholder: secret ? 'Unchanged' : '' });
  const notes = h('textarea', { class: 'textarea', rows: 2, 'aria-label': 'Description', placeholder: 'API key for Vidoc, the video platform' });
  notes.value = secret ? secret.description : '';
  const parts = [
    field('Name', name, 'Agents refer to it by this name; their code reads $NAME, which holds a placeholder.'),
    field('Value', value, secret ? 'Leave empty to keep it.' : 'At least 8 characters. Agents use it without seeing it; people who manage it read it after their two-factor code.'),
    field('Description', notes, 'Which service it is for (a login’s user name goes here, never the value). Agents find the service’s address from it and ask you to allow it.'),
  ];
  const form = { name, value };
  if (!secret && ctx.isInstanceAdmin()) {
    form.scope = h('select', { class: 'select' },
      h('option', { value: 'sandbox' }, 'This sandbox'),
      h('option', { value: 'instance' }, 'Every sandbox'));
    parts.push(h('details', null, h('summary', { class: 'muted small' }, 'More options'),
      h('div', { class: 'stack space-top' }, field('Scope', form.scope))));
  }
  if (secret) {
    form.hosts = h('input', { class: 'input mono', 'aria-label': 'Hosts', value: secret.hosts.join(', '), placeholder: 'api.example.com', autocomplete: 'off' });
    parts.push(field('Hosts', form.hosts, 'The only destinations it is sent to or typed into, comma-separated (*.example.com for subdomains). Agents ask for them; empty sends it nowhere.'));
  }
  if (secret && secret.injection !== 'none') {
    form.injection = h('select', { class: 'select' },
      h('option', { value: 'bearer' }, 'Authorization: Bearer <value>'),
      h('option', { value: 'basic' }, 'Authorization: Basic <user name:value>'),
      h('option', { value: 'header' }, 'A header of its own'),
      h('option', { value: 'none' }, 'Nothing: agents send the placeholder'));
    form.injection.value = secret.injection;
    form.header = h('input', { class: 'input mono', value: secret.header_name || '', placeholder: 'X-Api-Key' });
    form.username = h('input', { class: 'input', autocomplete: 'off', placeholder: secret.has_username ? 'Unchanged' : '' });
    const headerField = field('Header name', form.header);
    const usernameField = field('User name', form.username);
    const sync = () => {
      headerField.hidden = form.injection.value !== 'header';
      usernameField.hidden = form.injection.value !== 'basic';
    };
    form.injection.addEventListener('change', sync);
    sync();
    parts.push(field('Added to requests as', form.injection), headerField, usernameField);
  }
  form.body = h('div', { class: 'stack' }, ...parts);
  form.values = () => {
    const out = { name: name.value.trim(), description: notes.value.trim() };
    if (form.scope) out.scope = form.scope.value;
    if (form.hosts) out.hosts = form.hosts.value.split(/[\s,]+/).filter(Boolean);
    if (form.injection) {
      out.injection = form.injection.value;
      out.header_name = out.injection === 'header' ? form.header.value.trim() : null;
      if (out.injection === 'basic' && form.username.value.trim()) out.username = form.username.value.trim();
    }
    if (value.value) out.value = value.value;
    return out;
  };
  return form;
}

/** A host as people read it: a wildcard says it covers any subdomain. */
function hostText(name) {
  return name.startsWith('*.') ? `${name} (any subdomain of ${name.slice(2)})` : name;
}

/** Where a secret goes: its bound hosts, or that it goes nowhere yet; and what an agent asks for. */
function agentHosts(ctx, secret, reload) {
  const parts = [];
  if (secret.hosts.length) parts.push(h('div', { class: 'chips' }, secret.hosts.map((name) => iconBadge('globe', name, '', 'Bound host'))));
  else if (!secret.requested_hosts.length) parts.push(h('span', { class: 'muted small' }, 'Not sent anywhere yet: agents ask for its hosts and you allow them here.'));
  if (secret.requested_hosts.length) {
    // The answer names the hosts shown here: the server refuses it if an agent asked for more
    // meanwhile, so nothing is allowed unseen (SPEC-121).
    const shown = [...secret.requested_hosts];
    const answer = async (allow) => {
      buttons.forEach((b) => { b.disabled = true; });
      try {
        const saved = await ctx.api.post(`/api/agent-secrets/${enc(secret.id)}/request`, { allow, hosts: shown });
        toast(allow ? `Allowed. Agents can use ${saved.name} on ${shown.join(', ')}.` : 'Dismissed.', { kind: 'ok' });
      } catch (error) { errorToast(error, allow ? 'Couldn’t allow' : 'Couldn’t dismiss'); }
      reload();
    };
    const asked = shown.join(', ');
    const buttons = secret.can_manage ? [
      smallButton('Allow', `Allow ${asked} for ${secret.name}`, () => answer(true), { class: 'btn-primary' }),
      smallButton('Dismiss', `Dismiss the hosts asked for ${secret.name}`, () => answer(false)),
    ] : [];
    parts.push(h('div', { class: 'row-wrap' },
      h('span', { class: 'small' }, secret.requested_by ? `Task ${secret.requested_by} asks to send it to` : 'An agent asks to send it to'),
      h('div', { class: 'chips' }, shown.map((name) => iconBadge('globe', hostText(name), 'warn', 'Requested host'))),
      buttons.length ? h('span', { class: 'row-wrap' }, buttons) : null));
  }
  return parts;
}

/** Whether agents may copy a secret into the project's git-ignored .env files (SPEC-129). */
function agentEnvFiles(ctx, secret, reload) {
  if (secret.env_files !== 'asked' && secret.env_files !== 'allowed') return [];
  const answer = async (allow) => {
    buttons.forEach((b) => { b.disabled = true; });
    try {
      // The answer names what is shown here: the server refuses it if it changed meanwhile.
      await ctx.api.post(`/api/agent-secrets/${enc(secret.id)}/env`, { allow, env_files: secret.env_files, env_requested_by: secret.env_requested_by });
      toast(allow ? `Allowed. Agents can copy ${secret.name} into .env files.` : (secret.env_files === 'allowed' ? 'Revoked. Copies already written stay until someone removes them or rotates the value.' : 'Dismissed.'), { kind: 'ok' });
    } catch (error) { errorToast(error, allow ? 'Couldn’t allow' : 'Couldn’t change it'); }
    reload();
  };
  const asked = secret.env_files === 'asked';
  const buttons = !secret.can_manage ? [] : asked ? [
    smallButton('Allow', `Allow copying ${secret.name} into .env files`, () => answer(true), { class: 'btn-primary' }),
    smallButton('Dismiss', `Dismiss copying ${secret.name} into .env files`, () => answer(false)),
  ] : [smallButton('Revoke', `Stop agents copying ${secret.name} into .env files`, () => answer(false))];
  // One answer covers every task, and for a secret of every sandbox every sandbox.
  const where = secret.scope === 'instance' ? 'every sandbox' : 'this sandbox';
  const text = asked
    ? `${secret.env_requested_by ? `Task ${secret.env_requested_by} asks` : 'An agent asks'} to copy its value into the project’s git-ignored .env, for code that runs outside ${BRAND.name}. Allowing lets every task in ${where} copy it.`
    : `Agents in ${where} may copy its value into git-ignored .env files`;
  return [h('div', { class: 'row-wrap' },
    iconBadge('file', '.env', asked ? 'warn' : '', asked ? 'Requested .env copy' : 'Allowed .env copies'),
    h('span', { class: 'small' }, text),
    buttons.length ? h('span', { class: 'row-wrap' }, buttons) : null)];
}

function mountAgents(ctx, sandbox, host, actions) {
  if (!sandbox) {
    setChildren(actions);
    setChildren(host, emptyState({ iconName: 'lock', title: 'Open a sandbox', text: 'Agent secrets belong to a sandbox (or to every sandbox). Open one to see and add them.' }));
    return;
  }
  const write = allows(sandbox.access, 'write');
  const list = h('div');
  setChildren(actions, createButton('Add agent secret', () => edit(null), { disabled: !write, title: write ? null : 'Requires write access' }));
  setChildren(host,
    h('p', { class: 'muted small' }, `Agents in ${sandbox.name} use this sandbox’s secrets and those for every sandbox.`),
    h('div', { class: 'banner banner-info' }, icon('shield'), h('div', { class: 'banner-text small' },
      `Agents use these without seeing them: ${BRAND.name} adds a secret to HTTPS requests for its hosts and types it into sign-in pages of those hosts. Agents find the hosts and ask; you allow them here. Anything that would show a value to an agent — command output, files, web pages, tool results — shows `,
      h('code', null, '[secret:NAME]'), ' instead.')),
    list);

  function render(secrets) {
    if (secrets.length === 0) return emptyState({ iconName: 'key', title: 'No agent secrets yet', text: 'Add an API key or a login an agent needs: its name, value and what it is for. Agents find where it goes and ask you to allow it.', action: write ? createButton('Add agent secret', () => edit(null)) : null });
    return h('div', { class: 'item-list' }, secrets.map((secret) => itemCard({
      iconName: 'key',
      title: h('span', { class: 'mono' }, secret.name),
      badges: [
        secret.scope === 'instance' ? iconBadge('globe', 'Every sandbox', 'violet') : iconBadge('folder', 'This sandbox', ''),
        secret.rotate_due ? badge('rotate', 'warn', 'Changed more than 180 days ago') : null,
      ].filter(Boolean),
      description: secret.description || h('span', { class: 'muted' }, 'No description'),
      extra: h('div', { class: 'stack-tight' },
        ...agentHosts(ctx, secret, reload),
        ...agentEnvFiles(ctx, secret, reload),
        h('span', { class: 'muted small' }, [secret.injection !== 'none' ? secret.injection_text : null, secret.last_used_at ? `used ${relLong(secret.last_used_at)}` : 'not used yet', `changed ${relLong(secret.value_updated_at)}`].filter(Boolean).join(' · '))),
      controls: [
        secret.can_manage ? smallButton('Show', `Show ${secret.name}`, async () => {
          try {
            const values = await withReauth(ctx, () => ctx.api.post(`/api/agent-secrets/${enc(secret.id)}/reveal`, { purpose: 'show' }));
            if (values) showRevealed(secret.name, values);
          } catch (error) { errorToast(error, 'Couldn’t reveal'); }
        }) : null,
        smallButton('Edit', `Edit ${secret.name}`, () => edit(secret), { disabled: !secret.can_manage, title: secret.can_manage ? `Edit ${secret.name}` : 'You cannot change this secret' }),
        iconButton('trash', `Delete ${secret.name}`, () => remove(secret), { class: 'btn-quiet-danger', disabled: !secret.can_manage }),
      ].filter(Boolean),
    })));
  }

  const reload = loader(list, 'the agent secrets', () => ctx.api.get(`/api/sandboxes/${enc(sandbox.id)}/agent-secrets`), render, { shape: 'items' });

  function edit(secret) {
    const form = agentForm(ctx, secret);
    openModal({ title: secret ? `Edit ${secret.name}` : 'Add agent secret', wide: true, body: form.body, actions: [
      { label: 'Cancel', value: false },
      { label: secret ? 'Save' : 'Add', kind: 'primary', submit: true, run: async () => {
        const values = form.values();
        if (!NAME_RE.test(values.name)) return invalid(form.name, 'Capital letters, digits and _, starting with a letter.');
        if (!secret && !values.value) return invalid(form.value, 'Enter the value.');
        if (values.value && [...values.value].length < 8) return invalid(form.value, 'At least 8 characters: shorter values cannot be recognised and hidden.');
        if (values.injection === 'header' && !values.header_name) return invalid(form.header, 'Name the header.');
        if (values.injection === 'basic' && !secret.has_username && !values.username) return invalid(form.username, 'Basic credentials need a user name.');
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
    } catch (error) { errorToast(error, 'Couldn’t delete'); }
  }
}

/** Mount the Secrets page: `/s/<slug>/secrets` (with the sandbox's agent secrets) or `/secrets`. */
export function mountSecretsPage(ctx, sandbox, route) {
  const frame = pageFrame({ title: 'Secrets', sub: 'team logins for people, keys agents use without seeing them' });
  const base = sandbox ? `/s/${encodeURIComponent(sandbox.slug)}/secrets` : '/secrets';
  const host = h('div', { class: 'stack' });
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
    if (current === 'agents') mountAgents(ctx, sandbox, host, frame.actions);
    else mountTeam(ctx, host, frame.actions);
  }
  show();
  return { el: frame.el, key: `secrets:${sandbox ? sandbox.id : ''}` };
}

