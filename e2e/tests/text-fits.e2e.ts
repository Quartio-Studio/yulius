import { test, type Browser } from '@e2e-dev/web';
import { expect } from 'e2e';
import { failOnPageErrors, mock } from './support.ts';

failOnPageErrors();

/**
 * Text that runs out of the box drawn around it, after two animation frames (redraws that follow
 * a resize have run). A box is an element with a visible border on both sides or a background.
 * Each visible text node must lie inside its nearest box, and each box inside the next box around
 * it, up to the first ancestor that clips or scrolls (an ellipsis, a wide table: intended). The
 * page must not scroll sideways. A short word (at most 12 characters) broken across lines is a
 * box squeezed below a word's width, the price of wrapping anywhere, and counts too. One line per
 * problem.
 */
function textOutside(browser: Browser) {
  return browser.evaluate(() => new Promise<string[]>((done) => requestAnimationFrame(() => requestAnimationFrame(() => {
    const out: string[] = [];
    const root = document.querySelector('.app') || document.body;
    const transparent = (c: string) => c === 'transparent' || /rgba\(.*,\s*0\)$/.test(c);
    const draws = (s: CSSStyleDeclaration) => (parseFloat(s.borderLeftWidth) > 0 && parseFloat(s.borderRightWidth) > 0 && !transparent(s.borderLeftColor))
      || !transparent(s.backgroundColor);
    const name = (el: Element) => `${el.tagName.toLowerCase()}.${[...el.classList].join('.')}`;
    type Box = { left: number; right: number; top: number; bottom: number };
    // The box around `from` (exclusive), or null once something clips or scrolls first.
    const boxAround = (from: Element | null): Element | null => {
      for (let p = from; p && p !== document.body; p = p.parentElement) {
        const s = getComputedStyle(p);
        if (s.overflowX !== 'visible' || s.overflowY !== 'visible') return null;
        if (draws(s)) return p;
      }
      return null;
    };
    // How far `inner` runs out of `box`'s border edge. A glyph's height may exceed a tight
    // line-height by `spill` on each side (half-leading), which is not running out.
    const escapes = (inner: Box, box: Element, spill = 0) => {
      const s = getComputedStyle(box);
      const b = box.getBoundingClientRect();
      const edge = { left: b.left + parseFloat(s.borderLeftWidth), right: b.right - parseFloat(s.borderRightWidth), top: b.top + parseFloat(s.borderTopWidth), bottom: b.bottom - parseFloat(s.borderBottomWidth) };
      return Math.round(Math.max(edge.left - inner.left, inner.right - edge.right, edge.top - inner.top - spill, inner.bottom - edge.bottom - spill));
    };
    const checked = new Set<Element>();
    const walker = document.createTreeWalker(root, NodeFilter.SHOW_TEXT);
    for (let node = walker.nextNode(); node; node = walker.nextNode()) {
      const text = node.textContent || '';
      const el = node.parentElement;
      if (!text.trim() || !el || !el.checkVisibility({ visibilityProperty: true })) continue;
      const range = document.createRange();
      range.selectNodeContents(node);
      const rects = [...range.getClientRects()].filter((r) => r.width > 0);
      if (!rects.length) continue;
      const label = `"${text.trim().slice(0, 40)}"`;
      const inner = { left: Math.min(...rects.map((r) => r.left)), right: Math.max(...rects.map((r) => r.right)), top: Math.min(...rects.map((r) => r.top)), bottom: Math.max(...rects.map((r) => r.bottom)) };
      const spill = Math.max(0, (rects[0].height - (parseFloat(getComputedStyle(el).lineHeight) || rects[0].height)) / 2);
      let box = boxAround(el);
      if (box && escapes(inner, box, spill) > 1) out.push(`${name(box)} > ${label} ${escapes(inner, box, spill)}px`);
      // Each box inside the next one around it.
      while (box && !checked.has(box)) {
        checked.add(box);
        const outer = boxAround(box.parentElement);
        if (outer && escapes(box.getBoundingClientRect(), outer) > 1) out.push(`${name(outer)} > ${name(box)} ${escapes(box.getBoundingClientRect(), outer)}px`);
        box = outer;
      }
      // A short word broken across lines (a hyphen is a break of its own).
      if (getComputedStyle(el).whiteSpace.startsWith('pre')) continue;
      for (const m of text.matchAll(/(?<![^\s-])[^\s-]{2,12}(?![^\s-])/g)) {
        const word = document.createRange();
        word.setStart(node, m.index);
        word.setEnd(node, m.index + m[0].length);
        const tops = new Set([...word.getClientRects()].filter((r) => r.width > 0).map((r) => Math.round(r.top)));
        if (tops.size > 1) out.push(`${name(el)} breaks "${m[0]}" across ${tops.size} lines`);
      }
    }
    const page = document.querySelector('#main .page');
    if (page && page.scrollWidth > page.clientWidth + 1) out.push(`the page scrolls sideways by ${page.scrollWidth - page.clientWidth}px`);
    if (document.documentElement.scrollWidth > window.innerWidth) out.push('the window scrolls sideways');
    done(out);
  }))));
}

