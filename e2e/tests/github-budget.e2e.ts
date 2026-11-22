import { test } from '@e2e-dev/web';
import { expect } from 'e2e';
import { failOnPageErrors, mock } from './support.ts';

failOnPageErrors();

test('Status shows the GitHub reserve and request purposes', async ({ app, browser }) => {
  await app.open(mock('/s/studio/git'));
  await browser.evaluate(() => {
    (window as unknown as { __yuliusMock: { githubBudget: unknown } }).__yuliusMock.githubBudget = {
      limit: 5000, remaining: 400, used: 4600, unattributed_minimum: 390,
      resets_at: Math.floor(Date.now() / 1000) + 900, attempts: { display: 4210 },
      not_modified: 0, display_paused: true,
    };
  });
  await browser.locator('.btn-status').click();
  const budget = browser.locator('.status-section:has(h2:text-is("GitHub API"))');
  await expect(budget).toContainText('400 of 5,000 remaining');
  await expect(budget).toContainText('Display refreshes paused');
  await expect(budget).toContainText('Staging queue');
  await expect(budget).toContainText('Display reads');
});
