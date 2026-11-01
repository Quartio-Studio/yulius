import { test, type Browser } from '@e2e-dev/web';
import { expect } from 'e2e';
import { axeViolations, failOnPageErrors, mock } from './support.ts';

failOnPageErrors();

// Connection troubles (SPEC-361): while the egress proxy refuses a sandbox at its share, a warning
// pill in the tab row names the sandbox, says whose connections fill it and leads to that task; the
// notification center gets one entry per trouble, also after Clear and a reload; the pill goes once
// it ends, also when the event was lost (`resync`).

type Mocked = { __yuliusMock: { setConnections(troubles: unknown[], announce?: boolean): void; resync(): void } };

const SHARE = {
  id: '1730370000000-0',
  limit: 'share',
  sandbox_id: 'studio',
  sandbox_name: 'studio',
  since_ms: 1730370000000,
  held: 128,
  max: 128,
  refused: 1,
  holders: [
    { kind: 'task', label: 'task #12', task_id: 't12', task_number: 12, task_title: 'Run the e2e suite', sandbox_id: null, held: 120 },
    { kind: 'ci', label: 'a CI job', task_id: null, task_number: null, task_title: null, sandbox_id: null, held: 8 },
  ],
};
const RELAY = { id: '1730370060000-1', limit: 'relay', sandbox_id: 'studio', sandbox_name: 'studio', since_ms: 1730370060000, held: 256, max: 256, refused: 1, holders: [] };

/** The notification center's entries for connection troubles in this browser, newest first. */
const notified = (browser: Browser) => browser.evaluate(() => {
  const entries: [string, boolean][] = [];
  for (let i = 0; i < localStorage.length; i += 1) {
    const name = localStorage.key(i) || '';
    if (!name.startsWith('yulius.notifications.') || name === 'yulius.notifications.filter') continue;
    for (const entry of JSON.parse(localStorage.getItem(name) || '[]')) {
      if (String(entry.key || '').startsWith('connections:')) entries.push([entry.key, Boolean(entry.read)]);
    }
  }
  return entries;
});

const set = (browser: Browser, troubles: unknown[], announce = true) =>
  browser.evaluate(([list, say]) => (window as unknown as Mocked).__yuliusMock.setConnections(list as unknown[], say as boolean), [troubles, announce] as const);

test('a full proxy share shows a pill naming the run and notifies once per trouble', async ({ app, browser, screen }) => {
  await app.open(mock('/'));
  expect(await browser.locator('.connection-alert:not([hidden])').count()).toBe(0);

  await set(browser, [SHARE]);
  await expect(screen.getByText('Proxy share full: studio')).toBeVisible();
  const link = browser.locator('.connection-alert a[href="/s/studio/t/12"]');
  expect(await link.count()).toBe(1);
  const title = await link.getAttribute('title');
  expect(title).toContain('task #12’s run holds 120 of them');
  expect(title).toContain('all of its 128 connections');
  expect(title).toContain('Since ');
  expect(await axeViolations(browser)).toEqual([]);
  await expect.poll(() => notified(browser)).toEqual([[`connections:${SHARE.id}`, false]]);

  // Read, it stays read when the panel reads the troubles again for another one.
  await browser.locator('.notif-btn').click();
  await screen.getByRole('button', { name: 'Mark all read' }).click();
  await set(browser, [SHARE, RELAY]);
  await expect(screen.getByText('Proxy share full: studio +1')).toBeVisible();
  // With two troubles the pill leads nowhere in particular.
  expect(await browser.locator('.connection-alert a').count()).toBe(0);
  expect(await notified(browser)).toEqual([[`connections:${RELAY.id}`, false], [`connections:${SHARE.id}`, true]]);

  // Cleared, and the page loaded again (its first read lists the open troubles, as the server's
  // would): they do not notify again.
  await screen.getByRole('button', { name: 'Clear' }).click();
  await expect.poll(() => notified(browser)).toEqual([]);
  await browser.reload();
  await expect(screen.getByText('Proxy share full: studio +1')).toBeVisible();
  expect(await notified(browser)).toEqual([]);
});

test('the pill goes when its trouble ends, also when the event was lost', async ({ app, browser, screen }) => {
  await app.open(mock('/'));
  await set(browser, [SHARE]);
  await expect(screen.getByText('Proxy share full: studio')).toBeVisible();
  await set(browser, []);
  await expect(browser.locator('.connection-alert')).toBeHidden();

  await set(browser, [SHARE]);
  await expect(screen.getByText('Proxy share full: studio')).toBeVisible();
  await set(browser, [], false);
  await browser.evaluate(() => (window as unknown as Mocked).__yuliusMock.resync());
  await expect(browser.locator('.connection-alert')).toBeHidden();
});

test('a full egress proxy and a full relay say what is full', async ({ app, browser, screen }) => {
  await app.open(mock('/'));
  await set(browser, [
    { id: '1-0', limit: 'proxy', sandbox_id: null, sandbox_name: null, since_ms: 1, held: 256, max: 256, refused: 3, holders: [
      { kind: 'sandbox', label: 'studio', task_id: null, task_number: null, task_title: null, sandbox_id: 'studio', held: 130 },
      { kind: 'other_sandbox', label: 'another sandbox', task_id: null, task_number: null, task_title: null, sandbox_id: null, held: 126 },
    ] },
    RELAY,
  ]);
  await expect(screen.getByText('Egress proxy full +1')).toBeVisible();
  const title = await browser.locator('.connection-alert .update-text').getAttribute('title');
  expect(title).toContain('The busiest: studio (130), another sandbox (126).');
  expect(title).toContain('studio’s relay toward');
  await expect.poll(() => notified(browser)).toEqual([[`connections:${RELAY.id}`, false], ['connections:1-0', false]]);
});
