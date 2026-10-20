import { test, type Browser } from '@e2e-dev/web';
import { expect } from 'e2e';
import { failOnPageErrors, mock } from './support.ts';

failOnPageErrors();

// SPEC-242: a new superadmin password (setup, recovery) needs 8 characters; shorter than 12, or
// made of a few distinct characters, gets advice that does not block. The mock's setup token and
// recovery code (crates/yulius-server/web/README.md); not real credentials.
const SETUP_TOKEN = 'YUL-SETUP-1234';
const RECOVERY_CODE = '123456';

/** Whether a field is marked invalid (the runner refuses to read a password field itself). */
const invalid = (browser: Browser, label: string) => browser.evaluate((name: string) => {
  const field = [...document.querySelectorAll('label.field')].find((el) => el.querySelector('.field-label')?.textContent === name);
  return field?.querySelector('input')?.getAttribute('aria-invalid') ?? null;
}, label);

test('setup refuses 7 characters, takes 8 with a 12-character recommendation and completes', async ({ app, browser, screen }) => {
  await app.open(mock('/setup', '&setup=1'));
  await expect(screen.getByRole('heading', { level: 1 })).toHaveText('Set up this instance');
  await expect(browser.locator('label.field:has-text("Superadmin password") .field-hint')).toHaveText('At least 8 characters; 12 or more recommended.');

  await browser.locator('label.field:has(> .field-label:text-is("Setup token")) input').fill(SETUP_TOKEN);
  await screen.getByLabel('I understand the password can only be recovered with the recovery code.').click();
  await screen.getByLabel('I have stored the password in several safe places.').click();
  const password = browser.locator('label.field:has(> .field-label:text-is("Superadmin password")) input');
  const repeat = browser.locator('label.field:has(> .field-label:text-is("Repeat password")) input');

  await password.fill('zaq1@WS');
  await repeat.fill('zaq1@WS');
  await expect(browser.locator('#setup-password-error')).toHaveText('Use at least 8 characters.');
  await expect.poll(() => invalid(browser, 'Superadmin password')).toBe('true');
  await expect(screen.getByRole('button', 'Create superadmin')).toBeDisabled();

  // The password from the user feedback: 8 characters and a keyboard pattern, accepted.
  await password.fill('zaq1@WSX');
  await repeat.fill('zaq1@WSX');
  await expect(browser.locator('#setup-password-error')).toHaveText('');
  await expect(browser.locator('#setup-password-advice')).toHaveText('Short passwords are easier to guess: at least 12 characters recommended.');
  await expect.poll(() => invalid(browser, 'Superadmin password')).toBe(null);
  await expect(screen.getByRole('button', 'Create superadmin')).toBeEnabled();

  await screen.getByRole('button', 'Create superadmin').click();
  // Set up: the mock issues no recovery code, so the optional key steps follow at once.
  await expect(screen.getByRole('heading', { level: 1 })).toHaveText('Task intake with Jev');
});

test('the advice names repeated characters and leaves a long varied password alone', async ({ app, browser, screen }) => {
  await app.open(mock('/setup', '&setup=1'));
  const password = browser.locator('label.field:has(> .field-label:text-is("Superadmin password")) input');
  const hint = browser.locator('label.field:has-text("Superadmin password") .field-hint');

  await password.fill('abababababab');
  await expect(browser.locator('#setup-password-advice')).toHaveText('Repeated characters are easy to guess: a phrase of several words is stronger.');
  await expect(hint).toBeHidden();
  await expect.poll(() => invalid(browser, 'Superadmin password')).toBe(null);

  await password.fill('correct horse battery');
  await expect(browser.locator('#setup-password-advice')).toHaveText('');
  await expect(hint).toBeVisible();

  // Code points, as the server counts: 7 emoji are 14 UTF-16 units but too short.
  await password.fill('🔑'.repeat(7));
  await expect(browser.locator('#setup-password-error')).toHaveText('Use at least 8 characters.');
});

test('recovery refuses 7 characters and resets the password with 8', async ({ app, browser, screen }) => {
  await app.open(mock('/', '&signedout=1'));
  await screen.getByText('Superadmin password').click();
  await screen.getByRole('button', 'Forgot password?').click();
  await expect(screen.getByRole('heading', { level: 1 })).toHaveText('Reset the superadmin password');
  await expect(browser.locator('label.field:has-text("Repeat new password")')).toBeVisible();
  await expect(screen.getByText('attempts left', { exact: false })).toBeVisible();

  await screen.getByLabel('Recovery code').fill(RECOVERY_CODE);
  const password = browser.locator('label.field:has(> .field-label:text-is("New password")) input');
  const repeat = browser.locator('label.field:has(> .field-label:text-is("Repeat new password")) input');
  await password.fill('zaq1@WS');
  await repeat.fill('zaq1@WS');
  await expect(browser.locator('#recovery-password-error')).toHaveText('Use at least 8 characters.');
  await expect(screen.getByRole('button', 'Reset password')).toBeDisabled();

  await password.fill('zaq1@WSX');
  await repeat.fill('zaq1@WSX');
  await expect(browser.locator('#recovery-password-advice')).toHaveText('Short passwords are easier to guess: at least 12 characters recommended.');
  await screen.getByRole('button', 'Reset password').click();
  await expect(screen.getByRole('heading', { level: 1 })).toHaveText('Password changed — your new recovery code');
});
