import { test, type Browser } from '@e2e-dev/web';
import { expect } from 'e2e';
import { axeViolations, failOnPageErrors, mock } from './support.ts';

failOnPageErrors();

// Connection troubles (SPEC-361): while the egress proxy refuses a sandbox at its share, a warning
// pill in the tab row names the sandbox, says whose connections fill it and leads to that task; the
// notification center gets one entry per trouble, whatever its refusals; the pill goes once it ends.

type Mocked = { __yuliusMock: { setConnections(troubles: unknown[]): void } };

const share = (refused: number) => ({
  id: '1730370000000-0',
  limit: 'share',
  sandbox_id: 'studio',
  sandbox_name: 'studio',
  since_ms: 1730370000000,
  held: 128,
  max: 128,
  refused,
  holders: [
    { kind: 'task', label: 'task #12', task_id: 't12', task_number: 12, task_title: 'Run the e2e suite', sandbox_id: null, held: 120 },
    { kind: 'ci', label: 'a CI job', task_id: null, task_number: null, task_title: null, sandbox_id: null, held: 8 },
  ],
});

/** The notification center's entries for connection troubles, as this browser keeps them. */
const notified = (browser: Browser) => browser.evaluate(() => {
  const keys: string[] = [];
  for (let i = 0; i < localStorage.length; i += 1) {
    const name = localStorage.key(i) || '';
    if (!name.startsWith('yulius.notifications.') || name === 'yulius.notifications.filter') continue;
    for (const entry of JSON.parse(localStorage.getItem(name) || '[]')) if (String(entry.key || '').startsWith('connections:')) keys.push(entry.key);
  }
  return keys;
});

test('a full proxy share shows a pill naming the run, notifies once and goes when it ends', async ({ app, browser, screen }) => {
  await app.open(mock('/'));
  expect(await browser.locator('.connection-alert:not([hidden])').count()).toBe(0);

  await browser.evaluate((trouble) => (window as unknown as Mocked).__yuliusMock.setConnections([trouble]), share(1));
  await expect(screen.getByText('Proxy share full: studio')).toBeVisible();
  const link = browser.locator('.connection-alert a[href="/s/studio/t/12"]');
  expect(await link.count()).toBe(1);
  const title = await link.getAttribute('title');
  expect(title).toContain('task #12’s run holds 120 of them');
  expect(title).toContain('all of its 128 connections');
  expect(title).toContain('1 connection was refused so far.');
  expect(await axeViolations(browser)).toEqual([]);
  await expect.poll(() => notified(browser)).toEqual(['connections:1730370000000-0']);

  // Read, it stays read: more refusals within the same trouble notify nobody again.
  await browser.locator('.notif-btn').click();
  await screen.getByRole('button', { name: 'Mark all read' }).click();
  await expect(browser.locator('.notif-badge')).toBeHidden();
  await browser.evaluate((trouble) => (window as unknown as Mocked).__yuliusMock.setConnections([trouble]), share(40));
  await expect.poll(() => link.getAttribute('title')).toContain('40 connections were refused so far.');
  await expect(browser.locator('.notif-badge')).toBeHidden();
  expect(await notified(browser)).toEqual(['connections:1730370000000-0']);

  await browser.evaluate(() => (window as unknown as Mocked).__yuliusMock.setConnections([]));
  await expect(browser.locator('.connection-alert')).toBeHidden();
});

test('a full egress proxy and a full relay say what is full', async ({ app, browser, screen }) => {
  await app.open(mock('/'));
  await browser.evaluate(() => (window as unknown as Mocked).__yuliusMock.setConnections([
    { id: '1-0', limit: 'proxy', sandbox_id: null, sandbox_name: null, since_ms: 1, held: 256, max: 256, refused: 3, holders: [
      { kind: 'sandbox', label: 'studio', task_id: null, task_number: null, task_title: null, sandbox_id: 'studio', held: 130 },
      { kind: 'other_sandbox', label: 'another sandbox', task_id: null, task_number: null, task_title: null, sandbox_id: null, held: 126 },
    ] },
    { id: '2-1', limit: 'relay', sandbox_id: 'studio', sandbox_name: 'studio', since_ms: 2, held: 256, max: 256, refused: 1, holders: [] },
  ]));
  await expect(screen.getByText('Egress proxy full +1')).toBeVisible();
  const title = await browser.locator('.connection-alert .update-text').getAttribute('title');
  expect(title).toContain('The busiest: studio (130), another sandbox (126).');
  expect(title).toContain('studio’s relay toward');
  await expect.poll(() => notified(browser)).toEqual(['connections:2-1', 'connections:1-0']);
});
