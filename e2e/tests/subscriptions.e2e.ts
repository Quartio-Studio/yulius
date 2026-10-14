import { test } from '@e2e-dev/web';
import { expect } from 'e2e';
import { axeViolations, failOnPageErrors, mock } from './support.ts';

failOnPageErrors();

// Claude subscriptions as one pool (SPEC-001 ROT-009, ROT-010, ROT-013; F 2026-10-10).

test('the subscriptions page shows the pool capacity and nothing to manage but add and remove', async ({ app, browser, screen }) => {
  await app.open(mock('/admin/accounts'));
  // One name for one thing (#147): the page, the menu and Settings say "Subscriptions".
  await expect(screen.getByRole('heading', { level: 1 })).toHaveText('Subscriptions');
  const capacity = screen.getByRole('region', 'Capacity');
  await expect(capacity.getByText(/Add about 1 subscription to stop pausing/)).toBeVisible();
  for (const figure of ['Available now', 'Left this week', 'Burn rate', 'Runs out', 'Tokens (7 days)', 'API cost (7 days)', 'Paused (7 days)']) {
    await expect(capacity.getByText(figure)).toBeVisible();
  }
  // Token use and its cost at API list prices, each with a 7-day chart (SPEC-110 TUS-006, TUS-010).
  await expect(capacity.getByText(/^≈ \$[\d,.]+$/)).toBeVisible();
  await expect(capacity.getByText(/at Claude API list prices/)).toBeVisible();
  for (const name of ['Tokens', 'API cost', 'Burn rate', 'Paused']) {
    await expect(capacity.getByRole('img', new RegExp(`^${name}, last 7 days: first .+, highest .+, last .+$`))).toBeVisible();
  }
  // Each subscription's observed tokens say what they would cost on the API (TUS-004).
  await expect(screen.getByText(/^[\d.,]+[KMB]? tokens · 9 runs · 8 tasks · ≈ \$[\d,.]+ at API prices$/).first()).toBeVisible();
  await expect(screen.getByRole('button', 'Add subscription')).toBeVisible();
  await expect(screen.getByRole('button', 'Sign in again')).toBeVisible();
  // No priority, ordering or on/off controls remain (only the account list's Remove buttons).
  expect(await browser.evaluate(() => document.querySelectorAll('main input[type=number], main [role=switch]').length)).toBe(0);
  await expect(screen.getByText('Refresh usage')).toBeHidden();
  // A usage reading that failed for now is one quiet sentence, no HTTP codes (F 2026-10-10).
  await expect(screen.getByText('Usage updates again in about 12 min.')).toBeVisible();
  expect(await browser.evaluate(() => (document.querySelector('main') || document.body).innerText.includes('HTTP'))).toBe(false);
  await expect(screen.getByText(/Tasks don.t belong to a subscription/)).toBeVisible();

  expect(await axeViolations(browser)).toEqual([]);
});

test('tasks are subscription-agnostic: no account choice or account in the task view', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/t/12'));
  await browser.locator('.taskview .stepstrip-main [aria-label="Task actions"]').click();
  await expect(screen.getByRole('menuitem', /^Stop/)).toBeVisible();
  await expect(screen.getByRole('menuitem', /Switch account/)).toBeHidden();
  await browser.keyboard.press('Escape');
  // The subscription the task runs on shows only in Task details.
  await expect(browser.locator('.taskview .stepstrip-main')).not.toHaveText(/Claude #2/);
});

test('a task paused because no subscription has capacity left says so with the reset time, outside Needs you', async ({ app, screen }) => {
  await app.open(mock('/s/studio/t/12'));
  await expect(screen.getByRole('link', /Search indexing\s*, Paused: no subscription has capacity left \(resets ~[^)]+\)/)).toBeVisible();
});

test('the dashboard shows the subscriptions of the whole instance', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/dashboard'));
  await expect(screen.getByRole('heading', { level: 1 })).toHaveText('Dashboard');
  await expect(screen.getByRole('heading', 'Subscriptions')).toBeVisible();
  await expect(screen.getByText(/Add about 1 subscription to stop pausing/)).toBeVisible();

  expect(await axeViolations(browser)).toEqual([]);
});
