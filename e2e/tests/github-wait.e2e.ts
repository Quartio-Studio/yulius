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

test('an open Status panel clears the GitHub wait at reset without polling', async ({ app, browser }) => {
  await app.open(mock('/s/studio/tasks'));
  const reset = await browser.evaluate(() => {
    const w = window as unknown as { __yuliusMock: { githubReset: number }; expire: () => void };
    const deadline = Date.now() + 904_000;
    w.__yuliusMock.githubReset = deadline;
    const set = window.setTimeout.bind(window);
    const callbacks: (() => void)[] = [];
    window.setTimeout = (handler: TimerHandler, ms?: number, ...args: unknown[]) => {
      if (typeof handler === 'function' && ms && ms > 800_000 && ms <= 904_000) {
        callbacks.push(() => handler(...args));
      }
      return set(handler, ms, ...args);
    };
    w.expire = () => {
      Date.now = () => deadline;
      callbacks.forEach(fn => fn());
    };
    return deadline;
  });
  await browser.locator('.btn-status').click();
  const notice = browser.locator('.status-pop .github-wait');
  await expect(notice).toHaveText(new RegExp(new Date(reset).toISOString().slice(11, 19)));
  const expired = await browser.evaluate(() => {
    (window as unknown as { expire: () => void }).expire();
    const notice = document.querySelector<HTMLElement>('.status-pop .github-wait');
    return !notice || notice.hidden;
  });
  expect(expired).toBe(true);
});
