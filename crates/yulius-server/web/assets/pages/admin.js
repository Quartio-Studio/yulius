// Administration (ACC-004, ACC-009, ROT-001/002/009/013, ACC-006): users, the Claude
// subscription pool with the login terminal, and the GitHub App connection (manifest flow).

import { enc } from '../api.js';
import { h, badge, openModal, field, invalid, toast, confirmDialog, errorToast, toggle, setChildren, emptyState, skeleton, bar } from '../dom.js';
import { compact } from '../format.js';
import { usd } from '../charts.js';
import { createTerminalView } from '../terminal.js';
import { accountUsage, capacityPanel } from '../layout.js';
import { pageFrame, loader, dataTable, createButton } from './common.js';
import { recoveryCodeCard } from './auth.js';
import { BRAND } from '../brand.js';
import { withReauth } from './secrets.js';

function forbidden(ctx, what) {
  const frame = pageFrame({ title: what });
  frame.body.append(h('div', { class: 'card placeholder' }, 'You do not have permission to manage this.'));
  return { el: frame.el, key: `forbidden:${what}` };
}

// ---------------------------------------------------------------- users

const TWO_FACTOR = { on: ['On', 'ok'], pending: ['Being set up', 'warn'], missing: ['Off', null] };

/**
 * Users. The superadmin admits users by GitHub login, disables them and marks instance admins;
 * instance admins see the list. Both reset a person's lost two-factor (SPEC-061), after their own
 * two-factor code.
 */
