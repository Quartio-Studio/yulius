import { test } from '@e2e-dev/web';
import { expect } from 'e2e';
import { axeViolations, failOnPageErrors, mock } from './support.ts';

failOnPageErrors();

// The mock's superadmin password (crates/yulius-server/web/README.md); not a real credential.
const MOCK_PASSWORD = 'correct horse battery';

test('a wrong password is refused and the right one signs in', async ({ app, browser, screen }) => {
  await app.open(mock('/', '&signedout=1'));
  await expect(browser).toHaveURL(/#\/signin/);
  await expect(screen.getByRole('heading', { level: 1 })).toHaveText('Sign in');

  // The password form sits in a disclosure below "Sign in with GitHub".
  await screen.getByText('Superadmin password').click();
  const password = screen.getByLabel('Superadmin password');
  await password.fill('not the password');
  await screen.getByRole('button', 'Sign in').click();
  // The password field's error (its aria-describedby; the runner refuses to read a password field).
  await expect(browser.locator('#password-error')).toHaveText(/\S/);
  await expect(browser).toHaveURL(/#\/signin/);

  await password.fill(MOCK_PASSWORD);
  await screen.getByRole('button', 'Sign in').click();

  await expect(browser).not.toHaveURL(/#\/signin/);
  // `/` is the home screen with every sandbox (UI-045).
  await expect(screen.getByRole('heading', { level: 1 })).toHaveText('Sandboxes');
});

// SPEC-422 (#1069): before the GitHub App is connected the page offers no GitHub sign-in that
// can only fail, says why, and opens the superadmin password form.
const NO_GITHUB = /GitHub sign-in isn’t set up on this instance yet/;

test('without a GitHub App the page offers the password, not GitHub', async ({ app, browser, screen }) => {
  await app.open(mock('/', '&signedout=1&github=0'));
  await expect(browser).toHaveURL(/#\/signin/);
  await expect(screen.getByText(NO_GITHUB)).toBeVisible();
  await expect(browser.locator('a.btn-github')).toHaveCount(0);
  await expect(browser.locator('a.auth-alt')).toHaveCount(0);
  await expect(browser.locator('details[open]')).toHaveCount(1);
  expect(await axeViolations(browser)).toEqual([]);
  const password = screen.getByLabel('Superadmin password');
  await expect(password).toBeFocused();
  await password.fill(MOCK_PASSWORD);
  await screen.getByRole('button', 'Sign in').click();
  await expect(screen.getByRole('heading', { level: 1 })).toHaveText('Sandboxes');
});

test('a GitHub sign-in started without an App says why on the sign-in page', async ({ app, browser }) => {
  // Where the server sends /api/auth/github/start without an App (an old link or bookmark):
  // the sentence once, not again as an error under it.
  await app.open(mock('/signin?error=github_unconfigured&next=/', '&signedout=1&github=0'));
  await expect(browser.locator('.auth-card >> text=/isn’t set up on this instance yet/')).toHaveCount(1);
  await expect(browser.locator('a.btn-github')).toHaveCount(0);
  await expect(browser).toHaveURL(/#\/signin\?next=/);
});

test('with a GitHub App the page still offers GitHub sign-in', async ({ app, browser, screen }) => {
  await app.open(mock('/', '&signedout=1'));
  await expect(browser.locator('a.btn-github')).toHaveText('Sign in with GitHub');
  await expect(screen.getByText(NO_GITHUB)).toHaveCount(0);
  await expect(browser.locator('details[open]')).toHaveCount(0);
});
