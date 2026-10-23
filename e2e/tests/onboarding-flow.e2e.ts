import { test, type Browser } from '@e2e-dev/web';
import { expect } from 'e2e';
import { axeViolations, failOnPageErrors, mock } from './support.ts';

failOnPageErrors();

// SPEC-283 (task #269), from a person setting up an instance: "GitHub connection must open in a
// new window/tab (it currently opens in the same tab, replacing Yulius, and users can't go back)",
// and after each step it must be clear what comes next.

type Opened = { url: string; target: string; openerCut: boolean; text: string; closed: boolean };
type Stubbed = Window & { __opened: { url: string; target: string; tab: { opener: unknown; closed: boolean; document: Document } }[] };
type Mocked = Window & { __yuliusMock: { finishGithubApp(): void } };

/** Replaces `window.open` with a stub that records the tab (`blocked`: answers null, as a pop-up blocker does). */
const stubWindowOpen = (browser: Browser, blocked = false) => browser.evaluate((isBlocked: boolean) => {
  const w = window as unknown as Stubbed;
  w.__opened = [];
  window.open = ((url: string, target: string) => {
    if (isBlocked) return null;
    const tab = { opener: window as unknown, closed: false, document: document.implementation.createHTMLDocument(''), close() { this.closed = true; } };
    w.__opened.push({ url, target, tab });
    return tab;
  }) as typeof window.open;
}, blocked);

const opened = (browser: Browser): Promise<Opened[]> => browser.evaluate(() => (window as unknown as Stubbed).__opened.map((o) => ({
  url: o.url, target: o.target, openerCut: o.tab.opener === null, text: o.tab.document.body.textContent ?? '', closed: o.tab.closed,
})));

/** The App now exists (created in the other tab) and the person comes back to this one. */
const finishOnGithub = (browser: Browser) => browser.evaluate(() => {
  (window as unknown as Mocked).__yuliusMock.finishGithubApp();
  document.dispatchEvent(new Event('visibilitychange'));
});

const owner = (browser: Browser) => browser.locator('label.field:has(> .field-label:text-is("Owner (organization or user)")) input');

