import { test, type Browser } from '@e2e-dev/web';
import { expect } from 'e2e';
import { failOnPageErrors, FIRST_LOAD_MS, firstShown, mock } from './support.ts';

failOnPageErrors();

type Mocked = { __yuliusMock?: { sandboxes: unknown[] } };

// #446, #455 (SPEC-138 TST-102): the app loads mock.js with a dynamic import, so the page's load
// event can come before the mock exists. A slow mock.js (a loaded machine) made tests that touch
// `window.__yuliusMock` right after `app.open` or `browser.reload` fail with "Cannot read
// properties of undefined". Opening or reloading a mock page now returns once the mock is there.
test('the mock is there as soon as a mock page has opened or reloaded, however slowly it loads', async ({ app, browser }) => {
  // Holds mock.js back like a busy machine would, well past the page's load event.
  await browser.route(/\/assets\/mock\.js(\?|$)/, async (route) => {
    await new Promise((resolve) => setTimeout(resolve, 1500));
    await route.continue();
  });
  const sandboxes = () => browser.evaluate(() => (window as unknown as Mocked).__yuliusMock!.sandboxes.length);

  await app.open(mock('/s/studio'));
  expect(await sandboxes()).toBeGreaterThan(0);

  await browser.reload();
  expect(await sandboxes()).toBeGreaterThan(0);
});

/** Holds mock.js back `ms` on the first load only, like a starved machine. */
async function slowFirstMock(browser: Browser, ms: number): Promise<void> {
  let first = true;
  await browser.route(/\/assets\/mock\.js(\?|$)/, async (route) => {
    if (first) {
      first = false;
      await new Promise((resolve) => setTimeout(resolve, ms));
    }
    await route.continue();
  });
}

// #354 (SPEC-412 FLD-002): on a starved runner the mock's install alone took longer than an
// assertion's 5 s; it is part of the page's first load and waited for with its budget.
test('opening a page waits for a mock that loads later than 5 s', async ({ app, browser }) => {
  await slowFirstMock(browser, 6_000);
  const started = Date.now();
  await app.open(mock('/s/studio'));
  expect(Date.now() - started, 'mock.js was held past an assertion\'s budget').toBeGreaterThan(5_000);
  expect(await browser.evaluate(() => (window as unknown as Mocked).__yuliusMock!.sandboxes.length)).toBeGreaterThan(0);
});

// #354 (SPEC-412 FLD-001, FLD-003): the first look at a page waits for its first load within
// `FIRST_LOAD_MS`, longer than an assertion's 5 s; the steps after it keep their budgets.
test('the first look at a page waits for its first load past an assertion\'s 5 s', async ({ app, browser, screen }) => {
  expect(FIRST_LOAD_MS).toBeGreaterThan(5_000);
  await app.open(mock('/s/studio/tasks', '&hold=/api/sandboxes/sb_studio/tasks'));
  // The Tasks list's read answers 6 s from now, as a starved page's first load does.
  const started = Date.now();
  await browser.evaluate(() => {
    setTimeout(() => (window as unknown as { __yuliusMock: { release: () => void } }).__yuliusMock.release(), 6_000);
  });
  await firstShown(browser.locator('.task-list .task-group').first());
  expect(Date.now() - started, 'the list was held past an assertion\'s budget').toBeGreaterThan(5_000);
  await expect(browser.locator('.skeleton')).toHaveCount(0);
});
