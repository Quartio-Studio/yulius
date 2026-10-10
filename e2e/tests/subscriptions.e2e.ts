import { test } from '@e2e-dev/web';
import { expect } from 'e2e';
import { axeViolations, failOnPageErrors, mock } from './support.ts';

failOnPageErrors();

// Claude subscriptions as one pool (SPEC-001 ROT-009, ROT-010, ROT-013; F 2026-10-10).

test('the accounts page shows the pool capacity and nothing to manage but add and remove', async ({ app, browser, screen }) => {
  await app.open(mock('/admin/accounts'));
  await expect(screen.getByRole('heading', { level: 1 })).toHaveText('AI accounts');
  const capacity = screen.getByRole('region', 'Capacity');
  await expect(capacity.getByText(/Add about 1 subscription to stop pausing/)).toBeVisible();
  for (const figure of ['Available now', 'Left this week', 'Burn rate', 'Runs out', 'Paused (7 days)']) {
    await expect(capacity.getByText(figure)).toBeVisible();
  }
  await expect(screen.getByRole('button', 'Add subscription')).toBeVisible();
  await expect(screen.getByRole('button', 'Sign in again')).toBeVisible();
  // No priority, ordering or on/off controls remain (only the account list's Remove buttons).
  expect(await browser.evaluate(() => document.querySelectorAll('main input[type=number], main [role=switch]').length)).toBe(0);
  await expect(screen.getByText('Refresh usage')).toBeHidden();

  expect(await axeViolations(browser)).toEqual([]);
});

test('a task paused for capacity says so with the reset time, outside Needs you', async ({ app, screen }) => {
  await app.open(mock('/s/studio/t/12'));
  await expect(screen.getByRole('link', /Search indexing\s*, Paused: waiting for subscription capacity \(resets ~[^)]+\)/)).toBeVisible();
});

test('the dashboard shows the subscriptions of the whole instance', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/dashboard'));
  await expect(screen.getByRole('heading', { level: 1 })).toHaveText('Dashboard');
  await expect(screen.getByRole('heading', 'Subscriptions')).toBeVisible();
  await expect(screen.getByText(/Add about 1 subscription to stop pausing/)).toBeVisible();

  expect(await axeViolations(browser)).toEqual([]);
});
