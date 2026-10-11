import { test } from '@e2e-dev/web';
import { expect } from 'e2e';
import { failOnPageErrors, mock } from './support.ts';

failOnPageErrors();

// F 2026-10-10 (STP-006, #139): "make sure todos declared by every task takes similar amount of
// time: 9/10 should mean 90%". Steps are of similar effort; the one that cannot be split declares
// a larger effort, and the progress bar is weighted by it.

test('the task progress bar is weighted by the effort of its steps', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/t/12'));
  await expect(screen.getByRole('heading', { level: 1 })).toHaveText('#12 Checkout API');

  // Two of seven steps are done; the open "Implement checkout API" step carries effort 3,
  // so the plan is 2 of 9 effort units: 22 %, not 28 %.
  const bar = browser.locator('.step-progress');
  await expect(bar).toHaveAttribute('aria-valuenow', '22');
  await expect(bar).toHaveAttribute('aria-label', '2 of 7 steps done, 22% of the plan');
});
