import { test, type Browser } from '@e2e-dev/web';
import { expect } from 'e2e';
import { failOnPageErrors, mock } from './support.ts';

failOnPageErrors();

/**
 * Header, toolbar and list controls whose boxes intersect (each box clipped to its scrolling or
 * clipping ancestors, so text cut by an ellipsis does not count), and whether the page scrolls
 * sideways. One line per overlapping pair.
 */
function layoutProblems(browser: Browser) {
  return browser.evaluate(() => {
    const main = document.querySelector('.app')!;
    const page = main.querySelector('#main .page');
    const selector = [
      '.page-head > *', '.page-actions > *', '.toolbar > *', '.tl-toolbar button', '.tl-summary', '.ss-toolbar > *',
      '.tl-columns > span', '.tl-row > *', '.tl-link > *', '.item-title > *', '.item-controls > *',
      '.home-card-head > *', '.home-count', '.home-card-actions > *',
      '.dl-need > *', '.dl-need .btn', '.dl-env > *', '.dl-strip > *', '.dl-board > *', '.dl-col-head > *',
      '.wf-head .gt-title-row > *', '.stepstrip-main > *', '.stepstrip-toggle > *', '.step-label > *', '.panel-head > *', '.panel-head .row > *',
    ].join(', ');
    const describe = (el: Element) => `${el.tagName.toLowerCase()}.${[...el.classList].slice(0, 2).join('.')} "${(el.getAttribute('aria-label') || el.textContent || '').trim().slice(0, 24)}"`;
    const box = (el: Element) => {
      const r = el.getBoundingClientRect();
      let { left, right, top, bottom } = r;
      for (let p = el.parentElement; p && p !== document.body; p = p.parentElement) {
        const style = getComputedStyle(p);
        if (style.overflowX === 'visible' && style.overflowY === 'visible') continue;
        const q = p.getBoundingClientRect();
        left = Math.max(left, q.left); right = Math.min(right, q.right);
        top = Math.max(top, q.top); bottom = Math.min(bottom, q.bottom);
      }
      return { left, right, top, bottom };
    };
    const items = [...main.querySelectorAll(selector)]
      .filter((el) => !el.closest('[hidden]') && getComputedStyle(el).visibility !== 'hidden')
      .map((el) => ({ el, b: box(el) }))
      .filter(({ b }) => b.right - b.left > 1 && b.bottom - b.top > 1);
    const overlaps: string[] = [];
    for (let i = 0; i < items.length; i += 1) {
      for (let j = i + 1; j < items.length; j += 1) {
        const a = items[i];
        const b = items[j];
        if (a.el.contains(b.el) || b.el.contains(a.el)) continue;
        const w = Math.min(a.b.right, b.b.right) - Math.max(a.b.left, b.b.left);
        const h = Math.min(a.b.bottom, b.b.bottom) - Math.max(a.b.top, b.b.top);
        if (w > 1 && h > 1) overlaps.push(`${describe(a.el)} × ${describe(b.el)}`);
      }
    }
    return {
      overlaps,
      sideways: Boolean(page && page.scrollWidth > page.clientWidth + 1) || document.documentElement.scrollWidth > window.innerWidth,
    };
  });
}

// SPEC-001 UI-046 (F 2026-10-10): "when it's smaller buttons are overlapping".
const pages = [
  ['home', '/', 'Sandboxes'],
  ['tasks', '/s/studio/tasks', 'Tasks'],
  // The task overview: its figures and its sessions fit a 390 px phone (#147).
  ['task overview', '/s/studio/tasks/12', '#12 Checkout API'],
  ['sessions', '/s/studio/sessions', 'Sessions'],
  ['git', '/s/studio/git', 'Git'],
  ['CI', '/s/studio/ci', 'CI/CD'],
  ['CD', '/s/studio/ci?section=cd', 'CI/CD'],
  ['workflows', '/s/studio/workflows', 'Workflows'],
  ['skills', '/s/studio/skills', 'Skills'],
  ['automations', '/s/studio/automations', 'Automations'],
  // The editor's action bar with its start chip wraps on a phone (SPEC-093 AUT-024).
  ['automation editor', '/s/studio/automations/aut_1', 'Edit automation'],
  ['MCP', '/s/studio/mcp', 'MCP'],
  ['secrets', '/s/studio/secrets', 'Secrets'],
  ['dashboard', '/s/studio/dashboard', 'Dashboard'],
  ['pipeline', '/s/studio/pipeline', 'Pipeline'],
  ['pipeline timeline', '/s/studio/pipeline?open=ci_run:ci_s2', 'Pipeline'],
  ['settings', '/settings', 'Settings'],
] as const;

