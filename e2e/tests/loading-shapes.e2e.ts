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
  const shape = '#main .skeleton[role="status"]';
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
