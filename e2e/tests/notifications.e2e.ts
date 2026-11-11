import { test, type Browser } from '@e2e-dev/web';
import { expect } from 'e2e';
import { failOnPageErrors, firstShown, mock, touchTask } from './support.ts';

failOnPageErrors();

// The notification center and a session that is restarted under the hood (SPEC-468 CCF-008): a
// task whose idle session moves to a newer Claude Code, or to another subscription, reads
// "Continuing" for a moment and then as it did. That is no news: its person is not told a second
// time. A switch after which the task reads differently is news, and so is the end of a turn that
// went on after a switch. The mock runs with `ambient=0`: only the test changes tasks.

/** The notification center's entries about tasks in this browser, newest first. */
const notified = (browser: Browser) => browser.evaluate(() => {
  const entries: [string, string, boolean][] = [];
  for (let i = 0; i < localStorage.length; i += 1) {
    const name = localStorage.key(i) || '';
    if (!name.startsWith('yulius.notifications.') || name === 'yulius.notifications.filter') continue;
    for (const entry of JSON.parse(localStorage.getItem(name) || '[]')) {
      if (String(entry.key || '').startsWith('task:')) entries.push([entry.title, entry.detail, Boolean(entry.read)]);
    }
  }
  return entries;
});

const YOUR_TURN: [string, string] = ['#1 Explore data model', 'Your turn · sandbox-3'];

test('a session restarted under the hood that reads as before notifies nobody again', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio', '&ambient=0'));
  await firstShown(browser.locator('.notif-btn'));
  expect(await notified(browser)).toEqual([]);

  // A task of another sandbox ends its turn with work left: its person is told, once, and reads it.
  await touchTask(browser, 'sb_three', 1, { activity: 'waiting' });
  await expect.poll(() => notified(browser)).toEqual([[...YOUR_TURN, false]]);
  await browser.locator('.notif-btn').click();
  await screen.getByRole('button', { name: 'Mark all read' }).click();
  await expect.poll(() => notified(browser)).toEqual([[...YOUR_TURN, true]]);

  // Its idle session restarts with nothing typed: "Continuing", then their turn as before.
  await touchTask(browser, 'sb_three', 1, { status: 'switching', activity: null });
  await touchTask(browser, 'sb_three', 1, { status: 'running', activity: 'waiting' });
  // Another task's news comes after it: by then the restart was heard, and it was none.
  await touchTask(browser, 'sb_studio', 12, { activity: 'needs_attention' });
  await expect.poll(() => notified(browser)).toEqual([
    ['#12 Checkout API', 'Needs your action · studio', false],
    [...YOUR_TURN, true],
  ]);
  expect(await browser.locator('.notif-badge').textContent()).toBe('1');

  // A restart after which the task reads differently is news: here it could not continue.
  await touchTask(browser, 'sb_three', 1, { status: 'switching', activity: null });
  await touchTask(browser, 'sb_three', 1, { status: 'stopped_switch_failed', activity: null });
  await expect.poll(() => notified(browser)).toEqual([
    ['#1 Explore data model', 'Stopped (switch failed) · sandbox-3', false],
    ['#12 Checkout API', 'Needs your action · studio', false],
  ]);
});

test('a session moved while it works tells its person when its turn ends', async ({ app, browser }) => {
  await app.open(mock('/s/studio', '&ambient=0'));
  await firstShown(browser.locator('.notif-btn'));

  // Working, moved to another subscription at a turn's end, working on: nothing to tell.
  await touchTask(browser, 'sb_three', 1, { status: 'switching', activity: null });
  await touchTask(browser, 'sb_three', 1, { status: 'running', activity: 'working' });
  // Its turn ends: its person is told.
  await touchTask(browser, 'sb_three', 1, { activity: 'waiting' });
  await expect.poll(() => notified(browser)).toEqual([[...YOUR_TURN, false]]);
});
