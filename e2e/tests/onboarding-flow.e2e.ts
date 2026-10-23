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
