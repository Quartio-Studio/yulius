// Administration (ACC-004, ACC-009, ROT-001/002/009, ACC-006): users, provider accounts
// with the login terminal, and the GitHub App connection (manifest flow).

import { enc } from '../api.js';
import { h, badge, openModal, field, toast, confirmDialog, errorToast, toggle } from '../dom.js';
import { createTerminalView } from '../terminal.js';
import { accountUsage } from '../layout.js';
import { pageFrame, loader, dataTable } from './common.js';

function forbidden(ctx, what) {
  const frame = pageFrame({ title: what });
  frame.body.append(h('div', { class: 'card placeholder' }, 'You do not have permission to manage this.'));
  return { el: frame.el, key: `forbidden:${what}` };
}

// ---------------------------------------------------------------- users

/** Superadmin: admit users by GitHub login, disable them, mark instance admins. */
export function mountUsersPage(ctx) {
  if (!ctx.isSuperadmin()) return forbidden(ctx, 'Users');
  const frame = pageFrame({ title: 'Users', sub: 'only admitted GitHub users can sign in', actions: [h('button', { type: 'button', class: 'btn btn-primary btn-sm', onClick: () => add() }, 'Admit user')] });
  const host = h('div');
  frame.body.append(host);

  const patch = async (user, body, message) => {
    try {
      await ctx.api.patch(`/api/admin/users/${enc(user.id)}`, body);
      toast(message, { kind: 'ok' });
      return true;
    } catch (error) {
      errorToast(error, 'Could not update user');
      return false;
    }
  };

  // AdminUser = {id, github_login, display_name, is_instance_admin, disabled, github_linked};
  // the superadmin is not listed.
  const reload = loader(host, () => ctx.api.get('/api/admin/users'), (list) => dataTable([
    { label: 'GitHub login', render: (u) => h('strong', null, `@${u.github_login || '?'}`) },
    { label: 'Name', render: (u) => u.display_name || '—', phoneHide: true },
    { label: 'Instance admin', render: (u) => toggle({ checked: Boolean(u.is_instance_admin), label: `Instance admin for ${u.github_login}`, onChange: async (checked, input) => {
      if (!(await patch(u, { is_instance_admin: checked }, checked ? `@${u.github_login} is now an instance admin` : `@${u.github_login} is no longer an instance admin`))) input.checked = !checked;
    } }) },
    { label: 'Access', render: (u) => toggle({ checked: !u.disabled, label: `${u.disabled ? 'Enable' : 'Disable'} ${u.github_login}`, onChange: async (checked, input) => {
      if (!checked && !(await confirmDialog({ title: `Disable @${u.github_login}?`, message: 'They are signed out everywhere and their live terminal connections close.', confirmLabel: 'Disable', danger: true }))) { input.checked = true; return; }
      if (!(await patch(u, { disabled: !checked }, checked ? `@${u.github_login} enabled` : `@${u.github_login} disabled`))) input.checked = !checked;
      else u.disabled = !checked;
    } }) },
    { label: 'GitHub sign-in', render: (u) => (u.github_linked ? badge('signed in before', 'ok') : badge('never signed in')), phoneHide: true },
  ], Array.isArray(list) ? list : [], { empty: 'No users yet. Admit teammates by their GitHub login.' }));

  function add() {
    const login = h('input', { class: 'input', placeholder: 'octocat', autocomplete: 'off', autofocus: true });
    const name = h('input', { class: 'input', placeholder: 'optional' });
    const admin = h('input', { type: 'checkbox' });
    openModal({
      title: 'Admit user',
      body: h('div', { class: 'stack' }, field('GitHub login', login), field('Display name', name), h('label', { class: 'check' }, admin, h('span', null, 'Instance administrator (accounts, instance MCP and skills; never grants repository access)'))),
      actions: [
        { label: 'Cancel', value: false },
        { label: 'Admit', kind: 'primary', run: async () => {
          const value = login.value.trim().replace(/^@/, '');
          if (!/^[A-Za-z0-9-]{1,39}$/.test(value)) { toast('Enter a valid GitHub login.', { kind: 'warn' }); return false; }
          const body = { github_login: value };
          if (name.value.trim()) body.display_name = name.value.trim();
          if (admin.checked) body.is_instance_admin = true;
          await ctx.api.post('/api/admin/users', body);
          toast(`@${value} admitted`, { kind: 'ok' });
          reload();
          return true;
        } },
      ],
    });
  }
  return { el: frame.el, key: 'admin:users' };
}

