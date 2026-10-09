import { test } from '@e2e-dev/web';
import { expect } from 'e2e';
import { failOnPageErrors, mock } from './support.ts';

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
  await expect(screen.getByRole('heading', { level: 1 })).toHaveText('New task');
});