export function mountUsersPage(ctx) {
  if (!ctx.isInstanceAdmin()) return forbidden(ctx, 'Users');
  const superadmin = ctx.isSuperadmin();
  const me = ctx.store.state.me && ctx.store.state.me.user ? ctx.store.state.me.user.id : null;
  const frame = pageFrame({ title: 'Users', sub: 'who may sign in: only admitted GitHub users', actions: superadmin ? [createButton('Admit user', () => add())] : [] });
  const host = h('div');
  if (superadmin) frame.body.append(recoveryCard(ctx));
  frame.body.append(host);

  const patch = async (user, body, message) => {
    try {
      await ctx.api.patch(`/api/admin/users/${enc(user.id)}`, body);
      toast(message, { kind: 'ok' });
      return true;
    } catch (error) {
      errorToast(error, 'Couldn’t update user');
      return false;
    }
  };

  // A lost authenticator (SPEC-061): confirm, then the administrator's own code when the vault is
  // locked; the person sets two-factor up again on their next reveal.
  const resetTwoFactor = async (u) => {
    if (!(await confirmDialog({ title: `Reset two-factor of @${u.github_login}?`, message: `Their authenticator stops opening the secrets and their unlocks end. They set two-factor up again, after a recent sign-in, the next time they reveal a secret. The reset is recorded in the secrets activity.`, confirmLabel: 'Reset', danger: true }))) return;
    try {
      // `null`: the administrator cancelled the code dialog (a 204 resolves `true` here).
      if (!(await withReauth(ctx, () => ctx.api.del(`/api/users/${enc(u.id)}/two-factor`).then(() => true)))) return;
      toast(`Two-factor of @${u.github_login} reset.`, { kind: 'ok' });
      reload({ quiet: true });
    } catch (error) {
      errorToast(error, 'Couldn’t reset two-factor');
    }
  };

  // AdminUser = {id, github_login, display_name, is_instance_admin, disabled, github_linked,
  // two_factor}; the superadmin is not listed.
  const reload = loader(host, 'the users', () => ctx.api.get('/api/admin/users'), (list) => dataTable([
    { label: 'GitHub login', render: (u) => h('strong', null, `@${u.github_login || '?'}`) },
    { label: 'Name', render: (u) => u.display_name || '—', phoneHide: true },
    { label: 'Instance admin', render: (u) => (superadmin ? toggle({ checked: Boolean(u.is_instance_admin), label: `Instance admin for ${u.github_login}`, onChange: async (checked, input) => {
      if (!(await patch(u, { is_instance_admin: checked }, checked ? `@${u.github_login} is now an instance admin.` : `@${u.github_login} is no longer an instance admin.`))) input.checked = !checked;
    } }) : (u.is_instance_admin ? 'Yes' : 'No')) },
    { label: 'Access', render: (u) => (superadmin ? toggle({ checked: !u.disabled, label: `Access for ${u.github_login}`, onChange: async (checked, input) => {
      if (!checked && !(await confirmDialog({ title: `Disable @${u.github_login}?`, message: 'They are signed out everywhere and their live terminal connections close.', confirmLabel: 'Disable', danger: true }))) { input.checked = true; return; }
      if (!(await patch(u, { disabled: !checked }, checked ? `@${u.github_login} enabled.` : `@${u.github_login} disabled.`))) input.checked = !checked;
      else u.disabled = !checked;
    } }) : (u.disabled ? badge('Disabled', 'danger') : 'Allowed')) },
    { label: 'GitHub sign-in', render: (u) => (u.github_linked ? badge('Signed in before', 'ok') : badge('Never signed in')), phoneHide: true },
    { label: 'Two-factor', render: (u) => {
      const [text, kind] = TWO_FACTOR[u.two_factor] || TWO_FACTOR.missing;
      return h('div', { class: 'row' }, badge(text, kind),
        u.two_factor && u.two_factor !== 'missing' && u.id !== me
          ? h('button', { type: 'button', class: 'btn btn-sm btn-quiet-danger', 'aria-label': `Reset two-factor of @${u.github_login}`, title: 'For a lost authenticator: they set two-factor up again', onClick: () => resetTwoFactor(u) }, 'Reset')
          : null);
    } },
  ], Array.isArray(list) ? list : [], { empty: emptyState({ iconName: 'users', title: 'No users yet', text: superadmin ? 'Admit teammates by their GitHub login; they sign in with GitHub.' : 'The superadmin admits teammates by their GitHub login.', action: superadmin ? createButton('Admit user', () => add()) : null }) }), { shape: ['table', 6] });

  function add() {
    const login = h('input', { class: 'input', placeholder: 'octocat', autocomplete: 'off', autofocus: true });
    const name = h('input', { class: 'input', placeholder: 'optional' });
    const admin = h('input', { type: 'checkbox' });
    openModal({
      title: 'Admit user',
      body: h('div', { class: 'stack' }, field('GitHub login', login), field('Display name', name), h('label', { class: 'check' }, admin, h('span', null, 'Instance administrator (subscriptions, MCP and skills for every sandbox; never grants repository access)'))),
      actions: [
        { label: 'Cancel', value: false },
        { label: 'Admit', kind: 'primary', run: async () => {
          const value = login.value.trim().replace(/^@/, '');
          if (!/^[A-Za-z0-9-]{1,39}$/.test(value)) return invalid(login, 'Enter a valid GitHub login.');
          const body = { github_login: value };
          if (name.value.trim()) body.display_name = name.value.trim();
          if (admin.checked) body.is_instance_admin = true;
          await ctx.api.post('/api/admin/users', body);
          toast(`@${value} admitted.`, { kind: 'ok' });
          reload();
          return true;
        } },
      ],
    });
  }
  return { el: frame.el, key: 'admin:users' };
}

