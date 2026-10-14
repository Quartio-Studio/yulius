import { test, type Browser } from '@e2e-dev/web';
import { expect } from 'e2e';
import { axeViolations, failOnPageErrors, mock } from './support.ts';

failOnPageErrors();

// Task #106 (SPEC-120): "Floating loaders should have the shape and layout of the final layout".
// The shimmering fields of SPEC-092 are drawn with the page's own layout, so what loads lands where
// the fields were.

type Box = { x: number; y: number; width: number; height: number };

/** The rectangle of the first element matching `selector`, or null. */
function box(browser: Browser, selector: string): Promise<Box | null> {
  return browser.evaluate((s) => {
    const el = document.querySelector(s);
    if (!el) return null;
    const r = el.getBoundingClientRect();
    return { x: r.x, y: r.y, width: r.width, height: r.height };
  }, selector);
}

/** `actual` sits where `expected` is: the named edges and sizes within 2 px. */
function expectSameBox(actual: Box | null, expected: Box | null, keys: (keyof Box)[] = ['x', 'y', 'width', 'height']): void {
  expect(actual).not.toBeNull();
  expect(expected).not.toBeNull();
  // A miss names the edge and how far off it is.
  const off = (key: keyof Box) => Math.round(actual![key] - expected![key]);
  for (const key of keys) expect({ [key]: Math.abs(off(key)) <= 2 ? 0 : off(key) }).toEqual({ [key]: 0 });
}

/** The skeleton is one status for assistive technology and nothing in it takes focus. */
async function expectQuietShape(browser: Browser, root: string): Promise<void> {
  expect(await browser.evaluate((s) => {
    const shape = document.querySelector(s);
    if (!shape) return 'no skeleton';
    if (shape.getAttribute('role') !== 'status') return 'not a status';
    const drawn = [...shape.children].filter((child) => !child.classList.contains('sr-only'));
    if (drawn.some((child) => child.getAttribute('aria-hidden') !== 'true')) return 'shape not aria-hidden';
    if (shape.querySelector('a, button, input, select, textarea, [tabindex]')) return 'focusable inside';
    return 'ok';
  }, root)).toBe('ok');
}

const release = (browser: Browser) => browser.evaluate(() => (window as unknown as { __yuliusMock: { release: () => void } }).__yuliusMock.release());

test('a page of item cards loads as item cards where the cards will be', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/workflows', '&hold=/api/sandboxes/sb_studio/workflows'));
  await expect(screen.getByRole('heading', { level: 1 })).toHaveText('Workflows');
  const shape = '#main .skeleton[role="status"]';
  await expect(browser.locator(shape)).toHaveText('Loading the workflows…');
  await expectQuietShape(browser, shape);
  // A section heading, then cards with an icon tile and lines of text, as the loaded page.
  await expect(browser.locator(`${shape} .page-section-title`).first()).toBeAttached();
  expect(await browser.locator(`${shape} .item-list .item-card`).count()).toBeGreaterThanOrEqual(2);
  await expect(browser.locator(`${shape} .item-card .item-icon`).first()).toBeVisible();
  const loading = await box(browser, `${shape} .item-card`);
  expect(await axeViolations(browser)).toEqual([]);

  await release(browser);
  await expect(browser.locator(shape)).toHaveCount(0);
  const loaded = await box(browser, '#main .item-card');
  expectSameBox(loading, loaded, ['x', 'width']);
  // The same kind of card: its height within a fifth of a loaded one's.
  expect(Math.abs(loading!.height - loaded!.height)).toBeLessThan(loaded!.height / 5);
});

test('a table loads as a table with its columns', async ({ app, browser, screen }) => {
  await app.open(mock('/admin/users', '&hold=/api/admin/users'));
  await expect(screen.getByRole('heading', { level: 1 })).toHaveText('Users');
  // The recovery card below loads with its own status.
  const shape = '#main .skeleton[role="status"]:has(.table-wrap)';
  await expect(browser.locator(shape)).toHaveText('Loading the users…');
  await expectQuietShape(browser, shape);
  const columns = await browser.locator(`${shape} table thead th`).count();
  const rows = await browser.locator(`${shape} table tbody tr`).count();
  expect(rows).toBeGreaterThanOrEqual(3);
  const loading = await box(browser, `${shape} .table-wrap`);
  expect(await axeViolations(browser)).toEqual([]);

  await release(browser);
  await expect(browser.locator(shape)).toHaveCount(0);
  expect(await browser.locator('#main table thead th').count()).toBe(columns);
  expectSameBox(loading, await box(browser, '#main .table-wrap'), ['x', 'y', 'width']);
});

test('a grid of cards loads as cards, and an editor as its fields', async ({ app, browser, screen }) => {
  await app.open(mock('/admin/accounts', '&hold=/api/admin/accounts'));
  await expect(screen.getByRole('heading', { level: 1 })).toBeVisible();
  const shape = '#main .skeleton[role="status"]';
  await expect(browser.locator(shape)).toHaveText('Loading the subscriptions…');
  await expectQuietShape(browser, shape);
  expect(await browser.locator(`${shape} .grid-2 > .card`).count()).toBeGreaterThanOrEqual(2);
  const loading = await box(browser, `${shape} .grid-2`);
  expect(await axeViolations(browser)).toEqual([]);
  await release(browser);
  await expect(browser.locator(shape)).toHaveCount(0);
  expectSameBox(loading, await box(browser, '#main .grid-2'), ['x', 'y', 'width']);

  // The workflow editor: section headings over field boxes, not three lines of text.
  await app.open(mock('/s/studio/workflows/wf-feature', '&hold=/api/sandboxes/sb_studio/workflows'));
  await expect(browser.locator(shape)).toHaveText('Loading the workflow…');
  await expectQuietShape(browser, shape);
  expect(await browser.locator(`${shape} .skeleton-field`).count()).toBeGreaterThanOrEqual(2);
  expect(await axeViolations(browser)).toEqual([]);
  await release(browser);
  await expect(browser.locator(shape)).toHaveCount(0);
});

