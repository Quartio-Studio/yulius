import { test, type Browser } from '@e2e-dev/web';
import { expect } from 'e2e';
import { failOnPageErrors, mock } from './support.ts';

failOnPageErrors();

/**
 * Text that runs out of the box drawn around it: for each visible text node, the nearest ancestor
 * that draws a box (a border on both sides or a background) must hold the text's line boxes
 * horizontally. Text an ancestor clips or scrolls first (an ellipsis, a scrolling list) is
 * intended and does not count. One line per escaping text.
 */
function textOutside(browser: Browser) {
  return browser.evaluate(() => {
    const out: string[] = [];
    const root = document.querySelector('.app') || document.body;
    const walker = document.createTreeWalker(root, NodeFilter.SHOW_TEXT);
    const draws = (s: CSSStyleDeclaration) => (parseFloat(s.borderLeftWidth) > 0 && parseFloat(s.borderRightWidth) > 0)
      || (s.backgroundColor !== 'rgba(0, 0, 0, 0)' && s.backgroundColor !== 'transparent');
    for (let node = walker.nextNode(); node; node = walker.nextNode()) {
      const text = (node.textContent || '').trim();
      const el = node.parentElement;
      if (!text || !el || !el.checkVisibility({ visibilityProperty: true })) continue;
      const range = document.createRange();
      range.selectNodeContents(node);
      const rects = [...range.getClientRects()].filter((r) => r.width > 0);
      if (!rects.length) continue;
      const left = Math.min(...rects.map((r) => r.left));
      const right = Math.max(...rects.map((r) => r.right));
      for (let p: Element | null = el; p && p !== document.body; p = p.parentElement) {
        const s = getComputedStyle(p);
        if (s.overflowX !== 'visible') break;
        if (draws(s)) {
          const b = p.getBoundingClientRect();
          const inner = { left: b.left + parseFloat(s.borderLeftWidth), right: b.right - parseFloat(s.borderRightWidth) };
          if (left < inner.left - 1 || right > inner.right + 1) {
            out.push(`${p.tagName.toLowerCase()}.${[...p.classList].join('.')} > "${text.slice(0, 40)}" ${Math.round(Math.max(inner.left - left, right - inner.right))}px`);
          }
          break;
        }
      }
    }
    // A box that clips or scrolls holds its text on purpose, except the page sideways.
    const page = document.querySelector('#main .page');
    if (page && page.scrollWidth > page.clientWidth + 1) out.push(`the page scrolls sideways by ${page.scrollWidth - page.clientWidth}px`);
    return out;
  });
}

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

// SPEC-449 TXT-003: the pages of responsive.e2e.ts, at a wide window, a tablet and a phone.
const pages = [
  ['git', '/s/studio/git', 'Git'],
  ['tasks', '/s/studio/tasks', 'Tasks'],
  ['home', '/', 'Sandboxes'],
  ['task', '/s/studio/t/12', null],
  ['task overview', '/s/studio/tasks/12', '#12 Checkout API'],
  ['dashboard', '/s/studio/dashboard', 'Dashboard'],
  ['sessions', '/s/studio/sessions', 'Sessions'],
  ['CI', '/s/studio/ci', 'CI/CD'],
  ['CD', '/s/studio/ci?section=cd', 'CI/CD'],
  ['workflows', '/s/studio/workflows', 'Workflows'],
  ['skills', '/s/studio/skills', 'Skills'],
  ['automations', '/s/studio/automations', 'Automations'],
  ['automation editor', '/s/studio/automations/aut_1', 'Edit automation'],
  ['MCP', '/s/studio/mcp', 'MCP'],
  ['secrets', '/s/studio/secrets', 'Secrets'],
  ['pipeline', '/s/studio/dashboard?section=pipeline', 'Dashboard'],
  ['settings', '/settings', 'Settings'],
] as const;

for (const [name, path, heading] of pages) {
  test(`the ${name} page keeps long text inside its boxes at 1440, 768 and 390 px`, async ({ app, browser, screen }) => {
    await app.open(mock(path));
    if (heading) await expect(screen.getByRole('heading', { level: 1 })).toHaveText(heading);
    else await expect(browser.locator('.task-row:has-text("Checkout API")').first()).toBeVisible();
    await giveLongText(browser);
    // A sandbox's pages list its tasks: the long title is shown before the text is measured.
    if (path.startsWith('/s/')) await expect(browser.locator('.task-row:has-text("PaymentIntentConfirmationDialogContainer")').first()).toBeAttached();
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
    range.selectNodeContents(el);
    const text = range.getBoundingClientRect();
    return { inside: text.right <= card.right + 1, wraps: text.height > 2 * icon.height, iconOnFirstLine: icon.top - text.top < icon.height };
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
  await splitter.press('Home');
  await expect(splitter).toHaveAttribute('aria-valuenow', '200');
  await expect.poll(() => textOutside(browser)).toEqual([]);
  await expect(hint).toBeHidden();
  await expect(browser.locator('.btn-newtask')).toHaveAttribute('title', /^New task \(.+\)$/);
  await splitter.press('End');
  await expect(hint).toBeVisible();
  await expect.poll(() => textOutside(browser)).toEqual([]);
});