// ---------------------------------------------------------------- accounts

/** Account login terminal modal (ROT-002): the provider's own sign-in over the terminal protocol. */
export function openLoginTerminal(ctx, loginId, label, onFinish) {
  let terminal = null;
  const status = h('span', { class: 'muted small' }, 'Complete the provider sign-in in the terminal below. Secrets are masked for other viewers.');
  const host = h('div', { style: { flex: 1, minHeight: '320px', display: 'flex', flexDirection: 'column', border: '1px solid var(--border)', borderRadius: '6px', overflow: 'hidden' } });
  const modal = openModal({
    title: `Sign in · ${label}`,
    full: true,
    body: h('div', { class: 'stack', style: { flex: 1, minHeight: 0 } }, status, host),
    actions: [{ label: 'Close' }],
    onClose: () => { if (terminal) terminal.destroy(); onFinish(); },
  });
  // The login PTY is a fixed 500 columns so tokens and URLs never wrap; the view scrolls
  // horizontally instead of resizing it. state.status: running → ok | error | cancelled.
  terminal = createTerminalView({
    transport: ctx.transport,
    path: `/api/admin/accounts/login/${enc(loginId)}/terminal`,
    onLink: (uri, event) => ctx.openLink(uri, event),
    readOnlyLabel: () => 'The login has finished.',
    followServerSize: true,
    onExit: (exit) => {
      const ok = exit.status === 'ok';
      const code = exit.code !== undefined && exit.code !== null ? `, exit code ${exit.code}` : '';
      status.textContent = ok
        ? 'Signed in: the credential is stored encrypted for this account. You can close this window.'
        : exit.status === 'cancelled'
          ? `The login was cancelled${code}.`
          : `The login ended without a credential (${exit.status}${code}). See the account's last error.`;
      status.className = ok ? 'small' : 'field-error';
    },
  });
  host.append(terminal.el);
  requestAnimationFrame(() => { terminal.start(); terminal.focus(); });
  return modal;
}