type HoldingGit = Window & { __yuliusHoldGit?: Promise<void>; __releaseGit?: () => void };

test('the Git page loads as its status cards, sections and board (the reported screen)', async ({ app, browser, screen }) => {
  await browser.addInitScript(() => {
    const w = window as unknown as HoldingGit;
    w.__yuliusHoldGit = new Promise<void>((resolve) => { w.__releaseGit = resolve; });
  });
  await app.open(mock('/s/studio/git'));
  await expect(screen.getByRole('heading', { level: 1 })).toHaveText('Git');
  const shape = '#main .skeleton[role="status"]';
  await expect(browser.locator(shape)).toHaveText('Loading delivery…');
  await expectQuietShape(browser, shape);
  // Two status cards side by side, then Blocked, the board and Activity with their headings.
  expect(await browser.locator(`${shape} .dl-strip > .dl-card`).count()).toBe(2);
  expect(await browser.locator(`${shape} .dl-section-title`).count()).toBe(3);
  expect(await browser.locator(`${shape} .dl-needs > .dl-need`).count()).toBeGreaterThan(0);
  expect(await browser.locator(`${shape} .dl-col .dl-card-task`).count()).toBeGreaterThan(0);
  expect(await browser.locator(`${shape} .dl-activity > .dl-event`).count()).toBeGreaterThan(0);
  const columns = await browser.locator(`${shape} .dl-board > .dl-col`).count();
  const live = await box(browser, `${shape} .dl-strip > .dl-card:first-child`);
  const waiting = await box(browser, `${shape} .dl-strip > .dl-card:last-child`);
  const boardBox = await box(browser, `${shape} .dl-board`);
  expect(await axeViolations(browser)).toEqual([]);

  await browser.evaluate(() => (window as unknown as HoldingGit).__releaseGit!());
  await expect(screen.getByRole('heading', /^Where every task is/)).toBeVisible();
  await expect(browser.locator(shape)).toHaveCount(0);
  // The board has the profile's columns, and the cards and board land in the outlines' columns.
  expect(await browser.locator('#main .dl-board > .dl-col').count()).toBe(columns);
  expectSameBox(live, await box(browser, '#main .dl-strip > .dl-card:first-child'), ['x', 'width']);
  expectSameBox(waiting, await box(browser, '#main .dl-strip > .dl-card:last-child'), ['x', 'width']);
  expectSameBox(boardBox, await box(browser, '#main .dl-board'), ['x', 'width']);
});

test('the sidebar and Tasks lists load as task rows of the real rows’ height', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/tasks', '&hold=/api/sandboxes/sb_studio/tasks'));
  await expect(screen.getByRole('heading', { level: 1 })).toHaveText('Tasks');
  const side = '.task-list .skeleton[role="status"]';
  const page = '#main .tl .skeleton[role="status"]';
  await expect(browser.locator(side)).toHaveText('Loading tasks…');
  await expect(browser.locator(page)).toHaveText(/^Loading the tasks of /);
  await expectQuietShape(browser, side);
  await expectQuietShape(browser, page);
  // A group band over rows; the real row classes match real tasks only.
  await expect(browser.locator(`${side} .task-group-head`)).toHaveCount(1);
  expect(await browser.locator(`${side} .skeleton-task-row`).count()).toBeGreaterThanOrEqual(3);
  expect(await browser.locator('.task-list .task-row, .task-list .task-group').count()).toBe(0);
  expect(await browser.locator(`${page} .skeleton-tl-row`).count()).toBeGreaterThanOrEqual(3);
  expect(await browser.locator('#main .tl-row').count()).toBe(0);
  const sideRow = await box(browser, `${side} .skeleton-task-row`);
  const sideHead = await box(browser, `${side} .task-group-head`);
  const pageRow = await box(browser, `${page} .skeleton-tl-row`);
  const pageColumns = await box(browser, '#main .tl > .tl-columns');
  expect(await axeViolations(browser)).toEqual([]);

  await release(browser);
  await expect(browser.locator('.task-list .task-row').first()).toBeVisible();
  await expect(browser.locator('#main .tl-row').first()).toBeVisible();
  await expect(browser.locator('.skeleton')).toHaveCount(0);
  // The first group band and its rows sit where the outlines were; rows are as tall as real ones.
  expectSameBox(sideHead, await box(browser, '.task-list .task-group-head'));
  expectSameBox(sideRow, await box(browser, '.task-list .task-row'), ['x', 'width', 'height']);
  expectSameBox(pageColumns, await box(browser, '#main .tl-columns'));
  expectSameBox(pageRow, await box(browser, '#main .tl-row'), ['x', 'width', 'height']);
});
