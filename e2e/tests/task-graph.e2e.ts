import { test } from '@e2e-dev/web';
import { expect } from 'e2e';
import { axeViolations, failOnPageErrors, mock } from './support.ts';

failOnPageErrors();

// The Tasks page's Graph view (SPEC-164): the queue in start order, the chains, start rules and
// supertask holds drawn as a graph, the prepared tasks and the tasks on their own. `tasks=graph`
// seeds studio with that structure (mock.js).

const GRAPH = '&tasks=graph';

type WithMock = { __yuliusMock: { touchTask: (sandbox: string, number: number, changes: unknown) => void } };

/** Each node's box, by task number (JSON-safe). */
const boxes = () => Object.fromEntries([...document.querySelectorAll<HTMLElement>('.tg-node')].map((node) => {
  const r = node.getBoundingClientRect();
  return [node.dataset.number, { left: r.left, right: r.right, top: r.top }];
}));
/** Every drawn link as "from>to:kind". */
const edges = () => [...document.querySelectorAll<SVGPathElement>('.tg-edge')].map((edge) => `${edge.dataset.from}>${edge.dataset.to}:${edge.dataset.kind}`).sort();

test('Starts next lists the queue in start order: the supertask first, then the longest queued (TGR-002)', async ({ app, browser }) => {
  await app.open(mock('/s/studio/tasks?view=graph', GRAPH));
  const items = browser.locator('.tg-queue .tg-queue-item');
  await expect(items).toHaveCount(3);
  await expect(browser.locator('.tg-queue .tg-queue-item:nth-child(1)')).toHaveText(/^1.*#20.*Security hardening.*Supertask/);
  await expect(browser.locator('.tg-queue .tg-queue-item:nth-child(2)')).toHaveText(/^2.*#8.*Nightly feedback digest.*supertask #20/);
  await expect(browser.locator('.tg-queue .tg-queue-item:nth-child(3)')).toHaveText(/^3.*#21.*Translate the docs/);
  await expect(browser.locator('.tg-queue')).toContainText('shared with other projects');
});

test('Chains, rules and supertask holds are drawn left to right, one arrow per link (TGR-003, TGR-004)', async ({ app, browser }) => {
  await app.open(mock('/s/studio/tasks?view=graph', GRAPH));
  await expect(browser.locator('.tg-node[data-number="17"]')).toBeVisible();
  const box = await browser.evaluate(boxes);
  expect(box['15'].left).toBeGreaterThan(box['12'].right);
  expect(box['16'].left).toBeGreaterThan(box['15'].right);
  expect(box['17'].left).toBeGreaterThan(box['16'].right);
  const drawn = await browser.evaluate(edges);
  expect(drawn).toEqual(expect.arrayContaining(['12>15:after', '15>16:after', '12>17:rule', '16>17:rule', '12>20:hold', '6>20:hold', '20>8:hold', '20>21:hold']));
  // The rule's anchor link is not drawn twice.
  expect(drawn).not.toContain('12>17:after');
  // A rule-started task says its rule; a node is a link to the task's overview with its words.
  await expect(browser.locator('.tg-node[data-number="17"]')).toContainText('#12 done and #16 done');
  await expect(browser.locator('.tg-node[data-number="15"]')).toHaveAttribute('href', /\/s\/studio\/tasks\/15$/);
  await expect(browser.locator('.tg-node[data-number="15"]')).toHaveAttribute('aria-label', /^Task #15 Cart totals, .*#12/);
  // The legend names the three kinds of link.
  await expect(browser.locator('.tg-legend')).toHaveText(/Runs after.*Start rule.*Supertask hold/);
  // The archived #1 a rule names is a ghost node; nothing else from the closed past is drawn.
  await expect(browser.locator('.tg-node[data-number="1"]')).toHaveAttribute('class', /tg-ghost/);
  await expect(browser.locator('.tg-node[data-number="5"]')).toHaveCount(0);
  await browser.locator('.tg-node[data-number="15"]').click();
  await expect(browser).toHaveURL(/#\/s\/studio\/tasks\/15$/);
});

test('Prepared and On their own hold the rest; a task appears in one place (TGR-005)', async ({ app, browser }) => {
  await app.open(mock('/s/studio/tasks?view=graph', GRAPH));
  await expect(browser.locator('.tg-prepared .tg-item')).toHaveCount(2);
  await expect(browser.locator('.tg-prepared .tg-item:has-text("#18")')).toContainText('Scheduled');
  await expect(browser.locator('.tg-prepared .tg-item:has-text("#19")')).toContainText('When you decide');
  await expect(browser.locator('.tg-alone .tg-item:has-text("#7")')).toContainText('Paused');
  // #12 is in the graph, #8 in the queue: neither is on its own.
  await expect(browser.locator('.tg-alone .tg-item:has-text("#12 ")')).toHaveCount(0);
  await expect(browser.locator('.tg-alone .tg-item:has-text("#8 ")')).toHaveCount(0);
  expect(await axeViolations(browser)).toEqual([]);
});

test('Without any structure the sections that would be empty are left out (TGR-005)', async ({ app, browser }) => {
  await app.open(mock('/s/sandbox-3/tasks?view=graph'));
  await expect(browser.locator('.tg-alone .tg-item')).toHaveCount(1);
  await expect(browser.locator('.tg-queue, .tg-graph, .tg-prepared')).toHaveCount(0);
});

test('Links forming a cycle still draw every node once (TGR-006)', async ({ app, browser }) => {
  await app.open(mock('/s/studio/tasks?view=graph', `${GRAPH}&graph=cycle`));
  await expect(browser.locator('.tg-node[data-number="23"]')).toHaveCount(1);
  await expect(browser.locator('.tg-node[data-number="24"]')).toHaveCount(1);
  expect(await browser.evaluate(edges)).toEqual(expect.arrayContaining(['23>24:after', '24>23:after']));
});

test('The List / Graph switch keeps the view in the URL; the sidebar opens the graph; live tasks join it (TGR-001, TGR-006)', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/tasks', GRAPH));
  await expect(browser.locator('.tl-row').first()).toBeVisible();
  await screen.getByText('Graph', { exact: true }).click();
  await expect(screen.getByRole('radio', 'Graph')).toBeChecked();
  await expect(browser).toHaveURL(/#\/s\/studio\/tasks\?view=graph$/);
  await expect(browser.locator('.tg-node[data-number="15"]')).toBeVisible();
  await screen.getByText('List', { exact: true }).click();
  await expect(browser).toHaveURL(/#\/s\/studio\/tasks$/);
  await expect(browser.locator('.tg')).toHaveCount(0);

  // From another page, the Working group's button opens the graph.
  await browser.evaluate(() => { location.hash = '#/s/studio/git'; });
  await screen.getByRole('link', 'Show as a graph').click();
  await expect(browser).toHaveURL(/#\/s\/studio\/tasks\?view=graph$/);
  await expect(browser.locator('.tg-node[data-number="15"]')).toBeVisible();

  // #7 is chained after #16 meanwhile (as the server announces it): it joins the graph without a reload.
  await browser.evaluate(() => (window as unknown as WithMock).__yuliusMock.touchTask('sb_studio', 7, { status: 'chained', afterLink: { task_id: 'tsk_studio_16', number: 16, title: 'Receipt emails', status: 'chained', blocked: null, rule: null } }));
  await expect(browser.locator('.tg-node[data-number="7"]')).toBeVisible();
  expect(await browser.evaluate(edges)).toContain('16>7:after');
});

test('On a phone the graph scrolls in its frame, never the page (TGR-006)', async ({ app, browser }) => {
  await browser.setViewport({ width: 390, height: 844 });
  await app.open(mock('/s/studio/tasks?view=graph', GRAPH));
  await expect(browser.locator('.tg-node[data-number="17"]')).toHaveCount(1);
  const sizes = await browser.evaluate(() => {
    const frame = document.querySelector('.tg-frame') as HTMLElement;
    return { page: document.documentElement.scrollWidth, window: innerWidth, frame: frame.scrollWidth > frame.clientWidth };
  });
  expect(sizes.page).toBeLessThanOrEqual(sizes.window);
  expect(sizes.frame).toBe(true);
});
