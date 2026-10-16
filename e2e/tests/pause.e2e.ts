import { test } from '@e2e-dev/web';
import { expect } from 'e2e';
import { failOnPageErrors, mock } from './support.ts';

failOnPageErrors();

// Paused (SPEC-191 PAU, PER): a task whose live run an update holds, or a person paused, reads
// Paused everywhere it shows, and says why.

type Mocked = { __yuliusMock: { touchTask: (sid: string, n: number, c: object) => void } };

const touch = (browser: { evaluate: Function }, changes: object) =>
  browser.evaluate((c: object) => (window as unknown as Mocked).__yuliusMock.touchTask('sb_studio', 12, c), changes);

test('a task an update holds reads Paused in the list, the sidebar and its view', async ({ app, browser }) => {
  await app.open(mock('/s/studio/tasks'));
  const row = '.tl-row[data-task-id="tsk_studio_12"]';
  const sidebar = browser.locator('.task-row a[title^="Task #12 Checkout API"]');
  await expect(browser.locator(`${row} [role="img"][aria-label="Working"]`)).toBeVisible();

  await touch(browser, { pause: { by: 'update', user: null, since: null, pending: true } });
  await expect(browser.locator(`${row} [role="img"][aria-label="Paused"]`)).toBeVisible();
  await expect(browser.locator(row)).toContainText('Pausing for a Yulius update at the end of its turn');
  await touch(browser, { pause: { by: 'update', user: null, since: null, pending: false } });
  await expect(browser.locator(row)).toContainText('Paused for a Yulius update. It continues by itself.');
  await expect(sidebar).toHaveAttribute('title', /— Paused/);

  await app.open(mock('/s/studio/t/12'));
  await expect(browser.locator('.strip-status')).toContainText('Paused');

  // Released: it works again.
  await touch(browser, { pause: null });
  await expect(browser.locator('.strip-status')).toContainText('Working');
  await expect(sidebar).toHaveAttribute('title', /— Working/);
});