/** Tokens Yulius observed on an account (24 h, 7 d); unknown stays unknown. Numbers as everywhere (compact: 23.4K). */
function tokenTotals(account) {
  const fmt = (n) => (n == null ? 'unknown' : compact(n));
  const row = (label, t) => {
    if (!t) return null;
    const total = [t.input, t.output, t.cache_read, t.cache_write].some((v) => v != null)
      ? (t.input || 0) + (t.output || 0) + (t.cache_read || 0) + (t.cache_write || 0)
      : null;
    return h('div', { class: 'kv-line' }, h('span', { class: 'muted' }, label),
      h('span', null, `${fmt(total)} tokens · ${t.runs} run${t.runs === 1 ? '' : 's'} · ${t.tasks} task${t.tasks === 1 ? '' : 's'}${Number.isFinite(t.cost_usd) ? ` · ≈ ${usd(t.cost_usd)} at API prices` : ''}`),
      total != null ? h('span', { class: 'muted xs' }, `in ${fmt(t.input)} · out ${fmt(t.output)} · cache ${fmt((t.cache_read || 0) + (t.cache_write || 0))}`) : null);
  };
  const rows = [row('Last 24 h', account.tokens_24h), row('Last 7 days', account.tokens_7d)].filter(Boolean);
  return rows.length ? h('div', { class: 'stack-sm token-totals' }, h('div', { class: 'muted xs' }, `Observed by ${BRAND.name}`), rows) : null;
}

/** Superadmin recovery code (ACC-011): status and a password-confirmed regeneration. */
export function recoveryCard(ctx) {
  const state = h('div', { class: 'text-2' }, skeleton('Checking the recovery code…', 1));
  const generate = h('button', { type: 'button', class: 'btn btn-sm' }, 'Generate new code…');
  const card = h('div', { class: 'card card-pad stack-sm' }, h('h2', { class: 'card-title' }, 'Superadmin recovery code'), state, h('div', { class: 'row' }, generate));
  const refresh = () => ctx.api.get('/api/auth/recovery').then((s) => {
    state.textContent = s.available
      ? `A recovery code is set (${s.attempts_left} of 3 attempts left). Generating a new one replaces it.`
      : s.locked
        ? 'Recovery is disabled after three wrong codes. Generate a new code to enable it again.'
        : 'No recovery code yet: without one, a forgotten superadmin password can’t be reset. Generate one and keep it perfectly safe.';
    card.classList.toggle('card-warn', !s.available);
  }).catch(() => { state.textContent = 'Couldn’t load the recovery status.'; });
  generate.addEventListener('click', async () => {
    const password = h('input', { class: 'input', type: 'password', autocomplete: 'current-password' });
    const modal = openModal({
      title: 'Generate a new recovery code',
      body: h('div', { class: 'stack-sm' }, h('p', { class: 'text-2' }, 'The current code stops working. Confirm with the superadmin password.'), field('Superadmin password', password)),
      actions: [{ label: 'Cancel', value: false }, { label: 'Generate', kind: 'primary', value: 'go' }],
    });
    setTimeout(() => password.focus(), 30);
    if ((await modal.result) !== 'go') return;
    try {
      const result = await ctx.api.post('/api/admin/recovery-code', { password: password.value });
      const shown = openModal({ title: 'New recovery code', body: h('div'), actions: [], dismissible: false });
      shown.body.replaceChildren(recoveryCodeCard(result.recovery_code, { title: 'Your new recovery code', continueLabel: 'Done', onContinue: () => shown.close(true) }));
      refresh();
    } catch (error) {
      errorToast(error, 'Couldn’t generate a recovery code');
    }
  });
  refresh();
  return card;
}

// ---------------------------------------------------------------- accounts

/** Account login terminal modal (ROT-002): the provider's own sign-in over the terminal protocol. */
export function openLoginTerminal(ctx, loginId, label, onFinish) {
  let terminal = null;
  const status = h('span', { class: 'muted small' }, 'Complete the provider sign-in in the terminal below. Secrets are masked for other viewers.');
  const host = h('div', { class: 'login-terminal' });
  const modal = openModal({
    title: `Sign in · ${label}`,
    full: true,
    body: h('div', { class: 'stack fill-column' }, status, host),
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
        ? 'Signed in: the credential is stored encrypted for this subscription. You can close this window.'
        : exit.status === 'cancelled'
          ? `The login was cancelled${code}.`
          : `The login ended without a credential (${exit.status}${code}). See the subscription's last error.`;
      status.className = ok ? 'small' : 'field-error';
    },
  });
  host.append(terminal.el);
  requestAnimationFrame(() => { terminal.start(); terminal.focus(); });
  return modal;
}

