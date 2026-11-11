import { test, type Browser } from '@e2e-dev/web';
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
  await expect(browser.locator('a.btn-github:visible')).toHaveCount(0);
  await expect(browser.locator('a.auth-alt:visible')).toHaveCount(0);
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
  await expect(browser.locator('a.btn-github:visible')).toHaveCount(0);
  await expect(browser).toHaveURL(/#\/signin\?next=/);
});

test('with a GitHub App the page still offers GitHub sign-in', async ({ app, browser, screen }) => {
  await app.open(mock('/', '&signedout=1'));
  await githubOffered(browser);
  await expect(screen.getByText(NO_GITHUB)).toHaveCount(0);
  await expect(browser.locator('details[open]')).toHaveCount(0);
});

// Review round 1: GitHub connected after the page loaded (the superadmin's first run, or another
// tab) is offered at the next sign-in without a reload.
type Mocked = Window & { __yuliusMock: { finishGithubApp(): void; holdPath: string | null; held: Promise<void> | null; release(): void; heldWaiting?: number } };

/** Holds the page's next reads of the setup status until `releaseStatus` (review round 2). */
const holdStatus = (browser: Browser) => browser.evaluate(() => {
  const m = (window as unknown as Mocked).__yuliusMock;
  m.holdPath = '/api/setup/status';
  m.held = new Promise<void>((resolve) => { m.release = resolve; });
});
const statusWaiting = (browser: Browser) => browser.evaluate(() => (window as unknown as Mocked).__yuliusMock.heldWaiting ?? 0);
const releaseStatus = (browser: Browser) => browser.evaluate(() => { (window as unknown as Mocked).__yuliusMock.release(); });
const passwordValue = (browser: Browser) => browser.evaluate(() => (document.querySelector('input[type="password"]') as HTMLInputElement | null)?.value ?? null);
/** "Sign in with GitHub", the other-account link and the divider are shown (not merely present). */
const githubOffered = async (browser: Browser) => {
  await expect(browser.locator('a.btn-github')).toBeVisible();
  await expect(browser.locator('a.btn-github')).toHaveText('Sign in with GitHub');
  await expect(browser.locator('a.auth-alt')).toBeVisible();
  await expect(browser.locator('.auth-card .divider')).toBeVisible();
};
const signOut = async (browser: Browser, screen: { getByRole: Function }) => {
  await screen.getByRole('button', /account menu$/).click();
  await screen.getByRole('menuitem', 'Sign out').click();
  await expect(browser).toHaveURL(/#\/signin/);
};

test('GitHub connected after the page loaded is offered at the next sign-in', async ({ app, browser, screen }) => {
  await app.open(mock('/', '&signedout=1&github=0'));
  await expect(screen.getByText(NO_GITHUB)).toBeVisible();
  await screen.getByLabel('Superadmin password').fill(MOCK_PASSWORD);
  await screen.getByRole('button', 'Sign in').click();
  await expect(screen.getByRole('heading', { level: 1 })).toHaveText('Sandboxes');
  await browser.evaluate(() => { (window as unknown as Mocked).__yuliusMock.finishGithubApp(); });
  await screen.getByRole('button', /account menu$/).click();
  await screen.getByRole('menuitem', 'Sign out').click();
  await expect(browser).toHaveURL(/#\/signin/);
  await githubOffered(browser);
  await expect(screen.getByText(NO_GITHUB)).toHaveCount(0);
});

test('a stale "not set up" reason is not shown once GitHub is connected', async ({ app, browser }) => {
  await app.open(mock('/signin?error=github_unconfigured&next=/', '&signedout=1'));
  await githubOffered(browser);
  await expect(browser.locator('.auth-card >> text=/isn’t set up on this instance yet/')).toHaveCount(0);
});

test('a password typed while the page learns GitHub was connected stays', async ({ app, browser, screen }) => {
  // Signed in before GitHub was connected elsewhere: this session still says it isn't.
  await app.open(mock('/', '&github=0'));
  await expect(screen.getByRole('heading', { level: 1 })).toHaveText('Sandboxes');
  await browser.evaluate(() => { (window as unknown as Mocked).__yuliusMock.finishGithubApp(); });
  await holdStatus(browser);
  await signOut(browser, screen);
  await expect(screen.getByText(NO_GITHUB)).toBeVisible();
  await expect.poll(() => statusWaiting(browser)).toBe(1);
  const password = screen.getByLabel('Superadmin password');
  await expect(password).toBeFocused();
  await password.fill(MOCK_PASSWORD);
  await releaseStatus(browser);
  await githubOffered(browser);
  await expect(screen.getByText(NO_GITHUB)).toHaveCount(0);
  // The same form: the typed password, the open disclosure and the keyboard stay.
  expect(await passwordValue(browser)).toBe(MOCK_PASSWORD);
  await expect(browser.locator('details[open]')).toHaveCount(1);
  await expect(password).toBeFocused();
  await screen.getByRole('button', 'Sign in').click();
  await expect(screen.getByRole('heading', { level: 1 })).toHaveText('Sandboxes');
});

test('GitHub connected in this session is offered at sign-out before the status answers', async ({ app, browser, screen }) => {
  await app.open(mock('/admin/github', '&github=0'));
  await expect(screen.getByText(/You confirm the app on GitHub/)).toBeVisible();
  await browser.evaluate(() => { window.open = (() => ({ opener: null, closed: false, document: document.implementation.createHTMLDocument(''), close() {} })) as unknown as typeof window.open; });
  await browser.locator('label.field:has(> .field-label:text-is("Owner (organization or user)")) input').fill('Quartio-Studio');
  await screen.getByRole('button', 'Create GitHub App on GitHub').click();
  await expect(screen.getByRole('heading', { name: 'Finish on GitHub in the new tab' })).toBeVisible();
  // The App now exists (created in the other tab) and the person comes back to this one.
  await browser.evaluate(() => {
    (window as unknown as Mocked).__yuliusMock.finishGithubApp();
    document.dispatchEvent(new Event('visibilitychange'));
  });
  await expect(screen.getByText('Connected', { exact: true })).toBeVisible();
  // The setup status cannot answer: what the session knew draws the page.
  await holdStatus(browser);
  await signOut(browser, screen);
  await expect.poll(() => statusWaiting(browser)).toBe(1);
  await githubOffered(browser);
  await expect(screen.getByText(NO_GITHUB)).toHaveCount(0);
  await releaseStatus(browser);
});

test('a late status answer for an earlier sign-in page leaves the current one right', async ({ app, browser, screen }) => {
  await app.open(mock('/', '&github=0'));
  await expect(screen.getByRole('heading', { level: 1 })).toHaveText('Sandboxes');
  await browser.evaluate(() => { (window as unknown as Mocked).__yuliusMock.finishGithubApp(); });
  await holdStatus(browser);
  // Page A: drawn from the session (no GitHub), its status read held.
  await signOut(browser, screen);
  await expect.poll(() => statusWaiting(browser)).toBe(1);
  await screen.getByLabel('Superadmin password').fill(MOCK_PASSWORD);
  await screen.getByRole('button', 'Sign in').click();
  await expect(screen.getByRole('heading', { level: 1 })).toHaveText('Sandboxes');
  // GitHub goes away again; this session still says it is connected.
  await browser.evaluate(() => { (window as unknown as { __yuliusMock: { me: { instance: { github_configured: boolean } } } }).__yuliusMock.me.instance.github_configured = false; });
  // Page B: drawn from the session (GitHub), its status read held too.
  await signOut(browser, screen);
  await expect.poll(() => statusWaiting(browser)).toBe(2);
  await githubOffered(browser);
  // A's answer comes first and is for a page no longer shown; B's then says GitHub is gone.
  await releaseStatus(browser);
  await expect(screen.getByText(NO_GITHUB)).toBeVisible();
  await expect(browser.locator('a.btn-github')).toBeHidden();
});

// SPEC-466 MVC-012: after a move to another URL GitHub refuses every sign-in from the new address
// until the GitHub App holds its callback URL. The page says so (`callback=missing`: the server
// knows GitHub's answer), opens the superadmin password and keeps GitHub sign-in: GitHub decides,
// and pressing it makes the server ask GitHub again.
const MOVED_ADDRESS = /GitHub says it doesn’t know this address/;

test('when GitHub does not know this address the page says so and opens the password', async ({ app, browser, screen }) => {
  await app.open(mock('/', '&signedout=1&callback=missing'));
  await expect(browser).toHaveURL(/#\/signin/);
  await expect(screen.getByText(MOVED_ADDRESS)).toBeVisible();
  await expect(screen.getByText(/until the superadmin adds it to the GitHub App \(Settings → Instance says how\)\. The superadmin’s password below works meanwhile\./)).toBeVisible();
  await githubOffered(browser);
  await expect(browser.locator('details[open]')).toHaveCount(1);
  expect(await axeViolations(browser)).toEqual([]);
  const password = screen.getByLabel('Superadmin password');
  await password.fill(MOCK_PASSWORD);
  await screen.getByRole('button', 'Sign in').click();
  await expect(screen.getByRole('heading', { level: 1 })).toHaveText('Sandboxes');
});

test('the page reads the status once more by itself, and the sentence goes when GitHub accepts the address', async ({ app, browser, screen }) => {
  // `callback=fixed`: the first status says that GitHub refuses the address, and, as on the
  // server, reading it made the instance ask GitHub again, which accepts the address by now.
  await app.open(mock('/', '&signedout=1&callback=fixed'));
  await expect(browser).toHaveURL(/#\/signin/);
  const reads = () => browser.evaluate(() => (window as unknown as { __yuliusMock: { asked?: Record<string, number> } }).__yuliusMock.asked?.['/api/setup/status'] ?? 0);
  await expect.poll(reads, { timeout: 15000 }).toBe(2);
  await expect(screen.getByText(MOVED_ADDRESS)).toBeHidden();
  await githubOffered(browser);
});

test('the page learns in place that GitHub accepts the address now', async ({ app, browser, screen }) => {
  await app.open(mock('/', '&signedout=1&callback=missing'));
  await expect(screen.getByText(MOVED_ADDRESS)).toBeVisible();
  // Signing in and out again reads the status anew, as every sign-in page does.
  await browser.evaluate(() => { (window as unknown as { __yuliusMock: { address: { state: string } } }).__yuliusMock.address.state = 'registered'; });
  const password = screen.getByLabel('Superadmin password');
  await password.fill(MOCK_PASSWORD);
  await screen.getByRole('button', 'Sign in').click();
  await expect(screen.getByRole('heading', { level: 1 })).toHaveText('Sandboxes');
  await signOut(browser, screen);
  await expect(browser).toHaveURL(/#\/signin/);
  await githubOffered(browser);
  await expect(screen.getByText(MOVED_ADDRESS)).toBeHidden();
});
