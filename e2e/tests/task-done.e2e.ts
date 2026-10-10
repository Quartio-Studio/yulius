import { test } from '@e2e-dev/web';
import { expect } from 'e2e';
import { axeViolations, failOnPageErrors, mock } from './support.ts';

failOnPageErrors();

// F 2026-10-10 (TSK-014): "when the task's goal is achieved and none left shell is running, the
// task should be marked as finished (with green 'v' icon). Atm it keeps running."

test('a task whose goal is achieved is Done with a green check', async ({ app, browser, screen }) => {
  await app.open(mock('/s/product/t/8'));
  await expect(screen.getByRole('heading', { level: 1 })).toHaveText('#8 Improve the code');

  // The header says Done with the check, although the pull request's checks still run.
  const state = browser.locator('.strip-status');
  await expect(state).toHaveText(/^Done/);
  await expect(state).toHaveAttribute('data-tone', 'done');
  await expect(browser.locator('.strip-status .glyph.tone-done')).toBeVisible();

  // The sidebar lists it under Done, not Working.
  await expect(screen.getByRole('region', 'Done').getByRole('link', /Improve the code/)).toBeVisible();
  await expect(screen.getByRole('region', 'Working').getByRole('link', /Improve the code/)).toBeHidden();

  expect(await axeViolations(browser)).toEqual([]);
});

test('a finished goal with a background shell still running stays Working and says why', async ({ app, browser, screen }) => {
  await app.open(mock('/s/product/t/9'));
  await expect(screen.getByRole('heading', { level: 1 })).toHaveText('#9 Start the preview server');

  await expect(browser.locator('.strip-status')).toHaveText(/Working · 1 background shell still running/);
  await expect(screen.getByRole('region', 'Working').getByRole('link', /Start the preview server/)).toBeVisible();
  await expect(screen.getByRole('region', 'Done').getByRole('link', /Start the preview server/)).toBeHidden();
});

test('the Tasks page groups a task with an achieved goal under Done', async ({ app, browser, screen }) => {
  await app.open(mock('/s/product/tasks'));
  await expect(screen.getByRole('heading', { level: 1 })).toHaveText('Tasks');

  await expect(browser.locator('.tl-group.group-done')).toHaveText(/Improve the code/);
  await expect(browser.locator('.tl-group.group-working')).toHaveText(/Start the preview server/);
});