for (const [name, path, heading] of pages) {
  test(`the ${name} page has no overlapping controls and no sideways scroll at 768 and 390 px`, async ({ app, browser, screen }) => {
    await browser.setViewport({ width: 768, height: 900 });
    await app.open(mock(path));
    await expect(screen.getByRole('heading', { level: 1 })).toHaveText(heading);
    await expect.poll(() => layoutProblems(browser)).toEqual({ overlaps: [], sideways: false });

    await browser.setViewport({ width: 390, height: 844 });
    await expect.poll(() => layoutProblems(browser)).toEqual({ overlaps: [], sideways: false });
  });
}

// The reported screenshot: Tasks with a tool panel open. The state tabs ran past the page and an
// origin chip covered the Status column; the tabs now wrap and a narrow list stacks its columns.
test('the Tasks page with a tool panel open wraps its state tabs instead of overlapping', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/tasks'));
  await screen.getByRole('button', /^Show tools/).click();
  await screen.getByRole('button', 'Shell').click();
  await expect(screen.getByRole('region', 'Tool panel')).toBeVisible();
  await expect.poll(() => layoutProblems(browser)).toEqual({ overlaps: [], sideways: false });
  await expect(screen.getByRole('radio', /^Closed/)).toBeVisible();
});

// Task #100 (SPEC-115): the Git page beside a tool panel kept four board columns and two summary
// cards side by side, because they gave way to the window's width, not the page's. A column a few
// letters wide broke task titles letter by letter. Each column and card now keeps room for words.
const gitRoom = (browser: Browser) => browser.evaluate(() => {
  const widths = [...document.querySelectorAll('#main .dl-col, #main .dl-strip > .dl-card')]
    .map((el) => Math.round(el.getBoundingClientRect().width));
  return { count: widths.length, narrowest: widths.length ? Math.min(...widths) : 0 };
});

test('the Git page beside a tool panel keeps its board columns and summary cards wide enough to read', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/git'));
  await expect(screen.getByRole('heading', { level: 1 })).toHaveText('Git');
  // Without a panel a wide window still shows the board's columns side by side, in one row.
  await expect.poll(() => browser.evaluate(() => new Set([...document.querySelectorAll('#main .dl-col')]
    .map((el) => Math.round(el.getBoundingClientRect().top))).size)).toBe(1);
  await expect.poll(async () => (await gitRoom(browser)).count).toBeGreaterThan(2);
  await screen.getByRole('button', /^Show tools/).click();
  for (const tool of ['Editor', 'Shell']) {
    await screen.getByRole('navigation', 'Tools').getByRole('button', new RegExp(`^${tool}`)).click();
    await expect(screen.getByRole('region', 'Tool panel')).toBeVisible();
    for (const width of [1920, 1440, 1024]) {
      await browser.setViewport({ width, height: 900 });
      await expect.poll(() => layoutProblems(browser)).toEqual({ overlaps: [], sideways: false });
      await expect.poll(async () => (await gitRoom(browser)).narrowest).toBeGreaterThanOrEqual(180);
      expect((await gitRoom(browser)).count).toBeGreaterThan(2);
    }
  }
});

// A task beside a tool panel: the step strip's parts gave way to nothing, so "Step 3/7" ran under the
// progress bar; the panel's head at its narrowest kept its title.
test('a task with a tool panel open keeps its step strip and the panel head apart', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/t/12'));
  await expect(screen.getByRole('heading', { level: 1 })).toHaveText('#12 Checkout API');
  await screen.getByRole('button', /^Show tools/).click();
  for (const tool of ['Editor', 'Browser', 'Shell']) {
    await screen.getByRole('navigation', 'Tools').getByRole('button', new RegExp(`^${tool}`)).click();
    await expect(screen.getByRole('region', 'Tool panel')).toBeVisible();
    await expect.poll(() => layoutProblems(browser)).toEqual({ overlaps: [], sideways: false });
  }
  await browser.setViewport({ width: 1024, height: 768 });
  await expect.poll(() => layoutProblems(browser)).toEqual({ overlaps: [], sideways: false });
});

