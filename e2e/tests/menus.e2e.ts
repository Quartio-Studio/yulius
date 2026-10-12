import { test } from '@e2e-dev/web';
import { expect } from 'e2e';
import { axeViolations, failOnPageErrors, mock } from './support.ts';

failOnPageErrors();

// Menus (SPEC-038): every drop-down is built by openMenu, so the composer's chips stand for all
// of them. The studio mock has nine live tasks, so "Run after" is a long menu.

test('a menu stays inside a short window and its items scroll', async ({ app, browser, screen }) => {
  await browser.setViewport({ width: 1280, height: 400 });
  await app.open(mock('/s/studio'));
  await expect(screen.getByRole('link', /Checkout API/).first()).toBeVisible();
  await screen.getByRole('button', /Run after: Start now/).click();
  await expect(screen.getByRole('menu')).toBeVisible();
  const box = await browser.evaluate(() => {
    const menu = document.querySelector('.menu')!.getBoundingClientRect();
    const list = document.querySelector('.menu-list')!;
    return { top: menu.top, bottom: menu.bottom, height: innerHeight, scrolls: list.scrollHeight > list.clientHeight };
  });
  expect(box.top).toBeGreaterThanOrEqual(8);
  expect(box.bottom).toBeLessThanOrEqual(box.height - 8);
  expect(box.scrolls).toBe(true);
  // The last item is reachable: ↑ from the filter focuses it and it scrolls into view.
  await screen.getByRole('searchbox', 'Filter').press('ArrowUp');
  await expect(screen.getByRole('menuitemradio', /^When a rule is true/)).toBeFocused();
  const shown = await browser.evaluate(() => {
    const list = document.querySelector('.menu-list')!.getBoundingClientRect();
    const item = (document.activeElement as HTMLElement).getBoundingClientRect();
    return item.top >= list.top - 1 && item.bottom <= list.bottom + 1;
  });
  expect(shown).toBe(true);
});

test('a description is one line, its full text in the tooltip', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio'));
  await screen.getByRole('button', /^Workflow:/).click();
  const auto = screen.getByRole('menuitemradio', /^Auto workflow/);
  await expect(auto).toBeVisible();
  await expect(auto).toHaveAttribute('title', /features get a spec, research and questions do not/);
  const height = await browser.evaluate(() => Math.round(document.querySelector('.menu-item.rich')!.getBoundingClientRect().height));
  expect(height).toBeLessThanOrEqual(44);
});

const openRunAfter = async ({ app, screen }: { app: { open: (url: string) => Promise<unknown> }; screen: any }) => {
  await app.open(mock('/s/studio'));
  await expect(screen.getByRole('link', /Checkout API/).first()).toBeVisible();
  await screen.getByRole('button', /Run after: Start now/).click();
};