/** Waits until the page's loading placeholders are gone, so the sweep reads its content. */
const loaded = (browser: Browser) => expect.poll(() => browser.evaluate(() => document.querySelectorAll('#main .skeleton').length),
  { message: 'the page finished loading' }).toBe(0);

type Touching = Window & { __yuliusMock: { touchTask(sandbox: string, number: number, changes: object): void } };

// The reported screenshot (task #431): a Needs you request with a URL ran past its board card. The
// mock's texts are short words, so each page gets a long URL and a long unbroken word to show.
const URL_ACTION = 'Add TypeSafe API credits (or turn on auto-reload) at https://console.typesafe.example/settings/billing/organization/quartio-studio-production, then tell task #12';
const LONG_TITLE = 'Rename src/components/checkout/PaymentIntentConfirmationDialogContainer.tsx everywhere';

const giveLongText = (browser: Browser) => browser.evaluate(([action, title]) => {
  const m = (window as unknown as Touching).__yuliusMock;
  m.touchTask('sb_studio', 12, { needs_you: { action, problem: 'The TypeSafe API key ran out of credits', decision: false, at: new Date().toISOString() } });
  m.touchTask('sb_studio', 11, { title });
}, [URL_ACTION, LONG_TITLE]);

// SPEC-449 TXT-003: the pages of responsive.e2e.ts and the task view, at a wide window, a tablet
// and a phone. The
// last field is the long text the page shows once it has redrawn (measured only after that); the
// other pages are swept with their own mock content.
const pages = [
  ['git', '/s/studio/git', 'Git', 'console.typesafe'],
  ['tasks', '/s/studio/tasks', 'Tasks', 'PaymentIntentConfirmation'],
  ['home', '/', 'Sandboxes', null],
  ['task', '/s/studio/t/12', null, 'console.typesafe'],
  ['task overview', '/s/studio/tasks/12', '#12 Checkout API', 'console.typesafe'],
  ['dashboard', '/s/studio/dashboard', 'Dashboard', null],
  ['sessions', '/s/studio/sessions', 'Sessions', null],
  ['CI', '/s/studio/ci', 'CI/CD', null],
  ['CD', '/s/studio/ci?section=cd', 'CI/CD', null],
  ['workflows', '/s/studio/workflows', 'Workflows', null],
  ['skills', '/s/studio/skills', 'Skills', null],
  ['automations', '/s/studio/automations', 'Automations', null],
  ['automation editor', '/s/studio/automations/aut_1', 'Edit automation', null],
  ['MCP', '/s/studio/mcp', 'MCP', null],
  ['secrets', '/s/studio/secrets', 'Secrets', null],
  ['pipeline', '/s/studio/dashboard?section=pipeline', 'Dashboard', null],
  ['pipeline timeline', '/s/studio/dashboard?section=pipeline&open=ci_run:ci_s2', 'Dashboard', null],
  ['settings', '/settings', 'Settings', null],
] as const;

for (const [name, path, heading, shows] of pages) {
  test(`the ${name} page keeps long text inside its boxes at 1440, 768 and 390 px`, async ({ app, browser, screen }) => {
    await app.open(mock(path));
    if (heading) await expect(screen.getByRole('heading', { level: 1 })).toHaveText(heading);
    else await expect(browser.locator('.task-row:has-text("Checkout API")').first()).toBeVisible();
    await loaded(browser);
    await giveLongText(browser);
    if (shows) {
      await expect.poll(() => browser.evaluate((text) => (document.querySelector('#main')?.textContent || '').includes(text), shows),
        { message: `the ${name} page shows ${shows}` }).toBe(true);
    }
    for (const width of [1440, 768, 390]) {
      await browser.setViewport({ width, height: 900 });
      await expect.poll(() => textOutside(browser), { message: `text out of its box at ${width} px` }).toEqual([]);
    }
  });
}

