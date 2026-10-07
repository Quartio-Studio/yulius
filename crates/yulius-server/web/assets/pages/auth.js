// Standalone pages shown before a session exists: setup wizard (J1, ACC-001..003) and
// sign-in (GitHub OAuth + superadmin password, ACC-005).

import { h, icon, field, setChildren, errorToast } from '../dom.js';

const MIN_PASSWORD = 12;

/** Setup wizard: token, password + confirmation, non-recoverable warning (ACC-002). */
export function mountSetup(ctx, { onDone }) {
  const token = h('input', { class: 'input mono', type: 'text', autocomplete: 'one-time-code', spellcheck: 'false', placeholder: 'printed on the VM by `yulius serve`', autofocus: true });
  const password = h('input', { class: 'input', type: 'password', autocomplete: 'new-password', minlength: MIN_PASSWORD });
  const confirm = h('input', { class: 'input', type: 'password', autocomplete: 'new-password' });
  const show = h('input', { type: 'checkbox' });
  const ackRecover = h('input', { type: 'checkbox', id: 'ack-recover' });
  const ackStored = h('input', { type: 'checkbox', id: 'ack-stored' });
  const problems = h('div', { class: 'field-error', role: 'alert' });
  const submit = h('button', { type: 'submit', class: 'btn btn-primary', disabled: true }, 'Create superadmin');

  const validate = () => {
    const issues = [];
    if (password.value && password.value.length < MIN_PASSWORD) issues.push(`Use at least ${MIN_PASSWORD} characters.`);
    if (confirm.value && confirm.value !== password.value) issues.push('The passwords do not match.');
    problems.textContent = issues.join(' ');
    const ok = token.value.trim() && password.value.length >= MIN_PASSWORD && confirm.value === password.value && ackRecover.checked && ackStored.checked;
    submit.disabled = !ok;
    return ok;
  };
  for (const input of [token, password, confirm]) input.addEventListener('input', validate);
  for (const box of [ackRecover, ackStored]) box.addEventListener('change', validate);
  show.addEventListener('change', () => { password.type = confirm.type = show.checked ? 'text' : 'password'; });

  const form = h('form', { class: 'auth-card', novalidate: true },
    h('input', { type: 'text', name: 'username', autocomplete: 'username', value: 'superadmin', hidden: true, readOnly: true, tabindex: '-1', 'aria-hidden': 'true' }),
    h('div', { class: 'auth-logo' }, 'Yulius'),
    h('h1', { class: 'auth-title' }, 'Set up this instance'),
    h('p', { class: 'text-2' }, 'Create the single superadmin. You need the one-time setup token printed on the server console.'),
    field('Setup token', token),
    field('Superadmin password', password, `At least ${MIN_PASSWORD} characters.`),
    field('Repeat password', confirm),
    h('label', { class: 'check small' }, show, h('span', null, 'Show passwords')),
    h('div', { class: 'warning-box', role: 'note' }, icon('warn', { size: 18 }),
      h('div', { class: 'stack-sm' },
        h('strong', null, 'This password cannot be recovered.'),
        h('span', null, 'There is no email reset, no “forgot password” and no recovery code. If you lose it, nobody can sign in as superadmin.'),
        h('span', null, 'Store it in several safe places — for example a password manager and a sealed paper copy — before you continue.'))),
    h('label', { class: 'check', for: 'ack-recover' }, ackRecover, h('span', null, 'I understand the superadmin password cannot be recovered.')),
    h('label', { class: 'check', for: 'ack-stored' }, ackStored, h('span', null, 'I have stored the password in several safe places.')),
    problems,
    submit);

  form.addEventListener('submit', async (event) => {
    event.preventDefault();
    if (!validate()) return;
    submit.disabled = true;
    submit.classList.add('busy');
    try {
      const me = await ctx.api.post('/api/setup', { token: token.value.trim(), password: password.value, password_confirm: confirm.value, confirm_saved: true }, { allow401: true });
      password.value = '';
      confirm.value = '';
      onDone(me);
    } catch (error) {
      errorToast(error, 'Setup failed');
      submit.disabled = false;
    } finally {
      submit.classList.remove('busy');
    }
  });

  return { el: h('div', { class: 'auth' }, form) };
}

/** Sign-in: GitHub (all users) and a compact superadmin password form. */
export function mountSignIn(ctx, { next, onDone, error }) {
  const nextPath = next && next.startsWith('/') && !next.startsWith('//') ? next : '/';
  const githubHref = `/api/auth/github/start?next=${encodeURIComponent(nextPath)}`;
  const password = h('input', { class: 'input', type: 'password', autocomplete: 'current-password', 'aria-label': 'Superadmin password' });
  const submit = h('button', { type: 'submit', class: 'btn' }, 'Sign in');
  const errorBox = h('div', { class: 'field-error', role: 'alert' }, error || '');
  const username = h('input', { type: 'text', name: 'username', autocomplete: 'username', value: 'superadmin', hidden: true, readOnly: true, tabindex: '-1', 'aria-hidden': 'true' });
  const form = h('form', { class: 'stack-sm' }, username, h('div', { class: 'row' }, password, submit));
  form.addEventListener('submit', async (event) => {
    event.preventDefault();
    if (!password.value) return;
    submit.disabled = true;
    submit.classList.add('busy');
    errorBox.textContent = '';
    try {
      const me = await ctx.api.post('/api/auth/password', { password: password.value }, { allow401: true });
      password.value = '';
      onDone(me, nextPath);
    } catch (err) {
      errorBox.textContent = err.status === 401 ? 'Wrong password.' : err.message;
      password.select();
    } finally {
      submit.disabled = false;
      submit.classList.remove('busy');
    }
  });
  const githubButton = h('a', { class: 'btn btn-primary btn-github', href: githubHref }, 'Sign in with GitHub');
  if (ctx.mock) {
    githubButton.addEventListener('click', async (event) => {
      event.preventDefault();
      const me = await ctx.api.post('/api/auth/mock-github', {});
      onDone(me, nextPath);
    });
  }
  const details = h('details', null, h('summary', { class: 'muted small', style: { cursor: 'pointer' } }, 'Superadmin password'), h('div', { style: { marginTop: '8px' } }, form));
  const card = h('div', { class: 'auth-card' },
    h('div', { class: 'auth-logo' }, 'Yulius'),
    h('h1', { class: 'auth-title' }, 'Sign in'),
    h('p', { class: 'text-2' }, 'Members sign in with GitHub. Project access mirrors your GitHub repository permissions.'),
    githubButton,
    errorBox,
    h('div', { class: 'divider' }, 'or'),
    details);
  if (error) details.open = false;
  setChildren(card.querySelector('.field-error'), error || '');
  return { el: h('div', { class: 'auth' }, card) };
}