test('a long menu opens with a filter; typed words narrow it and Enter picks the first item', async ({ app, browser, screen }) => {
  await openRunAfter({ app, screen });
  const filter = screen.getByRole('searchbox', 'Filter');
  await expect(filter).toBeFocused();
  await filter.fill('CHECKOUT api');
  await expect(screen.getByRole('menuitemradio')).toHaveCount(1);
  await expect(screen.getByRole('menuitemradio', /#12 Checkout API/)).toBeVisible();
  // Headers and separators make no sense between filtered items.
  expect(await browser.evaluate(() => [...document.querySelectorAll('.menu-sep, .menu-header')].some((el) => (el as HTMLElement).offsetParent !== null))).toBe(false);
  expect(await axeViolations(browser)).toEqual([]);
  await filter.press('Enter');
  await expect(screen.getByRole('menu')).toHaveCount(0);
  await expect(screen.getByRole('button', /Run after: After #12/)).toBeVisible();
});

test('a filter with no match says so; Escape clears it, then closes the menu', async ({ app, screen }) => {
  await openRunAfter({ app, screen });
  const filter = screen.getByRole('searchbox', 'Filter');
  await filter.fill('zzzz');
  await expect(screen.getByRole('menuitemradio')).toHaveCount(0);
  await expect(screen.getByText('No matches')).toBeVisible();
  await filter.press('Escape');
  await expect(filter).toHaveValue('');
  await expect(screen.getByRole('menuitemradio', /^Start now/)).toBeVisible();
  await filter.press('Escape');
  await expect(screen.getByRole('menu')).toHaveCount(0);
  await expect(screen.getByRole('button', /Run after: Start now/)).toBeFocused();
});

test('arrows move between the filter and the items; a letter typed on an item filters', async ({ app, screen }) => {
  await openRunAfter({ app, screen });
  const filter = screen.getByRole('searchbox', 'Filter');
  await filter.press('ArrowDown');
  await expect(screen.getByRole('menuitemradio', /^Start now/)).toBeFocused();
  await screen.getByRole('menuitemradio', /^Start now/).press('ArrowUp');
  await expect(filter).toBeFocused();
  await filter.press('ArrowDown');
  await screen.getByRole('menuitemradio', /^Start now/).press('ArrowDown');
  await screen.getByRole('menuitemradio', /#14 /).press('n');
  await expect(filter).toBeFocused();
  await expect(filter).toHaveValue('n');
});

test('a short menu has no filter and jumps by first letter', async ({ app, screen }) => {
  await app.open(mock('/s/studio'));
  await screen.getByRole('button', /^Mode:/).click();
  await expect(screen.getByRole('menu')).toBeVisible();
  await expect(screen.getByRole('searchbox')).toHaveCount(0);
  await screen.getByRole('menuitemradio', /^Auto mode/).press('p');
  await expect(screen.getByRole('menuitemradio', /^Prompt/)).toBeFocused();
});

test('an item matches when its label, description or hint holds every typed word (unit, in the page)', async ({ app, browser }) => {
  await app.open(mock('/s/studio'));
  const result = await browser.evaluate(async () => {
    const url = '/assets/dom.js';
    const { menuMatches } = await import(url);
    const item = { label: '#12 Checkout API', description: 'Starts when done', hint: 'Working' };
    return [
      menuMatches(item, ''),
      menuMatches(item, '  '),
      menuMatches(item, 'checkout'),
      menuMatches(item, 'API  #12'),
      menuMatches(item, 'starts working'),
      menuMatches(item, 'checkout merged'),
      menuMatches({ label: 'Settings' }, 'set'),
      menuMatches({ label: 'Settings' }, 'x'),
      // The tooltip is not searched: "Starts by itself when #N is done" is on every task row.
      menuMatches({ label: '#12 Checkout API', title: 'Starts by itself when #12 is done' }, 'starts'),
    ];
  });
  expect(result).toEqual([true, true, true, true, true, false, true, false, false]);
});

test('"Run after" lists each task on one line under one heading, every live task offered', async ({ app, browser, screen }) => {
  // `tasks=many`: 30 more queued tasks in studio, more than the menu used to show.
  await app.open(mock('/s/studio', '&tasks=many'));
  await expect(screen.getByRole('link', /Checkout API/).first()).toBeVisible();
  await screen.getByRole('button', /Run after: Start now/).click();
  await expect(screen.getByText('After a task is done')).toBeVisible();
  const row = screen.getByRole('menuitemradio', /^#12 Checkout API/);
  await expect(row).toHaveAttribute('title', 'Starts by itself when #12 is done');
  const rows = await browser.evaluate(() => [...document.querySelectorAll('.menu-item')]
    .filter((el) => /^#\d+ /.test(el.textContent || ''))
    .map((el) => ({ height: Math.round(el.getBoundingClientRect().height), rich: el.classList.contains('rich') })));
  expect(rows.length).toBe(39);
  expect(rows.every((r) => !r.rich && r.height <= 32)).toBe(true);
  await row.click();
  await expect(screen.getByRole('button', /Run after: After #12/)).toBeVisible();
});

test('an open menu without a filter text has no WCAG violations; Tab leaves it, Enter on no match keeps it', async ({ app, browser, screen }) => {
  await openRunAfter({ app, screen });
  const filter = screen.getByRole('searchbox', 'Filter');
  await expect(filter).toBeFocused();
  expect(await axeViolations(browser)).toEqual([]);
  await filter.fill('zzzz');
  await filter.press('Enter');
  await expect(filter).toBeVisible();
  await filter.press('Tab');
  await expect(screen.getByRole('menu')).toHaveCount(0);
  await expect(screen.getByRole('button', /Run after: Start now/)).toBeFocused();
});

test('an Enter that ends an IME composition in the filter picks nothing', async ({ app, browser, screen }) => {
  await openRunAfter({ app, screen });
  await screen.getByRole('searchbox', 'Filter').fill('chec');
  await browser.evaluate(() => {
    document.querySelector('.menu-filter')!.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', isComposing: true, bubbles: true, cancelable: true }));
  });
  await expect(screen.getByRole('menu')).toBeVisible();
  await expect(screen.getByRole('searchbox', 'Filter')).toHaveValue('chec');
  await screen.getByRole('searchbox', 'Filter').press('Escape');
  await screen.getByRole('searchbox', 'Filter').press('Escape');
  await expect(screen.getByRole('button', /Run after: Start now/)).toBeVisible();
});

test('Backspace on an item edits the filter', async ({ app, screen }) => {
  await openRunAfter({ app, screen });
  const filter = screen.getByRole('searchbox', 'Filter');
  await filter.fill('che');
  await filter.press('ArrowDown');
  await screen.getByRole('menuitemradio', /#12 Checkout API/).press('Backspace');
  await expect(filter).toBeFocused();
  await expect(filter).toHaveValue('ch');
});

test('a menu opened above its anchor stays next to it while the filter shortens it', async ({ app, browser, screen }) => {
  await browser.setViewport({ width: 1280, height: 800 });
  await app.open(mock('/s/studio', '&tasks=many'));
  await expect(screen.getByRole('link', /Checkout API/).first()).toBeVisible();
  await screen.getByRole('button', /Run after: Start now/).click();
  const gap = () => browser.evaluate(() => {
    const chip = document.querySelector('.chip-btn[aria-expanded="true"]')!.getBoundingClientRect();
    const menu = document.querySelector('.menu')!.getBoundingClientRect();
    return Math.round(chip.top - menu.bottom);
  });
  expect(await gap()).toBe(4);
  await screen.getByRole('searchbox', 'Filter').fill('checkout api');
  await expect(screen.getByRole('menuitemradio')).toHaveCount(1);
  expect(await gap()).toBe(4);
});

test('a window that only gets shorter (a phone keyboard) keeps the menu open', async ({ app, browser, screen }) => {
  await browser.setViewport({ width: 1280, height: 800 });
  await openRunAfter({ app, screen });
  await browser.evaluate(() => { (window as unknown as { __resized: boolean }).__resized = false; addEventListener('resize', () => { (window as unknown as { __resized: boolean }).__resized = true; }); });
  await browser.setViewport({ width: 1280, height: 500 });
  await expect.poll(() => browser.evaluate(() => (window as unknown as { __resized: boolean }).__resized)).toBe(true);
  await expect(screen.getByRole('searchbox', 'Filter')).toBeVisible();
  await browser.setViewport({ width: 1000, height: 500 });
  await expect(screen.getByRole('menu')).toHaveCount(0);
});

test('on a touch screen a long menu focuses its first item, not the filter', async ({ app, browser, screen }) => {
  await browser.addInitScript(() => {
    const original = window.matchMedia.bind(window);
    window.matchMedia = (query: string) => (query === '(pointer: coarse)' ? ({ ...original(query), matches: true, media: query }) as MediaQueryList : original(query));
  });
  await openRunAfter({ app, screen });
  await expect(screen.getByRole('searchbox', 'Filter')).toBeVisible();
  await expect(screen.getByRole('menuitemradio', /^Start now/)).toBeFocused();
});

test('a focused item shows its whole description', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio'));
  await screen.getByRole('button', /^Workflow:/).click();
  await screen.getByRole('menuitemradio', /^Auto workflow/).focus();
  await browser.keyboard.press('ArrowDown');
  await browser.keyboard.press('ArrowUp');
  await expect(screen.getByRole('menuitemradio', /^Auto workflow/)).toBeFocused();
  const lines = await browser.evaluate(() => {
    const desc = document.activeElement!.querySelector('.menu-desc')!;
    return Math.round(desc.getBoundingClientRect().height / parseFloat(getComputedStyle(desc).lineHeight));
  });
  expect(lines).toBeGreaterThan(1);
});

test('an anchor taller than the window still gets a usable menu (unit, in the page)', async ({ app, browser }) => {
  await app.open(mock('/s/studio'));
  const box = await browser.evaluate(async () => {
    const url = '/assets/dom.js';
    const { openMenu, closeMenu } = await import(url);
    const anchor = document.createElement('div');
    anchor.style.cssText = 'position: fixed; left: 40px; top: -100px; width: 100px; height: 3000px';
    document.body.append(anchor);
    const menu = openMenu(anchor, [{ label: 'One' }, { label: 'Two' }, { label: 'Three' }]);
    const rect = menu.getBoundingClientRect();
    const result = { top: rect.top, bottom: rect.bottom, height: rect.height, window: innerHeight };
    closeMenu();
    anchor.remove();
    return result;
  });
  expect(box.top).toBeGreaterThanOrEqual(8);
  expect(box.bottom).toBeLessThanOrEqual(box.window - 8);
  expect(box.height).toBeGreaterThan(80);
});

test('the Markdown slash menu filters too', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio'));
  await browser.evaluate(async () => {
    const url = '/assets/mdeditor.js';
    const { createMarkdownEditor } = await import(url);
    const editor = createMarkdownEditor({ markdown: '' });
    (window as unknown as { __md: unknown }).__md = editor;
    document.body.prepend(editor.el);
    editor.focus();
  });
  await browser.keyboard.type('/');
  const filter = screen.getByRole('searchbox', 'Filter');
  await expect(filter).toBeFocused();
  await filter.fill('code');
  await expect(screen.getByRole('menuitem')).toHaveCount(1);
  await filter.press('Enter');
  expect(await browser.evaluate(() => (document.querySelector('.md-block') as HTMLElement).dataset.type)).toBe('code');
});