/** Instance admin: provider accounts with usage, enable/priority, re-login, add. */
export function mountAccountsPage(ctx) {
  if (!ctx.isInstanceAdmin()) return forbidden(ctx, 'Accounts');
  const frame = pageFrame({ title: 'Accounts', sub: 'organization subscriptions used for rotation', actions: [h('button', { type: 'button', class: 'btn btn-primary btn-sm', onClick: () => add() }, 'Add account')] });
  const host = h('div');
  frame.body.append(host, h('p', { class: 'muted xs' }, 'Rotation uses enabled accounts in priority order (lower first), switches before an account is exhausted, then falls back to the other provider.'));

  const patch = async (account, body) => {
    try {
      await ctx.api.patch(`/api/admin/accounts/${enc(account.id)}`, body);
      Object.assign(account, body);
      return true;
    } catch (error) {
      errorToast(error, 'Could not update account');
      return false;
    }
  };

  const reload = loader(host, () => ctx.api.get('/api/admin/accounts'), (list) => {
    const accounts = (Array.isArray(list) ? list : []).sort((a, b) => (a.provider || '').localeCompare(b.provider || '') || (a.priority ?? 0) - (b.priority ?? 0));
    if (accounts.length === 0) return h('div', { class: 'card placeholder' }, 'No accounts yet. Add a Claude or Codex subscription; you complete the provider’s own sign-in.');
    return h('div', { class: 'grid-2' }, accounts.map((account) => {
      const priority = h('input', { class: 'input', type: 'number', min: 0, max: 10000, value: String(account.priority ?? 0), style: { width: '80px', height: '24px' }, 'aria-label': `Priority of ${account.label}` });
      priority.addEventListener('change', async () => {
        const value = Number(priority.value);
        if (!Number.isInteger(value) || value < 0 || value > 10000) { priority.value = String(account.priority ?? 0); toast('Priority is a whole number from 0 to 10000.', { kind: 'warn' }); return; }
        if (await patch(account, { priority: value })) toast(`${account.label}: priority ${value}`, { kind: 'ok' });
        else priority.value = String(account.priority ?? 0);
      });
      return h('div', { class: 'card card-pad stack' },
        accountUsage(account),
        h('div', { class: 'row-wrap' },
          h('label', { class: 'row small' }, toggle({ checked: account.enabled, label: `${account.enabled ? 'Disable' : 'Enable'} ${account.label}`, onChange: async (checked, input) => {
            if (!(await patch(account, { enabled: checked }))) input.checked = !checked;
            else toast(`${account.label} ${checked ? 'enabled' : 'disabled'}`, { kind: 'ok' });
          } }), 'Enabled'),
          h('label', { class: 'row small' }, 'Priority', priority),
          h('span', { class: 'spacer' }),
          h('button', { type: 'button', class: ['btn', 'btn-sm', account.status === 'relogin' && 'btn-primary'], onClick: async () => {
            try {
              const result = await ctx.api.post(`/api/admin/accounts/${enc(account.id)}/relogin`);
              openLoginTerminal(ctx, result.login_id, account.label, reload);
            } catch (error) {
              errorToast(error, 'Could not start re-login');
            }
          } }, 'Re-login'),
          h('button', { type: 'button', class: 'btn btn-sm btn-danger', onClick: async () => {
            if (!(await confirmDialog({ title: `Remove ${account.label}?`, message: account.in_use_by ? `${account.in_use_by} live task(s) use it: removal is refused until they stop or switch away.` : 'Its stored credentials are deleted and a running login is cancelled.', confirmLabel: 'Remove', danger: true }))) return;
            try {
              await ctx.api.del(`/api/admin/accounts/${enc(account.id)}`);
              toast(`${account.label} removed`, { kind: 'ok' });
              reload();
            } catch (error) {
              errorToast(error, 'Could not remove account');
            }
          } }, 'Remove')));
    }));
  });

  function add() {
    const provider = h('select', { class: 'select' }, h('option', { value: 'anthropic' }, 'Claude (Anthropic)'), h('option', { value: 'openai' }, 'Codex (OpenAI)'));
    const label = h('input', { class: 'input', placeholder: 'Claude #3', maxlength: 80, autofocus: true });
    openModal({
      title: 'Add account',
      body: h('div', { class: 'stack' }, field('Provider', provider), field('Label', label, 'Shown in usage bars and task details.'), h('p', { class: 'muted small' }, 'Next, a login terminal opens with the provider’s own sign-in (claude setup-token or codex login). Nothing is created when the login cannot start.')),
      actions: [
        { label: 'Cancel', value: false },
        { label: 'Continue to sign-in', kind: 'primary', run: async () => {
          if (!label.value.trim()) { toast('Give the account a label.', { kind: 'warn' }); return false; }
          const result = await ctx.api.post('/api/admin/accounts', { provider: provider.value, label: label.value.trim() });
          reload();
          openLoginTerminal(ctx, result.login_id, result.account ? result.account.label : label.value.trim(), reload);
          return true;
        } },
      ],
    });
  }

  const off = ctx.store.on('account', () => reload());
  return { el: frame.el, key: 'admin:accounts', destroy: off };
}

// ---------------------------------------------------------------- GitHub