/**
 * Instance admin: the Claude subscriptions as one pool (ROT-009, ROT-013, F 2026-10-10). Add a
 * subscription (Claude's own sign-in), see its usage, remove it — nothing else to manage: Yulius
 * places, switches and pauses tasks by itself. The capacity panel on top says whether to buy more.
 */
export function mountAccountsPage(ctx, route) {
  if (!ctx.isInstanceAdmin()) return forbidden(ctx, 'Subscriptions');
  // One name for one thing (#147): a Claude "subscription" (the API's account).
  const frame = pageFrame({ title: 'Subscriptions', sub: `Claude subscriptions in one pool: ${BRAND.name} places, switches and pauses tasks by itself`, actions: [createButton('Add subscription', () => add())] });
  const capacityHost = h('section', { class: 'card card-pad stack', 'aria-label': 'Capacity' },
    h('h2', { class: 'card-title' }, 'Capacity'), skeleton('Loading the capacity…', 3));
  const host = h('div');
  frame.body.append(capacityHost, host, h('p', { class: 'muted xs' }, `Tasks don't belong to a subscription: ${BRAND.name} starts each one on the subscription whose capacity expires soonest and moves it to another one by itself, at the end of a turn, when its subscription reaches 98%. A task pauses only when no subscription has capacity left — without telling the agent — and continues by itself, oldest first, when a window resets or a subscription is added. Usage comes from the running tasks, so an idle subscription shows it once a task runs on it.`));

  // Resolves true once the panel shows the capacity, false when it says it is not available.
  const loadCapacity = () => ctx.api.get('/api/capacity').then((view) => {
    setChildren(capacityHost, h('h2', { class: 'card-title' }, 'Capacity'), capacityPanel(view));
    return true;
  }).catch((error) => {
    setChildren(capacityHost, h('h2', { class: 'card-title' }, 'Capacity'), h('p', { class: 'muted small' }, `Capacity is not available: ${error.message || 'unknown error'}`));
    return false;
  });

  // Capacity is read beside the subscriptions, so the panel above them is in place before they
  // land (SPEC-120). Each later render (after an add or a removal) reads it again, and so does
  // the first one when that read failed.
  const firstCapacity = loadCapacity();
  let rendered = false;
  const reload = loader(host, 'the subscriptions', () => ctx.api.get('/api/admin/accounts'), (list) => {
    if (rendered) loadCapacity();
    else firstCapacity.then((shown) => { if (!shown) loadCapacity(); });
    rendered = true;
    const accounts = (Array.isArray(list) ? list : []).sort((a, b) => (a.label || '').localeCompare(b.label || ''));
    if (accounts.length === 0) return emptyState({ iconName: 'user', title: 'No subscriptions yet', text: 'Add a Claude subscription for the agents; you finish Claude’s own sign-in in a terminal here.', action: createButton('Add subscription', () => add()) });
    return h('div', { class: 'grid-2' }, accounts.map((account) => {
      // The name is edited in place (SPEC-244 SUB-001); the server trims and validates it.
      const rename = async (label) => {
        const fresh = await ctx.api.patch(`/api/admin/accounts/${enc(account.id)}`, { label });
        Object.assign(account, fresh);
        toast(`Subscription renamed to ${account.label}.`, { kind: 'ok' });
      };
      // Provider windows (refreshed in the background) and what Yulius observed (ROT-009).
      const usageHost = h('div', null, accountUsage(account, { rename, showEmail: true }), tokenTotals(account));
      const stale = !account.usage || !account.usage.observed_at || Date.now() - Date.parse(account.usage.observed_at) > 10 * 60 * 1000;
      if (account.status !== 'pending_login' && stale) {
        ctx.api.post(`/api/admin/accounts/${enc(account.id)}/usage`).then((fresh) => {
          Object.assign(account, fresh);
          if (usageHost.querySelector('.account-rename-input')) pendingReload = true;
          else setChildren(usageHost, accountUsage(account, { rename, showEmail: true }), tokenTotals(account));
        }).catch(() => {});
      }
      const broken = ['relogin', 'error', 'pending_login'].includes(account.status);
      return h('div', { class: 'card card-pad stack' },
        usageHost,
        h('div', { class: 'row-wrap' },
          h('span', { class: 'spacer' }),
          // A broken credential is repaired with Claude's sign-in; there is nothing else to set.
          broken ? h('button', { type: 'button', class: 'btn btn-sm btn-primary', onClick: async () => {
            try {
              const result = await ctx.api.post(`/api/admin/accounts/${enc(account.id)}/relogin`);
              openLoginTerminal(ctx, result.login_id, account.label, reload);
            } catch (error) {
              errorToast(error, 'Couldn’t start the sign-in');
            }
          } }, 'Sign in again') : null,
          // Removal is refused while tasks use the subscription: say so instead of offering it (UX2-12).
          h('button', { type: 'button', class: 'btn btn-sm btn-quiet-danger', disabled: account.in_use_by > 0, title: account.in_use_by > 0 ? `${account.in_use_by} live task${account.in_use_by === 1 ? '' : 's'} use this subscription; they move away at their next turn end or when stopped` : null, onClick: async () => {
            if (!(await confirmDialog({ title: `Remove ${account.label}?`, message: 'Its stored credential is deleted and a running sign-in is cancelled. Done tasks on it are closed (Continue starts them on another subscription), and tasks never start on it again.', confirmLabel: 'Remove', danger: true }))) return;
            try {
              await ctx.api.del(`/api/admin/accounts/${enc(account.id)}`);
              toast(`Subscription ${account.label} removed.`, { kind: 'ok' });
              reload();
            } catch (error) {
              errorToast(error, 'Couldn’t remove the subscription');
            }
          } }, 'Remove')));
    }));
  }, { shape: 'cards' });

  function add() {
    const label = h('input', { class: 'input', placeholder: 'Claude #3', maxlength: 80, autofocus: true });
    openModal({
      title: 'Add a Claude subscription',
      body: h('div', { class: 'stack' }, field('Name', label, 'Shown on usage bars and in task details; you can rename it later.'), h('p', { class: 'muted small' }, 'Next, a terminal opens with Claude’s own sign-in (claude setup-token). Tasks waiting for capacity continue on it as soon as it is signed in.')),
      actions: [
        { label: 'Cancel', value: false },
        { label: 'Continue to sign-in', kind: 'primary', run: async () => {
          if (!label.value.trim()) return invalid(label, 'Give the subscription a name.');
          const result = await ctx.api.post('/api/admin/accounts', { provider: 'anthropic', label: label.value.trim() });
          reload();
          openLoginTerminal(ctx, result.login_id, result.account ? result.account.label : label.value.trim(), reload);
          return true;
        } },
      ],
    });
  }

  // A name being edited is not redrawn away by an update: the reload waits until the field
  // closes (the reload keeps focus on the same control).
  let pendingReload = false;
  const off = ctx.store.on('account', () => {
    if (host.querySelector('.account-rename-input')) pendingReload = true;
    else reload({ quiet: true });
  });
  host.addEventListener('focusout', () => setTimeout(() => {
    if (pendingReload && !host.querySelector('.account-rename-input')) {
      pendingReload = false;
      reload({ quiet: true });
    }
  }, 0));
  // Settings → "Add subscription" lands here with ?add=1: open the dialog right away.
  if (route && route.query && route.query.get('add') === '1') {
    setTimeout(add, 0);
    ctx.navigate('/admin/accounts', { replace: true });
  }
  return { el: frame.el, key: 'admin:accounts', destroy: off };
}

