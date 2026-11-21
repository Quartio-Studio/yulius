import { test } from '@e2e-dev/web';
import { expect } from 'e2e';
import { failOnPageErrors, mock } from './support.ts';

failOnPageErrors();

test('task lists and Status show the GitHub reset once per view', async ({ app, browser }) => {
  const reset = Date.now() + 904_000;
  await app.open(mock('/s/studio/tasks', `&github_reset=${reset}`));
  const utc = new Date(reset).toISOString().slice(11, 19);
  const message = `GitHub rate limit reached. Merges, CI status updates and the staging queue are waiting until ${utc} UTC.`;
  await expect(browser.locator('.app > .github-wait')).toHaveText(message);
  await browser.locator('.btn-status').click();
  await expect(browser.locator('.status-pop .github-wait')).toHaveText(message);
  await expect(browser.locator('.app > .github-wait')).toBeHidden();
});