/** Superadmin: GitHub App connection via the manifest flow (J1 step 3, ACC-006). */
export function mountGithubPage(ctx) {
  if (!ctx.isSuperadmin()) return forbidden(ctx, 'GitHub connection');
  const frame = pageFrame({ title: 'GitHub connection', sub: 'the instance is bound to one GitHub organization or personal account' });
  const host = h('div');
  frame.body.append(host);
  // GET /api/admin/github is documented but this server build does not route it yet: on a
  // 404 the page shows what /api/me knows about the connection instead of an error.
  const fetchConfig = () => ctx.api.get('/api/admin/github').catch((error) => {
    if (error.status === 404) return { unsupported: true };
    throw error;
  });
  loader(host, fetchConfig, (config) => {
    if (config && config.unsupported) return instanceSummary(ctx);
    if (config && config.configured) {
      return h('div', { class: 'card card-pad stack', style: { maxWidth: '640px' } },
        h('div', { class: 'row' }, badge('connected', 'ok'), h('strong', null, config.owner_login || '?')),
        h('dl', { class: 'kv' }, h('dt', null, 'GitHub App'), h('dd', null, config.app_slug || '—'), h('dt', null, 'Owner'), h('dd', null, config.owner_login || '—')),
        config.install_url ? h('div', null, h('a', { class: 'btn btn-primary', href: config.install_url, target: '_blank', rel: 'noopener noreferrer' }, 'Install / configure repositories')) : null,
        h('p', { class: 'muted small' }, 'Repository permissions on GitHub decide who sees and controls each sandbox. Next: admit users and connect provider accounts.'));
    }
    return manifestForm(ctx);
  });
  return { el: frame.el, key: 'admin:github' };
}

/** Connection facts from `/api/me` when the GitHub admin endpoints are not available. */
function instanceSummary(ctx) {
  const instance = (ctx.store.state.me && ctx.store.state.me.instance) || {};
  const connected = instance.github_configured === true;
  return h('div', { class: 'card card-pad stack', style: { maxWidth: '640px' } },
    h('div', { class: 'row' }, connected ? badge('connected', 'ok') : badge('not connected', 'warn'), instance.owner_login ? h('strong', null, instance.owner_login) : null, instance.owner_kind ? badge(instance.owner_kind) : null),
    connected
      ? h('p', { class: 'text-2' }, 'This instance has a GitHub App. Repository permissions on GitHub decide who sees and controls each sandbox. Next: admit users and connect provider accounts.')
      : h('p', { class: 'text-2' }, 'This instance has no GitHub App yet, so projects cannot be imported or created.'),
    h('p', { class: 'muted small' }, 'GitHub App setup (the manifest flow) is not available in this server version: GET /api/admin/github answered 404.'));
}

function manifestForm(ctx) {
  const owner = h('input', { class: 'input', placeholder: 'Quartio-Studio', autocomplete: 'off' });
  const kindInputs = [['org', 'Organization'], ['user', 'Personal account']].map(([value, label]) => ({ input: h('input', { type: 'radio', name: 'owner-kind', value, checked: value === 'org' }), label }));
  const submit = h('button', { type: 'button', class: 'btn btn-primary' }, 'Create GitHub App on GitHub');
  submit.addEventListener('click', async () => {
    const ownerLogin = owner.value.trim();
    if (!/^[A-Za-z0-9-]{1,39}$/.test(ownerLogin)) { toast('Enter the GitHub organization or user name.', { kind: 'warn' }); owner.focus(); return; }
    submit.disabled = true;
    submit.classList.add('busy');
    try {
      const result = await ctx.api.post('/api/admin/github/manifest', { owner_login: ownerLogin, owner_kind: kindInputs.find((k) => k.input.checked).input.value });
      if (ctx.mock) { toast(`Mock: would POST the manifest to ${result.post_url}`, { kind: 'info', timeout: 8000 }); return; }
      // The browser posts the manifest form to GitHub (GitHub App manifest flow).
      const form = h('form', { method: 'post', action: result.post_url, hidden: true },
        h('input', { type: 'hidden', name: 'manifest', value: typeof result.manifest === 'string' ? result.manifest : JSON.stringify(result.manifest) }));
      document.body.append(form);
      form.submit();
    } catch (error) {
      errorToast(error, 'Could not prepare the GitHub App manifest');
    } finally {
      submit.disabled = false;
      submit.classList.remove('busy');
    }
  });
  return h('div', { class: 'card card-pad stack', style: { maxWidth: '640px' } },
    h('div', { class: 'row' }, badge('not connected', 'warn')),
    h('p', { class: 'text-2' }, 'Yulius creates a GitHub App for this instance using GitHub’s manifest flow. You confirm the app on GitHub, then install it on the repositories Yulius may manage.'),
    field('Owner (organization or user)', owner),
    h('div', { class: 'seg', role: 'radiogroup', 'aria-label': 'Owner kind' }, kindInputs.map((k) => h('label', null, k.input, k.label))),
    h('div', null, submit));
}

