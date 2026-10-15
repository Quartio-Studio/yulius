import { test } from '@e2e-dev/web';
import { expect } from 'e2e';
import { failOnPageErrors, mock } from './support.ts';

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
