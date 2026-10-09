// Standalone pages shown before a session exists: setup wizard (J1, ACC-001..003) and
// sign-in (GitHub OAuth + superadmin password, ACC-005), with the superadmin recovery code
// (ACC-002/011): shown once at setup and after a recovery; "Forgot password?" uses it.

import { h, icon, field, setChildren, toast } from '../dom.js';
import { BRAND } from '../brand.js';

/** The product's mark and name (brand.js) at the top of the sign-in and setup cards. */
function brandLockup() {
  return h('div', { class: 'auth-logo' }, h('span', { class: 'brand-logo', 'aria-hidden': 'true' }, icon('logo', { size: 34 })), BRAND.name);
}

const MIN_PASSWORD = 12;

/**
 * The recovery code, shown once with the warnings; Continue needs the acknowledgement.
 * Used by setup, recovery and the administration page.
 */
export function recoveryCodeCard(code, { title = 'Your recovery code', continueLabel = 'Continue', onContinue }) {
  const ack = h('input', { type: 'checkbox' });
  const next = h('button', { type: 'button', class: 'btn btn-primary', disabled: true, onClick: () => onContinue() }, continueLabel);
  ack.addEventListener('change', () => { next.disabled = !ack.checked; });
  const copy = h('button', { type: 'button', class: 'btn btn-sm', onClick: async () => {
    try { await navigator.clipboard.writeText(code); toast('Recovery code copied', { kind: 'ok' }); } catch { /* clipboard unavailable */ }
  } }, 'Copy');
  return h('div', { class: 'auth-card recovery-card' },
    brandLockup(),
    h('h1', { class: 'auth-title' }, title),
    h('div', { class: 'recovery-code', 'aria-label': `Recovery code ${code.split('').join(' ')}` }, code.slice(0, 3), h('span', { class: 'recovery-gap' }), code.slice(3)),
    h('div', { class: 'row justify-center' }, copy),
    h('div', { class: 'warning-box', role: 'note' }, icon('warn', { size: 18 }),
      h('div', { class: 'stack-sm' },
        h('strong', null, 'Keep this code perfectly safe.'),
        h('span', null, 'It is the only way to reset a forgotten superadmin password, and it is shown only now. Store it apart from the password — for example in a password manager and on a sealed paper copy.'),
        h('span', null, 'Three wrong codes disable password recovery permanently.'))),
    h('label', { class: 'check' }, ack, h('span', null, 'I have stored the recovery code safely.')),
    next);
}

/**
 * Says a too-short password and a mismatched repeat under their own fields (aria-invalid +
 * aria-describedby, WCAG 3.3.1) instead of in one line elsewhere on the card.
 */
function passwordChecks(password, confirm, prefix) {
  const passwordNote = h('span', { class: 'field-error', id: `${prefix}-password-error` });
  const confirmNote = h('span', { class: 'field-error', id: `${prefix}-confirm-error` });
  const check = () => {
    const short = Boolean(password.value) && password.value.length < MIN_PASSWORD;
    const mismatch = Boolean(confirm.value) && confirm.value !== password.value;
    passwordNote.textContent = short ? `Use at least ${MIN_PASSWORD} characters.` : '';
    // The error replaces the hint that says the same thing.
    const hint = password.closest('.field')?.querySelector('.field-hint');
    if (hint) hint.hidden = short;
    confirmNote.textContent = mismatch ? 'The passwords do not match.' : '';
    for (const [input, bad, note] of [[password, short, passwordNote], [confirm, mismatch, confirmNote]]) {
      if (bad) { input.setAttribute('aria-invalid', 'true'); input.setAttribute('aria-describedby', note.id); } else { input.removeAttribute('aria-invalid'); input.removeAttribute('aria-describedby'); }
    }
    return !short && !mismatch && password.value.length >= MIN_PASSWORD && confirm.value === password.value;
  };
  return { passwordNote, confirmNote, check };
}

