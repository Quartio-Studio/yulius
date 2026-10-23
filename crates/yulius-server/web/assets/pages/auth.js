// Standalone pages shown before a session exists: setup wizard (J1, ACC-001..003) and
// sign-in (GitHub OAuth + superadmin password, ACC-005), with the superadmin recovery code
// (ACC-002/011): shown once at setup and after a recovery; "Forgot password?" uses it.

import { h, icon, field, setChildren, toast, skeleton } from '../dom.js';
import { BRAND } from '../brand.js';
import { keySetupCard } from '../instancekeys.js';
import { CONNECT_PATH } from '../claude.js';

/** The product's mark and name (brand.js) at the top of the sign-in and setup cards. */
function brandLockup() {
  return h('div', { class: 'auth-logo' }, h('span', { class: 'brand-logo', 'aria-hidden': 'true' }, icon('logo', { size: 34 })), BRAND.name);
}

// A new superadmin password needs 8 characters (the server's MIN_PASSWORD_LEN, NIST SP 800-63B);
// 12 are advised, not required (SPEC-242, ADR-0183).
const MIN_PASSWORD = 8;
const RECOMMENDED_PASSWORD = 12;
const PASSWORD_HINT = `At least ${MIN_PASSWORD} characters; ${RECOMMENDED_PASSWORD} or more recommended.`;

/** Length in code points, as the server counts (`chars()`), not UTF-16 units. */
const passwordLength = (value) => [...value].length;

/** The setup wizard's steps: superadmin, recovery code, Jev, Vidoc, first subscription (SPEC-283). */
const SETUP_STEPS = 5;

/**
 * Advice for a password long enough to be accepted but easy to guess, or '' (SPEC-242 PWM-004).
 * It never blocks: no composition rules and no list of common passwords (PWM-002).
 */
function passwordAdvice(value) {
  const length = passwordLength(value);
  if (length < MIN_PASSWORD) return '';
  if (length < RECOMMENDED_PASSWORD) return `Short passwords are easier to guess: at least ${RECOMMENDED_PASSWORD} characters recommended.`;
  if (new Set(value).size < 5) return 'Repeated characters are easy to guess: a phrase of several words is stronger.';
  return '';
}

/**
 * The recovery code, shown once with the warnings; Continue needs the acknowledgement.
 * Used by setup, recovery and the administration page.
 */
export function recoveryCodeCard(code, { title = 'Your recovery code', continueLabel = 'Continue', onContinue, step = null, next: after = null }) {
  const ack = h('input', { type: 'checkbox' });
  const next = h('button', { type: 'button', class: 'btn btn-primary', disabled: true, onClick: () => onContinue() }, continueLabel);
  ack.addEventListener('change', () => { next.disabled = !ack.checked; });
  const copy = h('button', { type: 'button', class: 'btn btn-sm', onClick: async () => {
    try { await navigator.clipboard.writeText(code); toast('Recovery code copied', { kind: 'ok' }); } catch { /* clipboard unavailable */ }
  } }, 'Copy');
  return h('div', { class: 'auth-card recovery-card' },
    brandLockup(),
    step ? h('div', { class: 'auth-step' }, step) : null,
    h('h1', { class: 'auth-title' }, title),
    h('div', { class: 'recovery-code', 'aria-label': `Recovery code ${code.split('').join(' ')}` }, code.slice(0, 3), h('span', { class: 'recovery-gap' }), code.slice(3)),
    h('div', { class: 'row justify-center' }, copy),
    h('div', { class: 'warning-box', role: 'note' }, icon('warn', { size: 18 }),
      h('div', { class: 'stack-sm' },
        h('strong', null, 'Keep this code perfectly safe.'),
        h('span', null, 'It is the only way to reset a forgotten superadmin password, and it is shown only now. Store it apart from the password — for example in a password manager and on a sealed paper copy.'),
        h('span', null, 'Three wrong codes disable password recovery permanently.'))),
    h('label', { class: 'check' }, ack, h('span', null, 'I have stored the recovery code safely.')),
    next,
    after ? h('p', { class: 'auth-next muted small' }, after) : null);
}

/**
 * The wizard's last step (SPEC-283 ONB-005): the first Claude subscription, which every agent runs
 * on. Add subscription ends setup on Subscriptions with its sign-in open; Skip for now on Home.
 */
function subscriptionStep({ step, onAdd, onSkip }) {
  const add = h('button', { type: 'button', class: 'btn btn-primary', onClick: onAdd }, 'Add subscription');
  setTimeout(() => add.focus(), 30);
  return h('div', { class: 'auth-card' },
    brandLockup(),
    h('div', { class: 'auth-step' }, step),
    h('h1', { class: 'auth-title' }, 'Add your first Claude subscription'),
    h('p', { class: 'text-2' }, `Agents run on a Claude subscription: every task, and the onboarding of each repository, uses one, so nothing starts until one is connected. Add subscription signs one in from a terminal on this server; ${BRAND.name} never sees its password.`),
    h('p', { class: 'muted xs' }, 'You can add it, or more of them, later in Settings → Subscriptions.'),
    h('div', { class: 'row' }, add, h('button', { type: 'button', class: 'btn btn-ghost', onClick: onSkip }, 'Skip for now')),
    h('p', { class: 'auth-next muted small' }, 'Next: Home, where you create your first sandbox. A playground from a project folder needs no GitHub.'));
}

