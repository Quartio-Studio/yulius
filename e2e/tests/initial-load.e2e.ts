import assert from 'node:assert/strict';
import { afterEach, beforeEach, test } from '@e2e-dev/web';
import { expect } from 'e2e';
import { surface } from '../chromium.ts';
import { failOnPageErrors, FIRST_LOAD_MS, mock } from './support.ts';

failOnPageErrors();

const path = mock('/s/studio/tasks', '&ambient=0');
const entryScript = /\/assets\/app\.js$/;
const document = /\/\?mock=1&ambient=0$/;

// These cases control precisely which startup requests fail. Read the other loopback
// responses from the same static server through Playwright's HTTP client so an actual
// host network change cannot add failures to the injected sequence.
beforeEach(async () => {
  await surface.context().route(/^http:\/\/127\.0\.0\.1:\d+\/(?:\?mock=1|assets\/.*\.js$)/, async (route) => {
    await route.fulfill({ response: await route.fetch({ maxRedirects: 0 }) });
  });
});

afterEach(async () => { await surface.context().unrouteAll({ behavior: 'wait' }); });

// Chromium's interception API can abort a real script load, but cannot select
// ERR_NETWORK_CHANGED as the abort reason. Supply that reason on the failed request.
async function interruptRequests(pattern: RegExp, failures: number, networkChanged: boolean) {
  let requests = 0;
  await surface.context().route(pattern, async (route) => {
    requests += 1;
    if (requests > failures) return route.fallback();
    if (networkChanged) route.request().failure = () => ({ errorText: 'net::ERR_NETWORK_CHANGED' });
    await route.abort('connectionreset');
  });
  return () => requests;
}

test('the initial mock open recovers a network change on its entry script', async ({ app, browser }) => {
  const requests = await interruptRequests(entryScript, 1, true);
  await app.open(path);
  expect(requests()).toBe(2);
  expect(await browser.evaluate(() => Boolean((window as { __yuliusMock?: unknown }).__yuliusMock))).toBe(true);
  expect(await browser.url()).toBe(new URL(path, app.baseUrl).href);
});

test('a network change aborting the initial document reopens the requested URL', async ({ app, browser }) => {
  const requests = await interruptRequests(document, 1, true);
  await app.open(path);
  expect(requests()).toBe(2);
  expect(await browser.url()).toBe(new URL(path, app.baseUrl).href);
});

test('the first open can finish when the host interrupts both it and its recovery reload', async ({ app, browser }) => {
  const requests = await interruptRequests(entryScript, 2, true);
  await app.open(path);
  expect(requests()).toBe(3);
  expect(await browser.url()).toBe(new URL(path, app.baseUrl).href);
  expect(await browser.evaluate(() => Boolean((window as { __yuliusMock?: unknown }).__yuliusMock))).toBe(true);
});

test('two interrupted document loads reopen the original route and options', async ({ app, browser }) => {
  const requests = await interruptRequests(document, 2, true);
  await app.open(path);
  expect(requests()).toBe(3);
  expect(await browser.url()).toBe(new URL(path, app.baseUrl).href);
});

test('a burst interrupting three startup navigations can settle within the first-load budget', async ({ app, browser }) => {
  const requests = await interruptRequests(entryScript, 3, true);
  await app.open(path);
  expect(requests()).toBe(4);
  expect(await browser.url()).toBe(new URL(path, app.baseUrl).href);
});

test('persistent startup interruptions exhaust one recovery budget', async ({ app }) => {
  const requests = await interruptRequests(entryScript, Infinity, true);
  await assert.rejects(app.open(path), /startup budget exhausted.*ERR_NETWORK_CHANGED/);
  assert.ok(requests() >= 3);
});

test('an ordinary script failure still surfaces on the first open', async ({ app }) => {
  const requests = await interruptRequests(entryScript, 1, false);
  await assert.rejects(app.open(path), /__yuliusMock was never installed/);
  expect(requests()).toBe(1);
});

test('an ordinary entry-script failure still surfaces alongside a network change', async ({ app }) => {
  const requests = await interruptRequests(entryScript, 1, false);
  await interruptRequests(/\/assets\/api\.js$/, 1, true);
  await assert.rejects(app.open(path), /__yuliusMock was never installed/);
  expect(requests()).toBe(1);
});

test('a later open and an explicit reload each surface their own interruption', async ({ app, browser }) => {
  await app.open(path);
  const requests = await interruptRequests(document, 2, true);
  await assert.rejects(browser.reload(), /ERR_CONNECTION_RESET/);
  await browser.waitForURL('chrome-error://chromewebdata/');
  // Both failures commit the same URL. Observe the second commit before teardown reads the page.
  const page = surface.page();
  await Promise.all([
    page.waitForEvent('framenavigated', {
      predicate: (frame) => frame === page.mainFrame() && frame.url() === 'chrome-error://chromewebdata/',
      timeout: FIRST_LOAD_MS,
    }),
    assert.rejects(app.open(path), /ERR_CONNECTION_RESET/),
  ]);
  expect(requests()).toBe(2);
});