/** Setup wizard: token, password + confirmation, non-recoverable warning (ACC-002). */
export function mountSetup(ctx, { onDone }) {
  const token = h('input', { class: 'input mono', type: 'text', autocomplete: 'one-time-code', spellcheck: 'false', placeholder: 'printed on the VM by `yulius serve`', autofocus: true });
  const password = h('input', { class: 'input', type: 'password', autocomplete: 'new-password', minlength: MIN_PASSWORD });
  const confirm = h('input', { class: 'input', type: 'password', autocomplete: 'new-password' });
  const show = h('input', { type: 'checkbox' });
  const ackRecover = h('input', { type: 'checkbox', id: 'ack-recover' });
  const ackStored = h('input', { type: 'checkbox', id: 'ack-stored' });
  const checks = passwordChecks(password, confirm, 'setup');
  const problems = h('div', { class: 'field-error', role: 'alert' });
  const submit = h('button', { type: 'submit', class: 'btn btn-primary', disabled: true }, 'Create superadmin');

  const validate = () => {
    const passwordsOk = checks.check();
    problems.textContent = '';
    token.removeAttribute('aria-invalid');
    const ok = token.value.trim() && passwordsOk && ackRecover.checked && ackStored.checked;
    submit.disabled = !ok;
    return ok;
  };
  for (const input of [token, password, confirm]) input.addEventListener('input', validate);
  for (const box of [ackRecover, ackStored]) box.addEventListener('change', validate);
  show.addEventListener('change', () => { password.type = confirm.type = show.checked ? 'text' : 'password'; });

  const form = h('form', { class: 'auth-card', novalidate: true },
    h('input', { type: 'text', name: 'username', autocomplete: 'username', value: 'superadmin', hidden: true, readOnly: true, tabindex: '-1', 'aria-hidden': 'true' }),
    brandLockup(),
    h('h1', { class: 'auth-title' }, 'Set up this instance'),
    h('p', { class: 'text-2' }, 'Create the single superadmin. You need the one-time setup token printed on the server console.'),
    field('Setup token', token),
    field('Superadmin password', password, `At least ${MIN_PASSWORD} characters.`, checks.passwordNote),
    field('Repeat password', confirm, null, checks.confirmNote),
    h('label', { class: 'check small' }, show, h('span', null, 'Show passwords')),
    h('div', { class: 'warning-box', role: 'note' }, icon('warn', { size: 18 }),
      h('div', { class: 'stack-sm' },
        h('strong', null, 'Store this password safely.'),
        h('span', null, 'There is no email reset. The only way back in is the 6-digit recovery code shown on the next screen; three wrong codes disable recovery for good.'),
        h('span', null, 'Store the password in several safe places — for example a password manager and a sealed paper copy — before you continue.'))),
    h('label', { class: 'check', for: 'ack-recover' }, ackRecover, h('span', null, 'I understand the password can only be recovered with the recovery code.')),
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
      const code = me && me.recovery_code;
      if (me) delete me.recovery_code;
      if (code) {
        setChildren(root, recoveryCodeCard(code, { onContinue: () => onDone(me) }));
      } else {
        onDone(me);
      }
    } catch (error) {
      // Said on the card, above the button (a wrong token is marked on its field).
      problems.textContent = `Setup failed: ${error.message}`;
      if (error.status === 401 || error.status === 403) token.setAttribute('aria-invalid', 'true');
      submit.disabled = false;
    } finally {
      submit.classList.remove('busy');
    }
  });

  const root = h('main', { class: 'auth' }, form);
  return { el: root };
}

/** "Forgot password?": the recovery code sets a new superadmin password (ACC-011). */
function recoveryForm(ctx, { onBack, onRecovered }) {
  const code = h('input', { class: 'input mono recovery-input', type: 'text', inputmode: 'numeric', autocomplete: 'one-time-code', maxlength: 6, placeholder: '000000', 'aria-label': 'Recovery code' });
  const password = h('input', { class: 'input', type: 'password', autocomplete: 'new-password', minlength: MIN_PASSWORD });
  const confirm = h('input', { class: 'input', type: 'password', autocomplete: 'new-password' });
  const status = h('p', { class: 'text-2' }, 'Checking recovery…');
  const checks = passwordChecks(password, confirm, 'recovery');
  const problems = h('div', { class: 'field-error', role: 'alert' });
  const submit = h('button', { type: 'submit', class: 'btn btn-primary', disabled: true }, 'Reset password');
  const validate = () => {
    const passwordsOk = checks.check();
    const ok = /^\d{6}$/.test(code.value.trim()) && passwordsOk;
    problems.textContent = '';
    code.removeAttribute('aria-invalid');
    submit.disabled = !ok;
    return ok;
  };
  for (const input of [code, password, confirm]) input.addEventListener('input', validate);
  const form = h('form', { class: 'auth-card', novalidate: true },
    h('input', { type: 'text', name: 'username', autocomplete: 'username', value: 'superadmin', hidden: true, readOnly: true, tabindex: '-1', 'aria-hidden': 'true' }),
    brandLockup(),
    h('h1', { class: 'auth-title' }, 'Reset the superadmin password'),
    status,
    field('Recovery code', code, 'The 6 digits shown when the instance was set up.'),
    field('New password', password, `At least ${MIN_PASSWORD} characters.`, checks.passwordNote),
    field('Repeat new password', confirm, null, checks.confirmNote),
    problems,
    h('div', { class: 'row' }, submit, h('button', { type: 'button', class: 'btn btn-ghost', onClick: onBack }, 'Back to sign-in')));
  ctx.api.get('/api/auth/recovery', { allow401: true }).then((state) => {
    if (state && state.available) {
      status.textContent = `${state.attempts_left} attempt${state.attempts_left === 1 ? '' : 's'} left. After three wrong codes recovery is disabled permanently.`;
    } else {
      status.textContent = state && state.locked
        ? 'Password recovery is permanently disabled after too many wrong codes.'
        : 'No recovery code is set for this instance. A signed-in superadmin can create one under Administration.';
      for (const input of [code, password, confirm]) input.disabled = true;
    }
  }).catch(() => { status.textContent = 'Could not check recovery.'; });
  form.addEventListener('submit', async (event) => {
    event.preventDefault();
    if (!validate()) return;
    submit.disabled = true;
    submit.classList.add('busy');
    try {
      const result = await ctx.api.post('/api/auth/recover', { code: code.value.trim(), password: password.value, password_confirm: confirm.value }, { allow401: true });
      password.value = '';
      confirm.value = '';
      onRecovered(result.recovery_code);
    } catch (err) {
      problems.textContent = err.message || 'Recovery failed.';
      code.setAttribute('aria-invalid', 'true');
      code.select();
      submit.disabled = false;
    } finally {
      submit.classList.remove('busy');
    }
  });
  return form;
}