// ---------------------------------------------------------------- GitHub

/** Superadmin: GitHub App connection via the manifest flow (J1 step 3, ACC-006). */
export function mountGithubPage(ctx) {
  if (!ctx.isSuperadmin()) return forbidden(ctx, 'GitHub connection');
  const frame = pageFrame({ title: 'GitHub connection', sub: 'the GitHub organization or account this instance is bound to' });
  const host = h('div');
  frame.body.append(host);
  // The manifest callback returns here with ?error=… when GitHub's conversion failed. The text is
  // fixed: a crafted link must not put its own words on this page (like the sign-in page).
  if (new URLSearchParams(window.location.search).has('error')) {
    frame.body.prepend(h('div', { class: 'banner banner-danger', role: 'alert' }, badge('Setup failed', 'danger'),
      h('span', { class: 'banner-text' }, 'Creating the GitHub App did not finish. Start again below; the server log has the details.')));
  }
  loader(host, 'the GitHub connection', () => ctx.api.get('/api/admin/github'), (config) => {
    if (config && config.configured) {
      return h('div', { class: 'card card-pad stack' },
        h('div', { class: 'row' }, badge('Connected', 'ok'), h('strong', null, config.owner_login || '?')),
        h('dl', { class: 'kv' }, h('dt', null, 'GitHub App'), h('dd', null, config.app_slug || '—'), h('dt', null, 'Owner'), h('dd', null, config.owner_login || '—')),
        config.install_url ? h('div', null, h('a', { class: 'btn btn-primary', href: config.install_url, target: '_blank', rel: 'noopener noreferrer' }, 'Install / configure repositories')) : null,
        h('p', { class: 'muted small' }, 'Repository permissions on GitHub decide who sees and controls each sandbox. Next: admit users and add subscriptions.'));
    }
    return manifestForm(ctx);
  }, { shape: githubLoading });
  return { el: frame.el, key: 'admin:github' };
}