// Task #68 (SPEC-073, #341): a long state ("Working · 1 background task still running · …") kept
// its width and left the strip's label too little room, so "Step 7/7" ran under the progress bar.
// The toggle clips its label, so the overlap check alone cannot see a count pushed out of it: the
// count must also end inside the toggle. A short state stays whole while the step name has room.
const stripRoom = (browser: Browser) => browser.evaluate(() => {
  const toggle = document.querySelector('.taskview .stepstrip-toggle')!;
  const count = document.querySelector('.taskview .step-count');
  const state = document.querySelector('.taskview .strip-state');
  const inner = toggle.getBoundingClientRect().right - parseFloat(getComputedStyle(toggle).paddingRight);
  return {
    countInside: Boolean(count && count.getBoundingClientRect().right <= inner + 0.5),
    stateWhole: Boolean(state && state.scrollWidth <= state.clientWidth),
  };
});

test('a task with a long state keeps its step count clear of the progress bar', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/t/14'));
  await expect(screen.getByRole('heading', { level: 1 })).toHaveText('#14 Start the preview server');
  await expect(browser.locator('.taskview .strip-status')).toHaveText(/background task still running/);
  for (const width of [1440, 1024, 768]) {
    await browser.setViewport({ width, height: 900 });
    await expect.poll(() => layoutProblems(browser)).toEqual({ overlaps: [], sideways: false });
    await expect.poll(async () => (await stripRoom(browser)).countInside).toBe(true);
  }
  await browser.setViewport({ width: 1440, height: 900 });
  await screen.getByRole('button', /^Show tools/).click();
  await screen.getByRole('navigation', 'Tools').getByRole('button', /^Shell/).click();
  await expect(screen.getByRole('region', 'Tool panel')).toBeVisible();
  await expect.poll(() => layoutProblems(browser)).toEqual({ overlaps: [], sideways: false });
  await expect.poll(async () => (await stripRoom(browser)).countInside).toBe(true);
});

// Beside a tool panel on a laptop-sized window the strip is 421-560 px: the bar, the state and the
// pull request chip left the label less than its count, even with a short "Done · PR #520".
test('a task beside a tool panel keeps room for its step count in a narrow strip', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/t/13'));
  await expect(screen.getByRole('heading', { level: 1 })).toHaveText(/^#13 /);
  await screen.getByRole('button', /^Show tools/).click();
  await screen.getByRole('navigation', 'Tools').getByRole('button', /^Shell/).click();
  await expect(screen.getByRole('region', 'Tool panel')).toBeVisible();
  const narrow = new Set<number>();
  for (const task of [13, 14, 7]) {
    await app.open(mock(`/s/studio/t/${task}`));
    await expect(screen.getByRole('heading', { level: 1 })).toHaveText(new RegExp(`^#${task} `));
    await expect(screen.getByRole('region', 'Tool panel')).toBeVisible();
    for (let width = 1300; width <= 1440; width += 20) {
      await browser.setViewport({ width, height: 900 });
      await expect.poll(() => layoutProblems(browser)).toEqual({ overlaps: [], sideways: false });
      await expect.poll(async () => (await stripRoom(browser)).countInside).toBe(true);
      const strip = await browser.evaluate(() => document.querySelector('.taskview .stepstrip')!.getBoundingClientRect().width);
      if (strip > 420 && strip <= 500) narrow.add(width);
    }
  }
  // The sweep reached the band where the count used to run out of room.
  expect(narrow.size).toBeGreaterThan(0);
});

test('a task with a short state keeps it whole while the step name gives way', async ({ app, browser, screen }) => {
  await browser.setViewport({ width: 768, height: 900 });
  await app.open(mock('/s/studio/t/12'));
  await expect(screen.getByRole('heading', { level: 1 })).toHaveText('#12 Checkout API');
  await expect.poll(() => stripRoom(browser)).toEqual({ countInside: true, stateWhole: true });
});