test('Create GitHub App opens GitHub in a new tab and this tab waits, then says what comes next', async ({ app, browser, screen }) => {
  await app.open(mock('/admin/github', '&github=0'));
  await expect(screen.getByText(/You confirm the app on GitHub/)).toBeVisible();
  await stubWindowOpen(browser);
  await owner(browser).fill('Quartio-Studio');
  await screen.getByRole('button', 'Create GitHub App on GitHub').click();

  await expect(screen.getByRole('heading', { name: 'Finish on GitHub in the new tab' })).toBeVisible();
  await expect(browser).toHaveURL(/#\/admin\/github$/);
  const tabs = await opened(browser);
  expect(tabs.length).toBe(1);
  expect(tabs[0].target).toBe('_blank');
  // GitHub's pages get no handle on this tab, and the manifest went to the new tab.
  expect(tabs[0].openerCut).toBe(true);
  expect(tabs[0].text).toContain('https://github.com/organizations/Quartio-Studio/settings/apps/new');
  expect(await axeViolations(browser)).toEqual([]);

  await finishOnGithub(browser);
  await expect(screen.getByText('Connected', { exact: true })).toBeVisible();
  await expect(screen.getByText('GitHub connected', { exact: false })).toBeVisible();
  const next = screen.getByRole('region', 'Next steps');
  await expect(next.getByRole('link', { name: 'Install on repositories' })).toHaveAttribute('target', '_blank');
  await expect(next.getByRole('button', { name: 'Import repository' })).toBeVisible();
  await expect(next.getByRole('button', { name: 'Create from template' })).toBeVisible();
  await expect(next.getByRole('link', { name: 'Admit users' })).toBeVisible();
  expect(await axeViolations(browser)).toEqual([]);
  await next.getByRole('button', { name: 'Import repository' }).click();
  await expect(screen.getByRole('dialog')).toBeVisible();

  // Home knows too, without a reload.
  await browser.evaluate(() => { location.hash = '#/'; });
  await expect(screen.getByRole('link', { name: 'Connect GitHub' })).toHaveCount(0);
});

test('a blocked new tab says to allow pop-ups and posts nothing', async ({ app, browser, screen }) => {
  await app.open(mock('/admin/github', '&github=0'));
  await expect(screen.getByText(/You confirm the app on GitHub/)).toBeVisible();
  await stubWindowOpen(browser, true);
  await owner(browser).fill('Quartio-Studio');
  await screen.getByRole('button', 'Create GitHub App on GitHub').click();
  await expect(screen.getByText(/blocked the new tab/)).toBeVisible();
  await expect(screen.getByRole('heading', { name: 'Finish on GitHub in the new tab' })).toHaveCount(0);
  await expect(screen.getByRole('button', 'Create GitHub App on GitHub')).toBeEnabled();
});

test('the waiting card starts again', async ({ app, browser, screen }) => {
  await app.open(mock('/admin/github', '&github=0'));
  await expect(screen.getByText(/You confirm the app on GitHub/)).toBeVisible();
  await stubWindowOpen(browser);
  await owner(browser).fill('Quartio-Studio');
  await screen.getByRole('button', 'Create GitHub App on GitHub').click();
  await screen.getByRole('button', 'Start again').click();
  await expect(screen.getByRole('button', 'Create GitHub App on GitHub')).toBeVisible();
});

test('the connected card says what comes next, Connect Claude first without a subscription', async ({ app, screen }) => {
  await app.open(mock('/admin/github', '&claude=0'));
  const next = screen.getByRole('region', 'Next steps');
  await expect(next.getByRole('link', { name: 'Connect Claude' })).toBeVisible();
  await expect(next.getByRole('button', { name: 'Import repository' })).toHaveCount(0);
});

/** Fills the first setup card and creates the superadmin (the mock's setup token, not a real one). */
async function createSuperadmin(browser: Browser, screen: Parameters<Parameters<typeof test>[1]>[0]['screen']) {
  await screen.getByLabel('Setup token').fill('YUL-SETUP-1234');
  await browser.locator('input[autocomplete="new-password"]').nth(0).fill('a long enough password');
  await browser.locator('input[autocomplete="new-password"]').nth(1).fill('a long enough password');
  await browser.locator('#ack-recover').check();
  await browser.locator('#ack-stored').check();
  await screen.getByRole('button', 'Create superadmin').click();
}

// The step label is set in capitals (CSS), which the runner reads.
const stepLabel = (browser: Browser) => browser.locator('.auth-card .auth-step');
const nextLine = (browser: Browser) => browser.locator('.auth-card .auth-next');

test('setup numbers its steps, says what each does and what comes next, and ends with the first subscription', async ({ app, browser, screen }) => {
  await app.open(mock('/', '&setup=1&signedout=1&recovery=1'));
  await expect(stepLabel(browser)).toHaveText(/^Step 1 of 5 · Superadmin$/i);
  await expect(browser.locator('#setup-token-why')).toHaveText(/protects your instance from unauthorized access/);
  await expect(nextLine(browser)).toHaveText('Next: your recovery code, two optional keys, then your first Claude subscription.');
  expect(await axeViolations(browser)).toEqual([]);
  await createSuperadmin(browser, screen);

  await expect(screen.getByRole('heading', { level: 1 })).toHaveText('Your recovery code');
  await expect(stepLabel(browser)).toHaveText(/^Step 2 of 5 · Recovery code$/i);
  await expect(nextLine(browser)).toHaveText('Next: two optional keys, then your first Claude subscription.');
  await browser.locator('.recovery-card input[type="checkbox"]').check();
  await screen.getByRole('button', 'Continue').click();

  await expect(screen.getByRole('heading', { level: 1 })).toHaveText('Task intake with Jev');
  await expect(stepLabel(browser)).toHaveText(/^Step 3 of 5 · Optional$/i);
  await expect(nextLine(browser)).toHaveText('Next: security scans with Vidoc (optional).');
  await screen.getByRole('button', 'Skip for now').click();

  await expect(screen.getByRole('heading', { level: 1 })).toHaveText('Security scans with Vidoc');
  await expect(stepLabel(browser)).toHaveText(/^Step 4 of 5 · Optional$/i);
  await expect(nextLine(browser)).toHaveText('Next: your first Claude subscription.');
  await screen.getByRole('button', 'Skip for now').click();

  await expect(screen.getByRole('heading', { level: 1 })).toHaveText('Add your first Claude subscription');
  await expect(stepLabel(browser)).toHaveText(/^Step 5 of 5 · Claude subscription$/i);
  await expect(screen.getByText(/Agents run on a Claude subscription/)).toBeVisible();
  expect(await axeViolations(browser)).toEqual([]);
  await screen.getByRole('button', 'Add subscription').click();
  await expect(screen.getByRole('dialog', 'Add a Claude subscription')).toBeVisible();
  await expect(browser).toHaveURL(/#\/admin\/accounts$/);
});

test('skipping the first subscription ends setup on Home', async ({ app, browser, screen }) => {
  await app.open(mock('/', '&setup=1&signedout=1'));
  await createSuperadmin(browser, screen);
  // No recovery code from this mock: four steps.
  await expect(stepLabel(browser)).toHaveText(/^Step 2 of 4 · Optional$/i);
  await screen.getByRole('button', 'Skip for now').click();
  await expect(stepLabel(browser)).toHaveText(/^Step 3 of 4 · Optional$/i);
  await screen.getByRole('button', 'Skip for now').click();
  await expect(stepLabel(browser)).toHaveText(/^Step 4 of 4 · Claude subscription$/i);
  await screen.getByRole('button', 'Skip for now').click();
  await expect(screen.getByRole('heading', { level: 1 })).toHaveText('Sandboxes');
});

/** Home's getting-started steps as [title, state]. */
const gettingStarted = (browser: Browser) => browser.evaluate(() => [...document.querySelectorAll('section[aria-label="Getting started"] li')]
  .map((li) => [li.querySelector('.gs-title')?.textContent ?? '', li.querySelector('.gs-state')?.textContent ?? '']));

test('Home lists the getting-started steps with their state, the first not done marked Next', async ({ app, browser, screen }) => {
  await app.open(mock('/', '&sandboxes=0&claude=0&github=0'));
  await expect(screen.getByRole('heading', { level: 1 })).toHaveText('Try Yulius on your own code');
  await expect.poll(() => gettingStarted(browser)).toEqual([
    ['Connect Claude', 'Next'], ['Create your first sandbox', 'To do'], ['Connect GitHub', 'Optional'],
  ]);
  await expect(browser.locator('section[aria-label="Getting started"] li:first-child .gs-text')).toHaveText(/Agents run on your Claude subscription/);
  expect(await axeViolations(browser)).toEqual([]);

  await app.open(mock('/', '&sandboxes=0'));
  await expect.poll(() => gettingStarted(browser)).toEqual([
    ['Connect Claude', 'Done'], ['Create your first sandbox', 'Next'], ['Connect GitHub', 'Done'],
  ]);
  // The next step's actions are right below: a playground, or a repository.
  await expect(screen.getByRole('button', 'Import repository')).toBeVisible();
  await expect(screen.getByRole('button', 'Create from template')).toBeVisible();
  expect(await axeViolations(browser)).toEqual([]);
});

test('members see no getting-started steps', async ({ app, browser, screen }) => {
  await app.open(mock('/', '&sandboxes=0&role=member'));
  await expect(screen.getByRole('heading', { level: 1 })).toHaveText('Try Yulius on your own code');
  await expect(browser.locator('section[aria-label="Getting started"]')).toHaveCount(0);
});
