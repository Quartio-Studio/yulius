import { test, type Browser } from '@e2e-dev/web';
import { expect } from 'e2e';
import { eventsConnected, failOnPageErrors, firstShown, mock, touchTask } from './support.ts';

failOnPageErrors();

// Notices with a key and an expiry in the bell (SPEC-624): a notice the server keeps joins the
// bell and is replaced by a newer one with its key; the server's removal drops it; every entry
// goes away at its expiry; a task's newer state replaces its entry; history kept before entries
// had expiries is given them. The mock runs with `ambient=0`: only the test changes things.

type Notice = { id: string; key: string; title: string; sandbox_id?: string | null; tone?: string; expires_at?: string | null; detail?: string | null; link?: string | null };
type Mocked = Window & { __yuliusMock: { postNotice: (n: Notice) => void; removeNotice: (id: string) => void; notices: { reads: number; dismissed: string[] } } };

/** The bell's entries in this browser, newest first: `[key, title, read, expires]`. */
const entries = (browser: Browser) => browser.evaluate(() => {
  const out: [string, string, boolean, string | null][] = [];
  for (let i = 0; i < localStorage.length; i += 1) {
    const name = localStorage.key(i) || '';
    if (!name.startsWith('yulius.notifications.') || name === 'yulius.notifications.filter') continue;
    for (const e of JSON.parse(localStorage.getItem(name) || '[]')) out.push([e.key, e.title, Boolean(e.read), e.expires ?? null]);
  }
  return out;
});

const post = (browser: Browser, notice: Notice) => browser.evaluate((n) => (window as unknown as Mocked).__yuliusMock.postNotice(n), notice);

test('a newer notice with the same key replaces the older one, and its removal drops it', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio', '&ambient=0'));
  await firstShown(browser.locator('.notif-btn'));
  await eventsConnected(browser);
  await post(browser, { id: 'nt_1', key: 'gate:quick::a', sandbox_id: 'sb_studio', tone: 'error', title: 'Integration gate: quick::a fails', link: '/s/studio/ci' });
  await expect.poll(() => entries(browser)).toEqual([['notice:sb_studio:-:gate:quick::a', 'Integration gate: quick::a fails', false, null]]);
  await browser.locator('.notif-btn').click();
  await screen.getByRole('button', 'Mark all read').click();
  await expect.poll(() => entries(browser).then((list) => list.map((e) => e[2]))).toEqual([true]);

  // The next run of the same failure: one entry, unread again; another key stands beside it.
  await post(browser, { id: 'nt_2', key: 'gate:quick::a', sandbox_id: 'sb_studio', tone: 'error', title: 'Integration gate: quick::a fails again' });
  await post(browser, { id: 'nt_3', key: 'gate:quick::b', sandbox_id: 'sb_studio', tone: 'error', title: 'Integration gate: quick::b fails' });
  await expect.poll(() => entries(browser).then((list) => list.map((e) => [e[1], e[2]]))).toEqual([
    ['Integration gate: quick::b fails', false],
    ['Integration gate: quick::a fails again', false],
  ]);

  // Repaired: the server removes it, and so does the bell.
  await browser.evaluate(() => (window as unknown as Mocked).__yuliusMock.removeNotice('nt_2'));
  await expect.poll(() => entries(browser).then((list) => list.map((e) => e[1]))).toEqual(['Integration gate: quick::b fails']);
});

test('an entry goes away at its expiry, and a cleared server notice is dismissed there', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio', '&ambient=0'));
  await firstShown(browser.locator('.notif-btn'));
  await eventsConnected(browser);
  const in30m = await browser.evaluate(() => new Date(Date.now() + 30 * 60 * 1000).toISOString());
  await post(browser, { id: 'nt_soon', key: 'watch:nightly', sandbox_id: 'sb_studio', title: 'Nightly build passed', expires_at: in30m });
  await post(browser, { id: 'nt_stays', key: 'watch:release', sandbox_id: 'sb_studio', title: 'Approve the release?' });
  await expect.poll(() => entries(browser).then((list) => list.length)).toBe(2);

  // Thirty-one minutes on, in the page's clock: the bell no longer shows it.
  await browser.evaluate(() => { const now = Date.now.bind(Date); Date.now = () => now() + 31 * 60 * 1000; });
  await browser.locator('.notif-btn').click();
  const panel = screen.getByRole('dialog', 'Notifications');
  await expect(panel.getByRole('button', /Approve the release\?/)).toBeVisible();
  expect(await panel.getByRole('button', /Nightly build passed/).count()).toBe(0);
  expect((await entries(browser)).map((e) => e[1])).toEqual(['Approve the release?']);

  // Clear dismisses the server's notice, so the next page does not bring it back.
  await panel.getByRole('button', 'Clear').click();
  await expect.poll(() => browser.evaluate(() => (window as unknown as Mocked).__yuliusMock.notices.dismissed)).toEqual(['nt_stays']);
});