/**
 * Says a too-short password and a mismatched repeat under their own fields (aria-invalid +
 * aria-describedby, WCAG 3.3.1) instead of in one line elsewhere on the card, and advice for a
 * weak but accepted password under it (described by it, not invalid; SPEC-242 PWM-004).
 */
function passwordChecks(password, confirm, prefix) {
  const passwordNote = h('span', { class: 'field-error', id: `${prefix}-password-error` });
  const adviceNote = h('span', { class: 'field-advice', id: `${prefix}-password-advice` });
  const confirmNote = h('span', { class: 'field-error', id: `${prefix}-confirm-error` });
  const check = () => {
    const short = Boolean(password.value) && passwordLength(password.value) < MIN_PASSWORD;
    const mismatch = Boolean(confirm.value) && confirm.value !== password.value;
    const advice = passwordAdvice(password.value);
    passwordNote.textContent = short ? `Use at least ${MIN_PASSWORD} characters.` : '';
    adviceNote.textContent = advice;
    // The error or the advice replaces the hint that says the same thing.
    const hint = password.closest('.field')?.querySelector('.field-hint');
    if (hint) hint.hidden = short || Boolean(advice);
    confirmNote.textContent = mismatch ? 'The passwords do not match.' : '';
    for (const [input, bad, note] of [[password, short, passwordNote], [confirm, mismatch, confirmNote]]) {
      if (bad) { input.setAttribute('aria-invalid', 'true'); input.setAttribute('aria-describedby', note.id); } else { input.removeAttribute('aria-invalid'); input.removeAttribute('aria-describedby'); }
    }
    if (advice) password.setAttribute('aria-describedby', adviceNote.id);
    return !short && !mismatch && passwordLength(password.value) >= MIN_PASSWORD && confirm.value === password.value;
  };
  return { passwordNote, adviceNote, confirmNote, check };
}

/**
 * Setup wizard: token, password + confirmation, non-recoverable warning (ACC-002), the recovery
 * code, the optional Jev key (ADM-001) and Vidoc key (SPEC-195 VDK-004), then the first Claude
 * subscription (SPEC-283): each step numbered, explained and saying what follows. `onDone(me,
 * path)` opens the app at `path`.
 */
export function mountSetup(ctx, { onDone }) {
  const token = h('input', { class: 'input mono', type: 'text', autocomplete: 'one-time-code', spellcheck: 'false', placeholder: 'printed on the VM by `yulius serve`', autofocus: true, 'aria-describedby': 'setup-token-why' });
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
    h('div', { class: 'auth-step' }, `Step 1 of ${SETUP_STEPS} · Superadmin`),
    h('h1', { class: 'auth-title' }, 'Set up this instance'),
    h('p', { class: 'text-2' }, 'Create the single superadmin: the account that owns this instance, connects Claude and GitHub, and admits your team.'),
    field('Setup token', token),
    // Said beside the field, not in its label, so the field's name stays "Setup token".
    h('p', { class: 'muted xs', id: 'setup-token-why' }, 'This one-time token protects your instance from unauthorized access: only someone who can read the server’s console, where `yulius serve` printed it, can claim the instance, so nobody who merely reaches this address can take it over.'),
    field('Superadmin password', password, PASSWORD_HINT, [checks.passwordNote, checks.adviceNote]),
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
    submit,
    h('p', { class: 'auth-next muted small' }, 'Next: your recovery code, two optional keys, then your first Claude subscription.'));

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
      // The session exists now: the optional key steps save through the superadmin's API.
      if (me && me.csrf_token) ctx.api.setCsrf(me.csrf_token);
      // The steps after this one: the recovery code (the server always issues one), the two
      // optional keys, the first subscription.
      const total = code ? SETUP_STEPS : SETUP_STEPS - 1;
      const at = (n) => `Step ${n - (code ? 0 : 1)} of ${total}`;
      const claudeStep = () => setChildren(root, subscriptionStep({
        step: `${at(5)} · Claude subscription`,
        onAdd: () => onDone(me, CONNECT_PATH),
        onSkip: () => onDone(me, '/'),
      }));
      const vidocStep = () => setChildren(root, keySetupCard(ctx, 'vidoc', { header: brandLockup(), step: `${at(4)} · Optional`, next: 'Next: your first Claude subscription.', onDone: claudeStep }));
      const jevStep = () => setChildren(root, keySetupCard(ctx, 'jev', { header: brandLockup(), step: `${at(3)} · Optional`, next: 'Next: security scans with Vidoc (optional).', onDone: vidocStep }));
      if (code) {
        setChildren(root, recoveryCodeCard(code, { step: `${at(2)} · Recovery code`, next: 'Next: two optional keys, then your first Claude subscription.', onContinue: jevStep }));
      } else {
        jevStep();
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
  const status = h('div', { class: 'text-2' }, skeleton('Checking the recovery code…', 1));
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
    field('New password', password, PASSWORD_HINT, [checks.passwordNote, checks.adviceNote]),
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
  }).catch(() => { status.textContent = 'Couldn’t check recovery.'; });
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