/** Sign-in: GitHub (all users) and a compact superadmin password form. */
export function mountSignIn(ctx, { next, onDone, error }) {
  const nextPath = next && next.startsWith('/') && !next.startsWith('//') ? next : '/';
  const githubHref = `/api/auth/github/start?next=${encodeURIComponent(nextPath)}`;
  const password = h('input', { class: 'input', type: 'password', autocomplete: 'current-password', 'aria-label': 'Superadmin password', 'aria-describedby': 'password-error' });
  // A wrong password is said under the password, not under the GitHub button (WCAG 3.3.1).
  const passwordError = h('div', { class: 'field-error', id: 'password-error', role: 'alert' });
  const submit = h('button', { type: 'submit', class: 'btn' }, 'Sign in');
  const errorBox = h('div', { class: 'field-error', role: 'alert' }, error || '');
  const username = h('input', { type: 'text', name: 'username', autocomplete: 'username', value: 'superadmin', hidden: true, readOnly: true, tabindex: '-1', 'aria-hidden': 'true' });
  const form = h('form', { class: 'stack-sm' }, username, h('div', { class: 'row' }, password, submit), passwordError);
  form.addEventListener('submit', async (event) => {
    event.preventDefault();
    if (!password.value) return;
    submit.disabled = true;
    submit.classList.add('busy');
    errorBox.textContent = '';
    passwordError.textContent = '';
    password.removeAttribute('aria-invalid');
    try {
      const me = await ctx.api.post('/api/auth/password', { password: password.value }, { allow401: true });
      password.value = '';
      onDone(me, nextPath);
    } catch (err) {
      passwordError.textContent = err.status === 401 ? 'Wrong password.' : err.message;
      password.setAttribute('aria-invalid', 'true');
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
  const forgot = h('button', { type: 'button', class: 'btn-link small', onClick: () => showRecovery() }, 'Forgot password?');
  const details = h('details', null, h('summary', { class: 'muted small' }, 'Superadmin password'), h('div', { class: 'stack-sm space-top' }, form, forgot));
  const card = h('div', { class: 'auth-card' },
    brandLockup(),
    h('h1', { class: 'auth-title' }, 'Sign in'),
    h('p', { class: 'text-2' }, 'Members sign in with GitHub. Sandbox access mirrors your GitHub repository permissions.'),
    githubButton,
    errorBox,
    h('div', { class: 'divider' }, 'or'),
    details);
  if (error) details.open = false;
  setChildren(card.querySelector('.field-error'), error || '');
  const root = h('main', { class: 'auth' }, card);
  function showRecovery() {
    setChildren(root, recoveryForm(ctx, {
      onBack: () => setChildren(root, card),
      onRecovered: (fresh) => setChildren(root, recoveryCodeCard(fresh, {
        title: 'Password changed — your new recovery code',
        continueLabel: 'Back to sign-in',
        onContinue: () => {
          setChildren(root, card);
          details.open = true;
          // Success is not an error: say it in the neutral notice.
          errorBox.className = 'field-note';
          errorBox.textContent = 'Password changed. Sign in with the new password.';
          password.focus();
        },
      })),
    }));
  }
  return { el: root };
}
