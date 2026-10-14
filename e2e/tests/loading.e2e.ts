import { test, type Browser } from '@e2e-dev/web';
import { expect } from 'e2e';
import { axeViolations, failOnPageErrors, mock } from './support.ts';

failOnPageErrors();

// Task #77 (SPEC-092): "Instead of loading we want floating loading fields in all places", at the
// Git page's spinner and "Loading delivery…". Content that loads shows shimmering bars where it
// will appear; what loads is said to assistive technology only.

/** Visible text inside `#main` that says "Loading", ignoring screen-reader-only labels. */
function visibleLoadingText(browser: Browser, root = '#main'): Promise<string[]> {
  return browser.evaluate((selector) => {
    const found: string[] = [];
    const scope = document.querySelector(selector);
    if (!scope) return found;
    const walker = document.createTreeWalker(scope, NodeFilter.SHOW_TEXT);
    for (let node = walker.nextNode(); node; node = walker.nextNode()) {
      const parent = node.parentElement;
      if (!parent || !/Loading/.test(node.textContent ?? '')) continue;
      if (parent.closest('.sr-only') || !parent.checkVisibility()) continue;
      found.push((node.textContent ?? '').trim());
    }
    return found;
  }, root);
}

type HoldingGit = Window & { __yuliusHoldGit?: Promise<void>; __releaseGit?: () => void };

test('the Git page shows shimmering fields, not a spinner, while delivery loads', async ({ app, browser, screen }) => {
  await browser.addInitScript(() => {
    const w = window as unknown as HoldingGit;
    w.__yuliusHoldGit = new Promise<void>((resolve) => { w.__releaseGit = resolve; });
  });
  await app.open(mock('/s/studio/git'));
  await expect(screen.getByRole('heading', { level: 1 })).toHaveText('Git');

  const fields = browser.locator('#main .skeleton[role="status"]');
  await expect(fields).toBeVisible();
  await expect(fields).toHaveText('Loading delivery…');
  expect(await browser.locator('#main .skeleton[role="status"] .skeleton-line').count()).toBeGreaterThan(0);
  expect(await browser.locator('.placeholder-loading').count()).toBe(0);
  expect(await visibleLoadingText(browser)).toEqual([]);
  expect(await axeViolations(browser)).toEqual([]);

  await browser.evaluate(() => (window as unknown as HoldingGit).__releaseGit!());
  await expect(screen.getByRole('heading', /^Where every task is/)).toBeVisible();
  await expect(fields).toHaveCount(0);
});

const release = (browser: Browser) => browser.evaluate(() => (window as unknown as { __yuliusMock: { release?: () => void } }).__yuliusMock.release?.());

test('task lists and the status popover load as shimmering fields', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/tasks', '&hold=/api/sandboxes/sb_studio/tasks'));
  await expect(screen.getByRole('heading', { level: 1 })).toHaveText('Tasks');
  await expect(browser.locator('.task-list .skeleton[role="status"]')).toHaveText('Loading tasks…');
  await expect(browser.locator('#main .skeleton[role="status"]')).toHaveText(/^Loading the tasks of /);
  expect(await visibleLoadingText(browser, 'body')).toEqual([]);
  expect(await axeViolations(browser)).toEqual([]);
  await release(browser);
  await expect(browser.locator('.task-list .task-group').first()).toBeVisible();
  await expect(browser.locator('.skeleton')).toHaveCount(0);

  await app.open(mock('/s/studio/git', '&hold=/api/status'));
  await browser.locator('.btn-status').click();
  const pop = browser.locator('.status-pop');
  await expect(browser.locator('.status-pop .skeleton[role="status"]')).toHaveText('Loading status…');
  expect(await visibleLoadingText(browser, '.status-pop')).toEqual([]);
  await release(browser);
  await expect(browser.locator('.status-pop .skeleton')).toHaveCount(0);
  await expect(pop).toBeVisible();
});