const GITHUB_LOGIN = /^[A-Za-z0-9](?:[A-Za-z0-9-]{0,38})$/;

/**
 * Why GitHub sign-in came back to the sign-in page (`/signin?error=<reason>&login=…`, ACC-004).
 * Only fixed sentences: a crafted link must not put its own words here (content spoofing), and
 * the refused login is named only when it is a well-formed GitHub login. `otherAccount` means
 * signing in again needs GitHub's account picker: the browser's GitHub account was refused.
 */
export function signInProblem(reason, login) {
  const account = typeof login === 'string' && GITHUB_LOGIN.test(login) ? login : null;
  switch (reason) {
    case 'not_admitted':
      return {
        otherAccount: true,
        text: account
          ? `GitHub signed you in as ${account}, and that account hasn’t been admitted to this instance. Continue with the admitted GitHub account, or ask the superadmin to admit ${account}.`
          : 'That GitHub account hasn’t been admitted to this instance. Continue with the admitted GitHub account, or ask the superadmin to admit it.',
      };
    case 'disabled':
      return { otherAccount: true, text: 'This account is disabled. Ask the superadmin to enable it, or continue with another GitHub account.' };
    case 'expired':
      return { otherAccount: false, text: 'That sign-in was already used or has expired. Sign in again.' };
    case 'denied':
      return { otherAccount: false, text: 'GitHub sign-in was cancelled.' };
    default:
      return { otherAccount: false, text: 'Sign-in did not complete. Try again.' };
  }
}

/** Sign-in: GitHub (all users) and a compact superadmin password form. */
export function mountSignIn(ctx, { next, onDone, problem }) {
  // Same rule as the server (#60): browsers read \\ as / and drop tabs and newlines.
  const nextPath = next && next.startsWith('/') && !next.startsWith('//') && !/[\\\u0000-\u001f\u007f]/.test(next) ? next : '/';
  const githubHref = `/api/auth/github/start?next=${encodeURIComponent(nextPath)}`;
  // GitHub otherwise signs in whichever account this browser is signed in to on github.com;
  // its account picker lets the same window continue with another account.
  const pickerHref = `${githubHref}&prompt=select_account`;
  const password = h('input', { class: 'input', type: 'password', autocomplete: 'current-password', 'aria-label': 'Superadmin password', 'aria-describedby': 'password-error' });
  // A wrong password is said under the password, not under the GitHub button (WCAG 3.3.1).
  const passwordError = h('div', { class: 'field-error', id: 'password-error', role: 'alert' });
  const submit = h('button', { type: 'submit', class: 'btn' }, 'Sign in');
  const errorBox = h('div', { class: 'field-error', role: 'alert' }, problem ? problem.text : '');
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
  const pickFirst = Boolean(problem && problem.otherAccount);
  const githubButton = h('a', { class: 'btn btn-primary btn-github', href: pickFirst ? pickerHref : githubHref }, pickFirst ? 'Sign in with another GitHub account' : 'Sign in with GitHub');
  const otherAccount = pickFirst ? null : h('a', { class: 'btn-link small auth-alt', href: pickerHref }, 'Use a different GitHub account');
  if (ctx.mock) {
    for (const link of [githubButton, otherAccount].filter(Boolean)) {
      link.addEventListener('click', async (event) => {
        event.preventDefault();
        const me = await ctx.api.post('/api/auth/mock-github', {});
        onDone(me, nextPath);
      });
    }
  }
  const forgot = h('button', { type: 'button', class: 'btn-link small', onClick: () => showRecovery() }, 'Forgot password?');
  const details = h('details', null, h('summary', { class: 'muted small' }, 'Superadmin password'), h('div', { class: 'stack-sm space-top' }, form, forgot));
  const card = h('div', { class: 'auth-card' },
    brandLockup(),
    h('h1', { class: 'auth-title' }, 'Sign in'),
    h('p', { class: 'text-2' }, 'Members sign in with GitHub. Sandbox access mirrors your GitHub repository permissions.'),
    githubButton,
    errorBox,
    otherAccount,
    h('div', { class: 'divider' }, 'or'),
    details);
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