test("a task's newer red run replaces its entry instead of adding one", async ({ app, browser }) => {
  await app.open(mock('/s/studio', '&ambient=0'));
  await firstShown(browser.locator('.notif-btn'));
  const red = (episode: string, run: string) => ({
    activity: 'needs_attention',
    needs_you_notice: { source: 'system', episode, happened: `CI run ${run} failed`, action: 'Fix the failing test', link: '', decision: false, at: new Date().toISOString() },
  });
  await touchTask(browser, 'sb_three', 1, red('pr_fail:41:run1:1', 'run1'));
  await expect.poll(() => entries(browser).then((list) => list.map((e) => [e[0], e[3]]))).toEqual([['task:tsk_three_1', null]]);
  // The next red run of the same pull request: one entry, the newer, unread again.
  await touchTask(browser, 'sb_three', 1, red('pr_fail:41:run2:2', 'run2'));
  await expect.poll(() => browser.evaluate(() => {
    const name = Object.keys(localStorage).find((k) => k.startsWith('yulius.notifications.') && k !== 'yulius.notifications.filter') || '';
    return JSON.parse(localStorage.getItem(name) || '[]').map((e: { key: string; detail: string; read: boolean }) => [e.key, e.detail.split(' · ')[0], e.read]);
  })).toEqual([['task:tsk_three_1', 'CI run run2 failed', false]]);
});

test('history kept before entries had expiries gets them, and what is already past them goes', async ({ app, browser }) => {
  await app.open(mock('/s/studio', '&ambient=0'));
  await firstShown(browser.locator('.notif-btn'));
  // The history as a page before SPEC-624 left it: no `expires`, no episode.
  await browser.evaluate(() => {
    const name = Object.keys(localStorage).find((k) => k.startsWith('yulius.notifications.') && k !== 'yulius.notifications.filter') || 'yulius.notifications.u_mock';
    const hours = (h: number) => new Date(Date.now() - h * 3600 * 1000).toISOString();
    localStorage.setItem(name, JSON.stringify([
      { id: 'a', kind: 'info', title: 'Recent report', detail: null, route: null, key: 'task:t1:needs_you:1', at: hours(2), read: false },
      { id: 'b', kind: 'info', title: 'Old report', detail: null, route: null, key: null, at: hours(30), read: true },
      { id: 'c', kind: 'error', title: 'A failure two days ago', detail: null, route: null, key: null, at: hours(48), read: false },
      { id: 'd', kind: 'ok', title: 'Done last week', detail: null, route: null, key: null, at: hours(24 * 8), read: true },
    ]));
  });
  await app.open(mock('/s/studio', '&ambient=0&again=1'));
  await firstShown(browser.locator('.notif-btn'));
  const kept = await entries(browser);
  expect(kept.map((e) => e[1])).toEqual(['Recent report', 'A failure two days ago']);
  const [recent, failure] = await browser.evaluate(() => {
    const name = Object.keys(localStorage).find((k) => k.startsWith('yulius.notifications.') && k !== 'yulius.notifications.filter') || '';
    return JSON.parse(localStorage.getItem(name) || '[]').map((e: { at: string; expires: string }) => Date.parse(e.expires) - Date.parse(e.at));
  });
  expect(recent).toBe(24 * 3600 * 1000);
  expect(failure).toBe(7 * 24 * 3600 * 1000);
});

test('a notice the server removed while no page was open is gone on the next load', async ({ app, browser }) => {
  await app.open(mock('/s/studio', '&ambient=0'));
  await firstShown(browser.locator('.notif-btn'));
  await eventsConnected(browser);
  await post(browser, { id: 'nt_gone', key: 'gate:quick::a', sandbox_id: 'sb_studio', tone: 'error', title: 'Integration gate: quick::a fails' });
  await expect.poll(() => entries(browser).then((list) => list.length)).toBe(1);
  // The next page's server holds no notice: repaired while nobody looked.
  await app.open(mock('/s/studio', '&ambient=0&again=1'));
  await expect.poll(() => browser.evaluate(() => (window as unknown as Mocked).__yuliusMock.notices.reads)).toBeGreaterThan(0);
  await expect.poll(() => entries(browser)).toEqual([]);
  await firstShown(browser.locator('.notif-btn'));
  expect(await browser.locator('.notif-badge').isHidden()).toBe(true);
});

test('an entry goes at its expiry while the panel stays closed', async ({ app, browser }) => {
  await app.open(mock('/s/studio', '&ambient=0'));
  await firstShown(browser.locator('.notif-btn'));
  await eventsConnected(browser);
  const in30m = await browser.evaluate(() => new Date(Date.now() + 30 * 60 * 1000).toISOString());
  await post(browser, { id: 'nt_soon', key: 'watch:nightly', sandbox_id: 'sb_studio', title: 'Nightly build passed', expires_at: in30m });
  await expect(browser.locator('.notif-badge')).toHaveText('1');
  await browser.evaluate(() => { const now = Date.now.bind(Date); Date.now = () => now() + 31 * 60 * 1000; });
  // Nothing is opened: the bell's own timer drops it.
  await expect.poll(() => entries(browser), { timeout: 15_000 }).toEqual([]);
  await expect(browser.locator('.notif-badge')).toBeHidden();
});
