import { test } from '@e2e-dev/web';
import { expect } from 'e2e';
import { axeViolations, failOnPageErrors, mock } from './support.ts';

failOnPageErrors();

// The mock's studio sandbox: its GitHub App lacks "Commit statuses" (assets/mock.js, checksAccess).
const MISSING = /can't read commit statuses for Quartio-Studio\/studio: the GitHub App needs the 'Commit statuses: read' permission/;

// F 2026-10-10 (#111): "bruh" at a yellow "GitHub: checks cannot be read: GitHub: GitHub refused
// the request (HTTP 403)" banner. A permission the App lacks is one quiet line for administrators.
test('the Git page names a missing GitHub App permission to administrators until dismissed', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/git'));
  await expect(screen.getByRole('heading', { level: 1 })).toHaveText('Git');
  await expect(screen.getByText(MISSING)).toBeVisible();
  await expect(screen.getByRole('link', 'Open the App’s permissions')).toHaveAttribute(
    'href',
    'https://github.com/organizations/Quartio-Studio/settings/apps/yulius-quartio-studio/permissions',
  );
  // Not a warning banner.
  expect(await browser.evaluate(() => document.querySelectorAll('.banner-warn').length)).toBe(0);
  expect(await axeViolations(browser)).toEqual([]);

  // Dismissed, it stays hidden for the session, also after a reload.
  await screen.getByRole('button', 'Dismiss until the next session').click();
  await expect(screen.getByText(MISSING)).toBeHidden();
  await browser.reload();
  await expect(screen.getByRole('heading', { level: 1 })).toHaveText('Git');
  await expect(screen.getByRole('heading', /^Blocked/)).toBeVisible();
  await expect(screen.getByText(MISSING)).toBeHidden();
});

test('members who cannot change the GitHub App do not see it', async ({ app, screen }) => {
  await app.open(mock('/s/studio/git', '&role=member'));
  await expect(screen.getByRole('heading', { level: 1 })).toHaveText('Git');
  await expect(screen.getByRole('heading', /^Blocked/)).toBeVisible();
  await expect(screen.getByText(MISSING)).toBeHidden();
});

// "git should be presentation layer strictly because all responsibilities will be handled by
// agents" (F 2026-10-10, #136): status, links and output only; agents merge, deploy and release.
const ACTIONS = /^(merge|release|deploy|mark .* verified|verify|open pull request|deliver\b)/i;

test('the Git page shows delivery state without merge, release, deploy or verify buttons', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/git'));
  await expect(screen.getByRole('heading', { level: 1 })).toHaveText('Git');
  await expect(screen.getByRole('heading', /^Where every task is/)).toBeVisible();
  await expect(screen.getByRole('heading', /^Blocked/)).toBeVisible();
  await expect(screen.getByText(/Agents merge and release as part of their workflow/)).toBeVisible();
  await expect(screen.getByRole('link', 'Pull requests')).toHaveAttribute('href', /\/pulls$/);
  const labels = await browser.evaluate(() => [...document.querySelectorAll('#main button, #main a.btn')].map((b) => (b.textContent ?? '').trim()));
  expect(labels.filter((label) => ACTIONS.test(label))).toEqual([]);

  // A task's delivery details are read-only too.
  await browser.locator('.dl-deliver').first().click();
  const dialog = screen.getByRole('dialog', /^Delivery · #/);
  await expect(dialog).toBeVisible();
  await expect(dialog.getByText(/Agents open, merge, deploy and release as part of their workflow/)).toBeVisible();
  const dialogLabels = await browser.evaluate(() => [...document.querySelectorAll('dialog[open] button')].map((b) => (b.textContent ?? '').trim()));
  expect(dialogLabels.filter((label) => ACTIONS.test(label))).toEqual([]);
  expect(await axeViolations(browser)).toEqual([]);
  await browser.keyboard.press('Escape');
  await expect(dialog).toBeHidden();

  // The task view's delivery line shows the pull request, not a Merge button.
  await app.open(mock('/s/studio/t/13'));
  await expect(screen.getByRole('heading', { level: 1 })).toHaveText(/^#13 /);
  await expect(screen.getByRole('button', /^PR #520, pull request/)).toBeVisible();
  await expect(screen.getByRole('button', 'Merge…')).toBeHidden();
});

// Task #108 (SPEC-122 TID-3): a bare #N is a task's number, so a pull request's chip says "PR".
test('pull request chips read "PR #N" wherever they show', async ({ app, browser, screen }) => {
  const chipText = (selector: string) => browser.evaluate((s: string) => [...document.querySelectorAll(s)].map((el) => (el.textContent ?? '').replace(/\s+/g, ' ').trim()), selector);

  await app.open(mock('/s/studio/tasks'));
  await expect(screen.getByRole('heading', { level: 1 })).toHaveText('Tasks');
  await expect(browser.locator('.task-list .tl-pr-chip').first()).toBeVisible();
  const sidebar = await chipText('.task-list .tl-pr-chip');
  expect(sidebar.find((t) => t.includes('520'))).toMatch(/^PR #520(?!\d)/);
  await expect(browser.locator('#main .tl-pr-chip').first()).toBeVisible();
  for (const text of await chipText('#main .tl-pr-chip')) expect(text).toMatch(/^PR #\d+$/);
  // The word replaces the branch icon (only a checks icon may follow it); the tooltip still names
  // the pull request in full.
  expect(await browser.evaluate(() => [...document.querySelectorAll('.tl-pr-chip')].filter((el) => el.firstElementChild?.tagName === 'svg').length)).toBe(0);
  await expect(browser.locator('#main a.tl-pr-chip').first()).toHaveAttribute('title', /^Pull request #\d+/);

  // The sidebar row's chip is heard as a pull request, not as "PR".
  await expect(screen.getByRole('link', /Task #13 PR #520\s*, pull request, open/)).toBeVisible();
  // Review round 2 (WCAG 2.5.3): a chip's name starts with what it shows.
  expect(await browser.evaluate(() => [...document.querySelectorAll('#main .tl-pr-chip')]
    .filter((el) => !(el.getAttribute('aria-label') ?? '').startsWith((el.textContent ?? '').trim())).length)).toBe(0);

  await app.open(mock('/s/studio/t/13'));
  await expect(screen.getByRole('button', /^PR #520, pull request/)).toHaveText('PR #520');

  // Review round 1: the Git board's cards carry their own chip, under the task's #N.
  await app.open(mock('/s/studio/git'));
  await expect(screen.getByRole('heading', { level: 1 })).toHaveText('Git');
  await expect(browser.locator('.dl-card-task .tl-pr-chip').first()).toBeVisible();
  for (const text of await chipText('#main .tl-pr-chip')) expect(text).toMatch(/^PR #\d+$/);
  expect(await browser.evaluate(() => [...document.querySelectorAll('.tl-pr-chip')].filter((el) => el.firstElementChild?.tagName === 'svg').length)).toBe(0);
  // And the delivery details name the pull request as one.
  await browser.locator('.dl-deliver').first().click();
  const dialog = screen.getByRole('dialog', /^Delivery · #/);
  await expect(dialog.getByRole('link', /^PR #\d+$/).first()).toBeVisible();
});