// SPEC-449 TXT-002: the board card's label wraps inside the card with its icon beside its first line.
test('a Git board card wraps a long Needs you line inside the card, its icon on the first line', async ({ app, browser, screen }) => {
  await browser.setViewport({ width: 1440, height: 900 });
  await app.open(mock('/s/studio/git'));
  await expect(screen.getByRole('heading', { level: 1 })).toHaveText('Git');
  await giveLongText(browser);
  const line = browser.locator('.dl-card-line:has-text("Needs you: Add TypeSafe API credits")');
  await expect(line).toBeVisible();
  const placed = () => browser.evaluate(() => {
    const el = [...document.querySelectorAll('.dl-card-line')].find((l) => l.textContent?.includes('Needs you: Add TypeSafe'))!;
    const card = el.closest('.dl-card-task')!.getBoundingClientRect();
    const icon = el.querySelector('svg, .icon')!.getBoundingClientRect();
    const range = document.createRange();
    range.selectNodeContents(el.lastChild!);
    const lines = [...range.getClientRects()].filter((r) => r.width > 0);
    const first = lines[0];
    const middle = icon.top + icon.height / 2;
    return {
      inside: Math.max(...lines.map((r) => r.right)) <= card.right + 1,
      wraps: new Set(lines.map((r) => Math.round(r.top))).size > 1,
      iconOnFirstLine: middle > first.top && middle < first.bottom,
    };
  });
  await expect.poll(placed).toEqual({ inside: true, wraps: true, iconOnFirstLine: true });
});

// The sweep's finding: the sidebar is 200 px below 1024 px and can be dragged as narrow anywhere
// (UI-044), and the New task button's shortcut hint ran out of it. It gives way when there is no
// room and shows again when there is; the button's tooltip still names it.
test('the New task button keeps its text inside at every sidebar width', async ({ app, browser, screen }) => {
  await browser.setViewport({ width: 1440, height: 900 });
  await app.open(mock('/s/studio/tasks'));
  await expect(screen.getByRole('heading', { level: 1 })).toHaveText('Tasks');
  const splitter = screen.getByRole('separator', 'Resize sidebar');
  const hint = browser.locator('.btn-newtask .newtask-key');
  await expect(hint).toBeVisible();
  // From the narrowest sidebar up in the splitter's 16 px steps, past where the hint comes back.
  await splitter.press('Home');
  for (let width = 200; width <= 296; width += 16) {
    await expect(splitter).toHaveAttribute('aria-valuenow', String(width));
    await expect.poll(() => textOutside(browser), { message: `text out of its box beside a ${width} px sidebar` }).toEqual([]);
    await splitter.press('ArrowRight');
  }
  await expect(hint).toBeVisible();
  await splitter.press('Home');
  await expect(hint).toBeHidden();
  await expect(browser.locator('.btn-newtask')).toHaveAttribute('title', /^New task \(.+\)$/);
  await splitter.press('End');
  await expect(hint).toBeVisible();
  await expect.poll(() => textOutside(browser)).toEqual([]);
});

// The sweep's finding at 768 px: Done's Archive all ran 40 px out of its group head in the 200 px
// sidebar. A narrow list keeps its icon, and the button keeps its name.
test('Archive all keeps its icon in a narrow sidebar and its name for screen readers', async ({ app, browser, screen }) => {
  await browser.setViewport({ width: 768, height: 900 });
  await app.open(mock('/s/studio/tasks'));
  await expect(screen.getByRole('heading', { level: 1 })).toHaveText('Tasks');
  const archive = browser.locator('.task-group-head.group-done .task-group-action');
  await expect(archive).toBeVisible();
  await expect(screen.getByRole('button', 'Archive all')).toBeVisible();
  const labelShown = () => browser.evaluate(() => getComputedStyle(document.querySelector('.task-group-head.group-done .task-group-action-label')!).position !== 'absolute');
  await expect.poll(labelShown).toBe(false);
  await expect.poll(() => textOutside(browser)).toEqual([]);
  await browser.setViewport({ width: 1440, height: 900 });
  await expect.poll(labelShown).toBe(true);
});