/**
 * The connection while it loads (SPEC-120, #449), drawn as the connected card most instances
 * show: its state line, the App and owner, the next step.
 */
function githubLoading() {
  return h('div', { class: 'card card-pad stack' },
    h('div', { class: 'row' }, bar('md'), bar('lg')),
    h('dl', { class: 'kv' }, h('dt', null, bar('md')), h('dd', null, bar(40)), h('dt', null, bar('sm')), h('dd', null, bar(30))),
    h('div', null, h('span', { class: 'skeleton-button' })),
    h('p', { class: 'muted small' }, bar(70)));
}

function manifestForm(ctx) {
  const owner = h('input', { class: 'input', placeholder: 'Quartio-Studio', autocomplete: 'off' });
  const kindInputs = [['org', 'Organization'], ['user', 'Personal account']].map(([value, label]) => ({ input: h('input', { type: 'radio', name: 'owner-kind', value, checked: value === 'org' }), label }));
  const submit = h('button', { type: 'button', class: 'btn btn-primary' }, 'Create GitHub App on GitHub');
  submit.addEventListener('click', async () => {
    const ownerLogin = owner.value.trim();
    if (!/^[A-Za-z0-9-]{1,39}$/.test(ownerLogin)) { invalid(owner, 'Enter the GitHub organization or user name.'); return; }
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
      errorToast(error, 'Couldn’t prepare the GitHub App manifest');
    } finally {
      submit.disabled = false;
      submit.classList.remove('busy');
    }
  });
  return h('div', { class: 'card card-pad stack' },
    h('div', { class: 'row' }, badge('Not connected', 'warn')),
    h('p', { class: 'text-2' }, `${BRAND.name} creates a GitHub App for this instance using GitHub’s manifest flow. You confirm the app on GitHub, then install it on the repositories ${BRAND.name} may manage.`),
    field('Owner (organization or user)', owner),
    h('div', { class: 'seg', role: 'radiogroup', 'aria-label': 'Owner kind' }, kindInputs.map((k) => h('label', null, k.input, k.label))),
    h('div', null, submit));
}

