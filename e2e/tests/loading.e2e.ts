import { test, type Browser } from '@e2e-dev/web';
import { expect } from 'e2e';
import { axeViolations, failOnPageErrors, mock } from './support.ts';

failOnPageErrors();

// Task #77 (SPEC-092): "Instead of loading we want floating loading fields in all places", at the
// Git page's spinner and "Loading delivery…". Content that loads shows shimmering bars where it
// will appear; what loads is said to assistive technology only.

/** Visible text inside `root` that says "Loading" or "Reading usage", ignoring screen-reader-only labels. */
function visibleLoadingText(browser: Browser, root = '#main'): Promise<string[]> {
  return browser.evaluate((selector) => {
    const found: string[] = [];
    const scope = document.querySelector(selector);
    if (!scope) return found;
    const walker = document.createTreeWalker(scope, NodeFilter.SHOW_TEXT);
    for (let node = walker.nextNode(); node; node = walker.nextNode()) {
      const parent = node.parentElement;
      if (!parent || !/Loading|Reading usage/.test(node.textContent ?? '')) continue;
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
  // Bars in the page's own layout (SPEC-120; loading-shapes.e2e.ts checks the shape).
  expect(await browser.locator('#main .skeleton[role="status"] .skeleton-bar').count()).toBeGreaterThan(0);
  expect(await browser.locator('.placeholder-loading').count()).toBe(0);
  expect(await visibleLoadingText(browser)).toEqual([]);
  expect(await axeViolations(browser)).toEqual([]);

  await browser.evaluate(() => (window as unknown as HoldingGit).__releaseGit!());
  await expect(screen.getByRole('heading', /^Where every task is/)).toBeVisible();
  await expect(fields).toHaveCount(0);
});

const release = (browser: Browser) => browser.evaluate(() => (window as unknown as { __yuliusMock: { release: () => void } }).__yuliusMock.release());

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
  // A subscription whose usage is not read yet shows a bar where its reset time will be.
  const usageBar = browser.locator('.status-pop .usage-reset .skeleton-line').first();
  await expect(usageBar).toBeVisible();
  // A short bar where the reset time goes (6em of the small text), not most of the row.
  expect(await browser.evaluate(() => document.querySelector('.status-pop .usage-reset .skeleton-line')!.getBoundingClientRect().width)).toBeLessThan(80);
  expect(await visibleLoadingText(browser, '.status-pop')).toEqual([]);
  expect(await axeViolations(browser)).toEqual([]);
});

test('a home card, the editor tree and a notepad load as shimmering fields', async ({ app, browser, screen }) => {
  // A home card's last activity waits for its sandbox's tasks.
  await app.open(mock('/', '&hold=/api/sandboxes/sb_studio/tasks'));
  await expect(screen.getByRole('heading', { level: 1 })).toHaveText('Sandboxes');
  // One bar per card; the grid, not each card, says it is busy (no three status messages at once).
  const activity = browser.locator('.home-card[data-id="sb_studio"] .home-activity .skeleton');
  await expect(activity).toBeVisible();
  await expect(browser.locator('.home-grid[aria-busy="true"]')).toBeVisible();
  expect(await browser.locator('.home-grid [role="status"]').count()).toBe(0);
  expect(await visibleLoadingText(browser)).toEqual([]);
  expect(await axeViolations(browser)).toEqual([]);
  await release(browser);
  await expect(activity).toHaveCount(0);
  await expect(browser.locator('.home-grid[aria-busy="true"]')).toHaveCount(0);

  // The editor's file tree, while a folder is listed.
  await app.open(mock('/s/studio/t/12', '&hold=/api/sandboxes/sb_studio/files'));
  await screen.getByRole('button', /^Show tools/).click();
  await screen.getByRole('button', 'Editor').click();
  const tree = screen.getByRole('tree', 'Files');
  // A tree's children may only be its items: the row's bars are decoration and the tree is busy.
  await expect(browser.locator('.ed-tree[aria-busy="true"] .ed-row-loading .skeleton')).toBeVisible();
  expect(await visibleLoadingText(browser, 'body')).toEqual([]);
  expect(await axeViolations(browser)).toEqual([]);
  await release(browser);
  await expect(tree.getByRole('treeitem', 'README.md')).toBeVisible();
  await expect(browser.locator('.ed-tree .skeleton')).toHaveCount(0);

  // A notepad before its first read: fields over the empty box, no "Loading…" save state.
  await app.open(mock('/s/studio/t/12', '&hold=/api/sandboxes/sb_studio/notes'));
  // The tools rail is remembered from the editor above: hide it to open Notes like a person would.
  await browser.evaluate(() => localStorage.clear());
  await browser.reload();
  await screen.getByRole('button', /^Show tools/).click();
  await screen.getByRole('navigation', 'Tools').getByRole('button', 'Notes').click();
  const pad = browser.locator('.notes-pane:not([hidden]) .skeleton[role="status"]');
  await expect(pad).toHaveText('Loading the notes…');
  expect(await visibleLoadingText(browser, 'body')).toEqual([]);
  expect(await axeViolations(browser)).toEqual([]);
  await release(browser);
  await expect(pad).toHaveCount(0);
  await expect(screen.getByRole('textbox', 'Sandbox notes')).not.toHaveAttribute('readonly', '');
});

test('the browser panel covers a page still loading with shimmering fields', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/t/12', '&hold=frames'));
  await screen.getByRole('button', /^Show tools/).click();
  await screen.getByRole('navigation', 'Tools').getByRole('button', /^Browser/).click();
  const cover = browser.locator('.bp-overlay .skeleton[role="status"]');
  await expect(cover).toHaveText('Loading page…');
  expect(await visibleLoadingText(browser, 'body')).toEqual([]);
  expect(await axeViolations(browser)).toEqual([]);
  await release(browser);
  await expect(browser.locator('.bp-overlay')).toBeHidden();
});

test('a loader names what loads, and the recovery views show fields while they check', async ({ app, browser, screen }) => {
  // loader() (pages/common.js) says what it loads, as its error does.
  await app.open(mock('/s/studio/workflows', '&hold=/api/sandboxes/sb_studio/workflows'));
  await expect(browser.locator('#main .skeleton[role="status"]')).toHaveText('Loading the workflows…');
  await release(browser);
  await expect(browser.locator('#main .skeleton')).toHaveCount(0);

  // The superadmin recovery card on Users.
  await app.open(mock('/admin/users', '&hold=/api/auth/recovery'));
  await expect(screen.getByRole('heading', 'Superadmin recovery code')).toBeVisible();
  await expect(browser.locator('#main .skeleton[role="status"]').filter({ hasText: 'Checking the recovery code…' })).toBeVisible();
  expect(await browser.evaluate(() => /Checking/.test(document.querySelector('#main')?.textContent?.replace(/Checking the recovery code…/g, '') ?? ''))).toBe(false);
  await release(browser);
  await expect(screen.getByText(/A recovery code is set/)).toBeVisible();

  // "Forgot password?" while the recovery state is read.
  await app.open(mock('/signin', '&signedout=1&hold=/api/auth/recovery'));
  await screen.getByText('Superadmin password').click();
  await screen.getByRole('button', 'Forgot password?').click();
  await expect(browser.locator('.skeleton[role="status"]')).toHaveText('Checking the recovery code…');
  expect(await visibleLoadingText(browser, 'body')).toEqual([]);
  expect(await browser.evaluate(() => [...document.querySelectorAll('p')].some((p) => p.checkVisibility() && /^Checking/.test(p.textContent ?? '')))).toBe(false);
  await release(browser);
  await expect(browser.locator('.skeleton')).toHaveCount(0);
});
