import { test, type Browser } from '@e2e-dev/web';
import { expect } from 'e2e';
import { eventsConnected, failOnPageErrors, firstShown, mock, touchTask } from './support.ts';

failOnPageErrors();

// The notification center and a session that is restarted under the hood (SPEC-468 CCF-008): a
// task whose idle session moves to a newer Claude Code, or to another subscription, reads
// "Continuing" for a moment and then as it did. That is no news: its person is not told a second
// time. A switch after which the task reads differently is news, and so is the end of a turn that
// went on after a switch. The mock runs with `ambient=0`: only the test changes tasks. Desktop
// notifications are on, with a stand-in for the browser's `Notification` that keeps what it is
// asked to show: the page is told that they are allowed and that nobody is looking at it.

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

/** Turns desktop notifications on, with a stand-in that keeps what it is asked to show. */
async function desktopNotifications(browser: Browser): Promise<void> {
  await browser.addInitScript(() => {
    const shown: string[] = [];
    (window as unknown as { __shown: string[] }).__shown = shown;
    class Shown {
      static permission = 'granted';
      onclick: (() => void) | null = null;
      constructor(title: string, options?: { body?: string }) {
        shown.push(`${title} | ${options?.body ?? ''}`);
      }
      close(): void {}
    }
    (window as unknown as { Notification: unknown }).Notification = Shown;
    document.hasFocus = () => false;
    localStorage.setItem('yulius.desktopNotifications', 'true');
  });
}

/** The desktop notifications about tasks shown so far, oldest first. */
const desktop = (browser: Browser) => browser.evaluate(() => (window as unknown as { __shown: string[] }).__shown.filter((line) => line.startsWith('#')));

const YOUR_TURN: [string, string] = ['#1 Explore data model', 'Your turn · sandbox-3'];
const YOUR_TURN_SHOWN = '#1 Explore data model | Your turn · sandbox-3';

test('a session restarted under the hood that reads as before notifies nobody again', async ({ app, browser, screen }) => {
  await desktopNotifications(browser);
  await app.open(mock('/s/studio', '&ambient=0'));
  await firstShown(browser.locator('.notif-btn'));
  expect(await notified(browser)).toEqual([]);

  // A task of another sandbox ends its turn with work left: its person is told, once, in the
  // bell and on the desktop, and reads it.
  await touchTask(browser, 'sb_three', 1, { activity: 'waiting' });
  await expect.poll(() => notified(browser)).toEqual([[...YOUR_TURN, false]]);
  expect(await desktop(browser)).toEqual([YOUR_TURN_SHOWN]);
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
  // The desktop heard of the other task only: no second "Your turn".
  expect(await desktop(browser)).toEqual([YOUR_TURN_SHOWN, '#12 Checkout API | Needs your action · studio']);

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

// SPEC-629 RBL-006, RBL-007: an idle task moved off a nearly used-up subscription is told once in
// the bell, keyed per move; one that stays because nothing has room gets one entry per episode.
type Rebalanced = Window & { __yuliusMock: { sendRebalance: (event: Record<string, unknown>) => void } };

/** The notification center's entries about subscription moves, newest first. */
const moves = (browser: Browser) => browser.evaluate(() => {
  const entries: [string, string][] = [];
  for (let i = 0; i < localStorage.length; i += 1) {
    const name = localStorage.key(i) || '';
    if (!name.startsWith('yulius.notifications.') || name === 'yulius.notifications.filter') continue;
    for (const entry of JSON.parse(localStorage.getItem(name) || '[]')) {
      if (String(entry.key || '').startsWith('rebalance:')) entries.push([entry.title, entry.detail]);
    }
  }
  return entries;
});

test('a task moved off a nearly used-up subscription is told once (SPEC-629)', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio', '&ambient=0'));
  await firstShown(browser.locator('.notif-btn'));
  await eventsConnected(browser);
  const send = (event: Record<string, unknown>) => browser.evaluate((e) => (window as unknown as Rebalanced).__yuliusMock.sendRebalance(e), event);
  const task = { task_id: 't_explore', sandbox_id: 'sb_three', number: 1, title: 'Explore data model' };
  const moved = { ...task, from: 'Claude a-first', to: 'Claude b-second', said: 'is at 96% of its 5-hour window', key: 'rebalance:t_explore:run-1' };
  await send(moved);
  await send(moved);
  await expect.poll(() => moves(browser)).toEqual([
    ['#1 Explore data model moved to Claude b-second', 'Claude a-first is at 96% of its 5-hour window'],
  ]);
  const stays = { ...task, from: 'Claude b-second', said: 'is at 97% of its week', key: 'rebalance:t_explore:b-second:1700000000000' };
  await send(stays);
  await send(stays);
  await expect.poll(() => moves(browser)).toEqual([
    ['#1 Explore data model stays on Claude b-second', 'Claude b-second is at 97% of its week and no other subscription has room'],
    ['#1 Explore data model moved to Claude b-second', 'Claude a-first is at 96% of its 5-hour window'],
  ]);
  await send({ ...task, from: 'Claude b-second', parked: true, said: 'is at 96% of its week', key: 'rebalance:t_explore:parked:1' });
  await expect.poll(() => moves(browser)).toEqual([
    ['#1 Explore data model parked off Claude b-second', 'Claude b-second is at 96% of its week; it continues on a subscription with room when written to'],
    ['#1 Explore data model stays on Claude b-second', 'Claude b-second is at 97% of its week and no other subscription has room'],
    ['#1 Explore data model moved to Claude b-second', 'Claude a-first is at 96% of its 5-hour window'],
  ]);
  await browser.locator('.notif-btn').click();
  await screen.getByRole('dialog', 'Notifications').getByRole('button', /^Unread: #1 Explore data model moved to Claude b-second/).click();
  await expect.poll(() => browser.evaluate(() => location.hash)).toBe('#/s/sandbox-3/t/1');
});
