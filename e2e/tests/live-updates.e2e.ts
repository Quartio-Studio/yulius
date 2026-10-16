import { test } from '@e2e-dev/web';
import { expect } from 'e2e';
import { failOnPageErrors, mock } from './support.ts';

failOnPageErrors();

// Live task lists (SPEC-191 LIV): the event stream says it is alive with a heartbeat, so a
// connection that died without a close (a laptop asleep, a network change, a proxy that dropped
// it) is noticed and replaced, and the lists catch up. `heartbeat=300` makes the mock beat every
// 300 ms, and the page then gives up on a stream silent for 750 ms.

type Mocked = { __yuliusMock: { heartbeats: number; silenceEvents: () => void; touchTask: (sid: string, n: number, c: object) => void } };

test('a stream that dies without a close is replaced and the lists catch up', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/tasks', '&heartbeat=300'));
  const mark = (label: string) => browser.locator(`.tl-row[data-task-id="tsk_studio_12"] [role="img"][aria-label="${label}"]`);
  const sidebar = browser.locator('.task-row a[title^="Task #12 Checkout API"]');
  await expect(mark('Working')).toBeVisible();
  await expect(sidebar).toHaveAttribute('title', /— Working/);

  // The connection dies silently: nothing reaches the page, and no close either.
  await browser.evaluate(() => (window as unknown as Mocked).__yuliusMock.silenceEvents());
  await browser.evaluate(() => (window as unknown as Mocked).__yuliusMock.touchTask('sb_studio', 12, { activity: 'needs_attention' }));

  // The page notices the silence, reconnects and reloads the lists, with no reload of its own.
  await expect(mark('Needs your action')).toBeVisible({ timeout: 5000 });
  await expect(sidebar).toHaveAttribute('title', /— Needs your action/);
  await expect(screen.getByText('Can’t reach Yulius', { exact: false })).toBeHidden();
});

test('a tab shown again replaces a silent stream before the watchdog would', async ({ app, browser }) => {
  // Beats every 2 s: the watchdog gives up after 5 s, a tab shown again after 3 s of silence.
  await app.open(mock('/s/studio/tasks', '&heartbeat=2000'));
  const mark = (label: string) => browser.locator(`.tl-row[data-task-id="tsk_studio_12"] [role="img"][aria-label="${label}"]`);
  await expect(mark('Working')).toBeVisible();
  // Silenced right after a beat: the watchdog alone could not catch up within 5 s of it.
  const beats = await browser.evaluate(() => (window as unknown as Mocked).__yuliusMock.heartbeats);
  await expect.poll(() => browser.evaluate(() => (window as unknown as Mocked).__yuliusMock.heartbeats), { timeout: 5000, intervals: [20] }).toBeGreaterThan(beats);
  const silencedAt = await browser.evaluate(() => {
    (window as unknown as Mocked).__yuliusMock.silenceEvents();
    (window as unknown as Mocked).__yuliusMock.touchTask('sb_studio', 12, { activity: 'needs_attention' });
    return Date.now();
  });
  // The tab is shown again (and again) until the lists caught up.
  await expect.poll(async () => browser.evaluate(() => {
    document.dispatchEvent(new Event('visibilitychange'));
    return Boolean(document.querySelector('.tl-row[data-task-id="tsk_studio_12"] [role="img"][aria-label="Needs your action"]'));
  }), { timeout: 4500, intervals: [200] }).toBe(true);
  const caughtUp = await browser.evaluate(() => Date.now());
  expect(caughtUp - silencedAt).toBeLessThan(4500);
});
