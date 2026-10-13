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
      '.stepstrip-main > *', '.stepstrip-toggle > *', '.step-label > *', '.panel-head > *', '.panel-head .row > *',
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
  ['CI', '/s/studio/ci', 'CI'],
  ['workflows', '/s/studio/workflows', 'Workflows'],
  ['skills', '/s/studio/skills', 'Skills'],
  ['automations', '/s/studio/automations', 'Automations'],
  ['MCP', '/s/studio/mcp', 'MCP'],
  ['secrets', '/s/studio/secrets', 'Secrets'],
  ['dashboard', '/s/studio/dashboard', 'Dashboard'],
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
test('a task with a long state keeps its step count clear of the progress bar', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/t/14'));
  await expect(screen.getByRole('heading', { level: 1 })).toHaveText('#14 Start the preview server');
  await expect(browser.locator('.taskview .strip-status')).toHaveText(/background task still running/);
  for (const width of [1440, 1024, 768]) {
    await browser.setViewport({ width, height: 900 });
    await expect.poll(() => layoutProblems(browser)).toEqual({ overlaps: [], sideways: false });
  }
  await browser.setViewport({ width: 1440, height: 900 });
  await screen.getByRole('button', /^Show tools/).click();
  await screen.getByRole('navigation', 'Tools').getByRole('button', /^Shell/).click();
  await expect(screen.getByRole('region', 'Tool panel')).toBeVisible();
  await expect.poll(() => layoutProblems(browser)).toEqual({ overlaps: [], sideways: false });
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
