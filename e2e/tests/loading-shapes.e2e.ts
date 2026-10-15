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

/**
 * The rectangle of the first element matching `selector` in an open dialog, its y measured from
 * the dialog body's top: a centred dialog moves as its content grows.
 */
function dialogBox(browser: Browser, selector: string): Promise<Box | null> {
  return browser.evaluate((s) => {
    const el = document.querySelector(`.modal .modal-body ${s}`);
    if (!el) return null;
    const r = el.getBoundingClientRect();
    const top = el.closest('.modal-body')!.getBoundingClientRect().top;
    return { x: r.x, y: r.y - top, width: r.width, height: r.height };
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

type Holding = { __yuliusMock: { holdPath: string | null; held: Promise<void> | null; release: () => void; released: boolean } };
/** Holds the mock's GET answers for `path` from now on, as `hold=` does from the page load. */
const holdNow = (browser: Browser, path: string) => browser.evaluate((p) => {
  const db = (window as unknown as Holding).__yuliusMock;
  db.holdPath = p;
  db.released = false;
  db.held = new Promise<void>((resolve) => { db.release = () => { db.released = true; resolve(); }; });
}, path);

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
  // The icon tile shimmers like the bars (not the item icon's own still tile).
  expect(await browser.evaluate((s) => getComputedStyle(document.querySelector(`${s} .item-icon`)!).backgroundImage, shape)).toMatch(/^linear-gradient/);
  const loading = await box(browser, `${shape} .item-card`);
  expect(await axeViolations(browser)).toEqual([]);

  await release(browser);
  await expect(browser.locator(shape)).toHaveCount(0);
  const loaded = await box(browser, '#main .item-card');
  expectSameBox(loading, loaded, ['x', 'width']);
  // The same kind of card; its height follows its content (a workflow's steps line), within a fifth.
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
  // The capacity panel above loads with its own status.
  let shape = '#main .skeleton[role="status"]:has(.grid-2)';
  await expect(browser.locator(shape)).toHaveText('Loading the subscriptions…');
  await expectQuietShape(browser, shape);
  expect(await browser.locator(`${shape} .grid-2 > .card`).count()).toBeGreaterThanOrEqual(2);
  // The capacity read is not held: measure once its panel has landed above the cards.
  await expect(browser.locator('#main section[aria-label="Capacity"] .skeleton')).toHaveCount(0);
  const loading = await box(browser, `${shape} .grid-2`);
  expect(await axeViolations(browser)).toEqual([]);
  await release(browser);
  await expect(browser.locator(shape)).toHaveCount(0);
  expectSameBox(loading, await box(browser, '#main .grid-2'), ['x', 'y', 'width']);

  // The workflow editor: its section cards beside the summary, where the loaded ones will be (a
  // custom workflow: a default one adds its read-only banner above them).
  await app.open(mock('/s/studio/workflows/wf_release', '&hold=/api/sandboxes/sb_studio/workflows'));
  shape = '#main .skeleton[role="status"]';
  await expect(browser.locator(shape)).toHaveText('Loading the workflow…');
  await expectQuietShape(browser, shape);
  expect(await browser.locator(`${shape} .wf-section .skeleton-field`).count()).toBeGreaterThanOrEqual(2);
  const section = await box(browser, `${shape} .wf-main > .wf-section`);
  const aside = await box(browser, `${shape} .wf-summary`);
  expect(await axeViolations(browser)).toEqual([]);
  await release(browser);
  await expect(browser.locator(shape)).toHaveCount(0);
  expectSameBox(section, await box(browser, '#main .wf-main > .wf-section'), ['x', 'y', 'width']);
  expectSameBox(aside, await box(browser, '#main .wf-summary'), ['x', 'y', 'width']);
});

test('Sessions, CI and the Dashboard load in their own layouts', async ({ app, browser }) => {
  const shape = '#main .skeleton[role="status"]';
  // Sessions: rows with the kind tile, a title and a meta line.
  await app.open(mock('/s/studio/sessions', '&hold=/api/sandboxes/sb_studio/sessions'));
  const rows = '#main .skeleton[role="status"]:has(.ss-rows)';
  await expect(browser.locator(rows)).toHaveText('Loading sessions…');
  await expectQuietShape(browser, rows);
  const row = await box(browser, `${rows} .ss-row-link`);
  expect(await axeViolations(browser)).toEqual([]);
  await release(browser);
  await expect(browser.locator(rows)).toHaveCount(0);
  expectSameBox(row, await box(browser, '#main .ss-row-link'), ['x', 'y', 'width', 'height']);

  // CI: the configuration card, the Runs heading, then run cards.
  await app.open(mock('/s/studio/ci', '&hold=/api/sandboxes/sb_studio/ci'));
  // The hygiene card below loads with its own status.
  const runs = '#main .skeleton[role="status"]:has(.ci-heading)';
  await expect(browser.locator(runs)).toHaveText('Loading the CI runs…');
  await expectQuietShape(browser, runs);
  await expect(browser.locator(`${runs} .ci-heading`)).toHaveCount(1);
  const config = await box(browser, `${runs} .card`);
  const run = await box(browser, `${runs} .item-card`);
  expect(await axeViolations(browser)).toEqual([]);
  await release(browser);
  await expect(browser.locator(runs)).toHaveCount(0);
  // No banner comes with this read, so the configuration card keeps its top edge too.
  expectSameBox(config, await box(browser, '#main section[aria-label="CI configuration"]'), ['x', 'y', 'width']);
  expectSameBox(run, await box(browser, '#main .item-card'), ['x', 'width']);

  // Dashboard: the six key figures in their strip, then the chart cards.
  await app.open(mock('/s/studio/dashboard', '&hold=/api/analytics/summary'));
  await expect(browser.locator(shape)).toHaveText('Loading the dashboard…');
  await expectQuietShape(browser, shape);
  expect(await browser.locator(`${shape} .db-kpis > .db-kpi`).count()).toBe(6);
  const kpis = await box(browser, `${shape} .db-kpis`);
  const firstKpi = await box(browser, `${shape} .db-kpi`);
  // The Subscriptions card (the capacity is read with the figures) comes before the charts.
  const subscriptionsCard = await box(browser, `${shape} .db-kpis + .db-card`);
  expect(await axeViolations(browser)).toEqual([]);
  await release(browser);
  await expect(browser.locator(shape)).toHaveCount(0);
  expectSameBox(kpis, await box(browser, '#main .db-kpis'), ['x', 'y', 'width']);
  expectSameBox(firstKpi, await box(browser, '#main .db-kpi'), ['x', 'y', 'width']);
  expectSameBox(subscriptionsCard, await box(browser, '#main .db-kpis + .db-card'), ['x', 'y', 'width']);
});

type HoldingGit = Window & { __yuliusHoldGit?: Promise<void>; __releaseGit?: () => void };

test('the Git page loads as its status cards, sections and board (the reported screen)', async ({ app, browser, screen }) => {
  await browser.addInitScript(() => {
    const w = window as unknown as HoldingGit;
    w.__yuliusHoldGit = new Promise<void>((resolve) => { w.__releaseGit = resolve; });
    // The App permission note comes with the read and sits above the cards; dismissed here so the
    // cards' top edges can be compared (a note or banner from the read pushes content down).
    sessionStorage.setItem('yulius.checks-access.dismissed:Quartio-Studio/studio:Commit statuses', '1');
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
  await expect(browser.locator('#main .checks-access')).toHaveCount(0);
  expectSameBox(live, await box(browser, '#main .dl-strip > .dl-card:first-child'), ['x', 'y', 'width']);
  expectSameBox(waiting, await box(browser, '#main .dl-strip > .dl-card:last-child'), ['x', 'y', 'width']);
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
  // Rows start with the task's number (SPEC-122), so the title bars start where the titles will.
  const sideTitle = await box(browser, `${side} .skeleton-task-row .task-title`);
  const pageTitle = await box(browser, `${page} .skeleton-tl-link > .skeleton-bar`);
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
  expectSameBox(sideTitle, await box(browser, '.task-list .task-link:not(:has(.tl-pr-chip)) .task-title'), ['x']);
  expectSameBox(pageTitle, await box(browser, '#main .tl-link .tl-title'), ['x']);
});

test('narrow Tasks rows load as two lines, like the rows they stand for', async ({ app, browser, screen }) => {
  await browser.setViewport({ width: 390, height: 844 });
  await app.open(mock('/s/studio/tasks', '&hold=/api/sandboxes/sb_studio/tasks'));
  await expect(screen.getByRole('heading', { level: 1 })).toHaveText('Tasks');
  const page = '#main .tl .skeleton[role="status"]';
  await expect(browser.locator(page)).toHaveText(/^Loading the tasks of /);
  const row = await box(browser, `${page} .skeleton-tl-row`);
  expect(await axeViolations(browser)).toEqual([]);
  await release(browser);
  await expect(browser.locator('#main .tl-row').first()).toBeVisible();
  // The first group's rows: a title line and the status under it.
  expectSameBox(row, await box(browser, '#main .tl-row'), ['x', 'width', 'height']);
});

test('loading shapes keep still under reduced motion and pass in the light theme', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/workflows', '&hold=/api/sandboxes/sb_studio/workflows'));
  await expect(screen.getByRole('heading', { level: 1 })).toHaveText('Workflows');
  const shape = '#main .skeleton[role="status"]';
  await expect(browser.locator(shape)).toBeVisible();
  // The harness cannot emulate the media query: the reduced-motion rule must cover the bars'
  // pseudo-element and every shimmering box (they all animate through the shared rule).
  expect(await browser.evaluate(() => {
    const selectors: string[] = [];
    const visit = (rules: CSSRuleList, reduced: boolean) => {
      for (const rule of rules) {
        if (rule instanceof CSSImportRule && rule.styleSheet) visit(rule.styleSheet.cssRules, reduced);
        else if (rule instanceof CSSMediaRule) visit(rule.cssRules, reduced || rule.conditionText.includes('prefers-reduced-motion: reduce'));
        else if (reduced && rule instanceof CSSStyleRule && /animation/.test(rule.style.cssText)) selectors.push(rule.selectorText);
      }
    };
    for (const sheet of document.styleSheets) visit(sheet.cssRules, false);
    const parts = selectors.flatMap((s) => s.split(',').map((x) => x.trim()));
    return parts.includes('*') && (parts.includes('*::before') || parts.includes('::before'));
  })).toBe(true);
  expect(await browser.evaluate((s) => getComputedStyle(document.querySelector(`${s} .skeleton-bar`)!, '::before').animationName, shape)).toBe('shimmer');
  await browser.evaluate(() => document.documentElement.setAttribute('data-theme', 'light'));
  expect(await axeViolations(browser)).toEqual([]);
  await release(browser);
  await expect(browser.locator(shape)).toHaveCount(0);
});

// #449: the loading states left as three text bars (SPEC-120 follow-up) load in their final layout.

test('the New sandbox dialog’s repository and template lists load as their rows', async ({ app, browser, screen }) => {
  await app.open(mock('/', '&hold=/api/github/repositories'));
  await screen.getByRole('button', 'Import repository').click();
  const shape = '.modal .card.list > .skeleton[role="status"]';
  const inShape = '.card.list > .skeleton[role="status"]';
  await expect(browser.locator(shape)).toHaveText('Loading repositories…');
  await expectQuietShape(browser, shape);
  expect(await browser.locator(`${shape} .list-item`).count()).toBeGreaterThanOrEqual(3);
  const row = await dialogBox(browser, `${inShape} .list-item`);
  const title = await dialogBox(browser, `${inShape} .list-item .list-main`);
  expect(await axeViolations(browser)).toEqual([]);
  await release(browser);
  await expect(browser.locator(shape)).toHaveCount(0);
  expectSameBox(row, await dialogBox(browser, '.card.list > .list-item'), ['x', 'y', 'width']);
  // The title bars start where the names land, after the radio.
  expectSameBox(title, await dialogBox(browser, '.card.list > .list-item .list-main'), ['x']);

  // The templates, under the playground's fields: rows with a name and a line under it.
  await holdNow(browser, '/api/templates');
  await browser.locator('.modal label:has-text("Playground")').first().click();
  await expect(screen.getByRole('dialog', 'Add sandbox').getByRole('radio', 'Playground')).toBeChecked();
  await expect(browser.locator(shape)).toHaveText('Loading templates…');
  await expectQuietShape(browser, shape);
  expect(await browser.locator(`${shape} .list-item .list-sub`).count()).toBeGreaterThanOrEqual(2);
  const template = await dialogBox(browser, `${inShape} .list-item`);
  const name = await dialogBox(browser, `${inShape} .list-item .list-main`);
  expect(await axeViolations(browser)).toEqual([]);
  await release(browser);
  await expect(browser.locator(shape)).toHaveCount(0);
  expectSameBox(template, await dialogBox(browser, '.card.list > .list-item'), ['x', 'y', 'width']);
  expectSameBox(name, await dialogBox(browser, '.card.list > .list-item .list-main'), ['x']);
});

test('the delivery and CI run dialogs load as their sections', async ({ app, browser, screen }) => {
  // Delivery: the stage track over the Pull request, Task spec and Next cards. The strip reads the
  // delivery when the task opens; the dialog's read is held after that.
  await app.open(mock('/s/studio/t/13'));
  await expect(browser.locator('.taskview .strip-pr')).toHaveText(/^PR #520/);
  await holdNow(browser, '/api/tasks/tsk_studio_13/delivery');
  await browser.locator('.taskview .strip-pr').click();
  let shape = '.modal .modal-body > .skeleton[role="status"]';
  await expect(browser.locator(shape)).toHaveText('Loading delivery…');
  await expectQuietShape(browser, shape);
  expect(await browser.locator(`${shape} .stage-track > .stage`).count()).toBeGreaterThanOrEqual(4);
  expect(await browser.locator(`${shape} .card`).count()).toBe(3);
  const track = await dialogBox(browser, `> .skeleton[role="status"] .stage-track`);
  const card = await dialogBox(browser, `> .skeleton[role="status"] .card`);
  expect(await axeViolations(browser)).toEqual([]);
  await release(browser);
  await expect(browser.locator(shape)).toHaveCount(0);
  expectSameBox(track, await dialogBox(browser, '.stage-track'), ['x', 'y', 'width']);
  // A note that comes with the read can sit above the cards: their columns are compared.
  expectSameBox(card, await dialogBox(browser, '.card'), ['x', 'width']);
  await browser.keyboard.press('Escape');
  await expect(browser.locator('.modal')).toHaveCount(0);

  // A CI run: its head line, the job tabs and the output box (its stages load after the run).
  await app.open(mock('/s/studio/ci?run=ci_s3', '&hold=/api/sandboxes/sb_studio/ci/runs/ci_s3'));
  shape = '.modal .modal-body > .skeleton[role="status"]';
  await expect(browser.locator(shape)).toHaveText('Loading the CI run…');
  await expectQuietShape(browser, shape);
  expect(await browser.locator(`${shape} .ci-tabs > .ci-tab`).count()).toBeGreaterThanOrEqual(2);
  const head = await dialogBox(browser, `> .skeleton[role="status"] .row-wrap`);
  const tabs = await dialogBox(browser, `> .skeleton[role="status"] .ci-tabs`);
  const output = await dialogBox(browser, `> .skeleton[role="status"] .run-output`);
  expect(await axeViolations(browser)).toEqual([]);
  await release(browser);
  await expect(browser.locator(shape)).toHaveCount(0);
  await expect(screen.getByRole('dialog', 'CI run').getByRole('tab', /lint/)).toBeVisible();
  expectSameBox(head, await dialogBox(browser, '.row-wrap'), ['x', 'y', 'width']);
  expectSameBox(tabs, await dialogBox(browser, '.ci-tabs'), ['x', 'width']);
  expectSameBox(output, await dialogBox(browser, '.run-output'), ['x', 'width']);
});