// SPEC-001 UI-047 (F 2026-10-10): "here is some space at the right".
test('every top-level page is centred, with no empty band on the right', async ({ app, browser, screen }) => {
  await browser.setViewport({ width: 1920, height: 1000 });
  // The content's margins inside the page's scrollport (its scrollbar excluded) differ by 2 px at most.
  const centred = () => browser.evaluate(() => {
    const page = document.querySelector('#main .page')!;
    const inner = page.querySelector('.page-inner')!.getBoundingClientRect();
    const start = page.getBoundingClientRect().left + page.clientLeft;
    return Math.abs((inner.left - start) - (start + page.clientWidth - inner.right)) <= 2;
  });
  for (const [, path, heading] of pages) {
    await app.open(mock(path));
    await expect(screen.getByRole('heading', { level: 1 })).toHaveText(heading);
    await expect.poll(centred).toBe(true);
  }
  // A builder page (its heading is for screen readers only).
  await app.open(mock('/s/studio/workflows/wf-feature'));
  await expect.poll(() => browser.evaluate(() => Boolean(document.querySelector('#main .wf-page')))).toBe(true);
  await expect.poll(centred).toBe(true);
});

// Task #104 (SPEC-119, #442): squeezed, the new-task composer's model, effort and Start moved to a
// second row as one block pushed to the right, its left half empty, and at its narrowest Start
// left the box. Squeezed, the last row now starts where the first does and ends with Start.
const composerControls = (browser: Browser) => browser.evaluate(() => {
  const box = document.querySelector('.composer-box')!.getBoundingClientRect();
  const controls = [...document.querySelectorAll('.composer-tools .chip-btn, .composer-tools .composer-preview-toggle, .composer-send')]
    .map((el) => ({ el, r: el.getBoundingClientRect() }));
  const send = document.querySelector('.composer-send')!.getBoundingClientRect();
  const rows = [...new Set(controls.map(({ r }) => Math.round(r.top + r.height / 2)))].sort((a, b) => a - b);
  const rowOf = (r: DOMRect) => rows.findIndex((y) => Math.abs(y - (r.top + r.height / 2)) <= 4);
  const left = (row: number) => Math.min(...controls.filter(({ r }) => rowOf(r) === row).map(({ r }) => r.left));
  const outside = controls.filter(({ r }) => r.left < box.left || r.right > box.right)
    .map(({ el }) => el.getAttribute('aria-label') || el.textContent!.trim());
  let overlaps = 0;
  for (const a of controls) for (const b of controls) {
    if (a === b) continue;
    if (Math.min(a.r.right, b.r.right) - Math.max(a.r.left, b.r.left) > 1 && Math.min(a.r.bottom, b.r.bottom) - Math.max(a.r.top, b.r.top) > 1) overlaps += 1;
  }
  const last = rows.length - 1;
  return {
    rows: new Set(controls.map(({ r }) => rowOf(r))).size,
    outside,
    overlaps,
    // The last row starts at the first row's left edge, and Start ends it, at the right.
    lastRowAligned: Math.abs(left(last) - left(0)) <= 1,
    startEndsLastRow: rowOf(send) === last && controls.every(({ r }) => r.right <= send.right + 0.5),
  };
});

test('the new-task composer squeezed keeps its controls in the box, the last row starting at the left', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio'));
  await expect(screen.getByRole('textbox', 'Task prompt')).toBeVisible();
  // Wide, every control is on one row.
  await expect.poll(async () => (await composerControls(browser)).rows).toBe(1);
  const squeezed = { outside: [], overlaps: 0, lastRowAligned: true, startEndsLastRow: true };
  // A long label (a workflow or a task to run after) truncates and keeps the row whole.
  await browser.evaluate(() => {
    for (const label of document.querySelectorAll('.composer-tools .chip-label')) label.textContent = 'After #123 Migrate the billing service';
  });
  await expect.poll(() => composerControls(browser)).toEqual({ rows: 1, ...squeezed });
  const check = async () => {
    await expect.poll(async () => {
      const { rows: _rows, ...rest } = await composerControls(browser);
      return rest;
    }).toEqual(squeezed);
  };
  for (const width of [900, 768, 600, 390]) {
    await browser.setViewport({ width, height: 900 });
    await expect.poll(async () => (await composerControls(browser)).rows).toBeGreaterThan(1);
    await check();
  }
  // The reported screen: a tool panel beside the composer.
  await browser.setViewport({ width: 1440, height: 900 });
  await screen.getByRole('button', /^Show tools/).click();
  await screen.getByRole('navigation', 'Tools').getByRole('button', /^Shell/).click();
  await expect(screen.getByRole('region', 'Tool panel')).toBeVisible();
  for (const width of [1920, 1440, 1280, 1024]) {
    await browser.setViewport({ width, height: 900 });
    await check();
  }
});
