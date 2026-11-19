import assert from 'node:assert/strict';
import { test } from '@e2e-dev/web';
import { expect } from 'e2e';
import { surface } from '../chromium.ts';
import { failOnPageErrors, mock } from './support.ts';

failOnPageErrors();

const path = mock('/s/studio/tasks', '&ambient=0');
const entryScript = /\/assets\/app\.js$/;
const document = /\/\?mock=1&ambient=0$/;

// Chromium's interception API can abort a real script load, but cannot select
// ERR_NETWORK_CHANGED as the abort reason. Supply that reason on the failed request.
async function interruptRequests(pattern: RegExp, failures: number, networkChanged: boolean) {
  let requests = 0;
  await surface.context().route(pattern, async (route) => {
    requests += 1;
    if (requests > failures) return route.continue();
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

test('a second interruption of the initial load fails after one recovery', async ({ app }) => {
  const requests = await interruptRequests(entryScript, 2, true);
  await assert.rejects(app.open(path));
  expect(requests()).toBe(2);
});

test('an ordinary script failure still surfaces on the first open', async ({ app }) => {
  const requests = await interruptRequests(entryScript, 1, false);
  await assert.rejects(app.open(path), /__yuliusMock was never installed/);
  expect(requests()).toBe(1);
});

test('a later open and an explicit reload each surface their own interruption', async ({ app, browser }) => {
  await app.open(path);
  const requests = await interruptRequests(document, 2, true);
  await assert.rejects(browser.reload(), /ERR_CONNECTION_RESET/);
  await browser.waitForURL('chrome-error://chromewebdata/');
  await assert.rejects(app.open(path), /ERR_CONNECTION_RESET/);
  expect(requests()).toBe(2);
});
