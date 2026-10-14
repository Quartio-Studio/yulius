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

/** How a menu item's description shows: its text, whether anything of it is cut or clipped, and its lines. */
const descriptionOf = (browser: any, label: string) => browser.evaluate((label: string) => {
  const item = [...document.querySelectorAll('.menu-item')].find((el) => el.querySelector('.menu-label')!.textContent === label)!;
  const desc = item.querySelector('.menu-desc') as HTMLElement;
  const box = desc.getBoundingClientRect();
  const list = document.querySelector('.menu-list')!.getBoundingClientRect();
  const menu = document.querySelector('.menu')!.getBoundingClientRect();
  return {
    text: desc.textContent,
    title: desc.getAttribute('title'),
    cut: desc.scrollWidth > desc.clientWidth || desc.scrollHeight > desc.clientHeight,
    insideList: box.left >= list.left - 1 && box.right <= list.right + 1 && box.top >= list.top - 1 && box.bottom <= list.bottom + 1,
    lines: Math.round(box.height / parseFloat(getComputedStyle(desc).lineHeight)),
    row: Math.round(item.getBoundingClientRect().height),
    menu: { left: menu.left, right: menu.right, top: menu.top, bottom: menu.bottom, width: menu.width },
    window: { width: innerWidth, height: innerHeight },
  };
}, label);

test('a description wraps onto as many lines as it needs, a short one takes one, none has a tooltip (SPEC-094)', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio'));
  await screen.getByRole('button', /^Workflow:/).click();
  await expect(screen.getByRole('menuitemradio', /^Auto workflow/)).toBeVisible();
  // Every item shows its whole description, not only the focused one.
  await browser.keyboard.press('ArrowDown');
  await expect(screen.getByRole('menuitemradio', /^No workflow/)).toBeFocused();
  const long = await descriptionOf(browser, 'Auto workflow');
  expect(long.text).toBe('Jev picks the workflow that fits, or none: features get a spec, research and questions do not');
  expect(long.cut).toBe(false);
  expect(long.insideList).toBe(true);
  expect(long.lines).toBeGreaterThan(1);
  // Shown whole, it needs no tooltip (a screen reader would read it twice); nor does the item.
  expect(long.title).toBe(null);
  expect(await browser.evaluate(() => document.querySelector('.menu-item.rich')!.getAttribute('title'))).toBe(null);
  // Compact as before (SPEC-038): a short description is one line in a row of at most 44 px, and
  // the menu is no wider than 320 px.
  const short = await descriptionOf(browser, 'No workflow');
  expect(short.cut).toBe(false);
  expect(short.lines).toBe(1);
  expect(short.row).toBeLessThanOrEqual(44);
  expect(short.menu.width).toBeLessThanOrEqual(320);
});

for (const viewport of [{ width: 1280, height: 800 }, { width: 390, height: 844 }]) {
  test(`the start menu shows Supertask's whole description, inside a ${viewport.width} px window (SPEC-094)`, async ({ app, browser, screen }) => {
    await browser.setViewport(viewport);
    await app.open(mock('/s/studio'));
    await expect(screen.getByRole('link', /Checkout API/).first()).toBeAttached();
    await screen.getByRole('button', /Run after: Start now/).click();
    await expect(screen.getByRole('menuitemradio', /^Supertask/)).toBeVisible();
    const supertask = await descriptionOf(browser, 'Supertask');
    expect(supertask.text).toBe('Waits for this project’s running tasks, then runs alone while the project’s other tasks wait. For broad work such as security hardening.');
    expect(supertask.cut).toBe(false);
    expect(supertask.insideList).toBe(true);
    expect(supertask.lines).toBeGreaterThan(1);
    const start = await descriptionOf(browser, 'Start now');
    expect(start.cut).toBe(false);
    // The menu stays inside the window, 8 px from its edges, and no wider than 320 px.
    const { menu, window } = supertask;
    expect(menu.left).toBeGreaterThanOrEqual(8);
    expect(menu.right).toBeLessThanOrEqual(window.width - 8);
    expect(menu.top).toBeGreaterThanOrEqual(8);
    expect(menu.bottom).toBeLessThanOrEqual(window.height - 8);
    expect(menu.width).toBeLessThanOrEqual(320);
  });
}

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
      // A disabled item's reason is its shown second line, so it is searched.
      menuMatches({ label: 'MCP for every sandbox', disabled: true, title: 'Open a sandbox first' }, 'sandbox first'),
    ];
  });
  expect(result).toEqual([true, true, true, true, true, false, true, false, false, true]);
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

test('a window that only gets shorter (a phone keyboard) keeps the menu open, placed again', async ({ app, browser, screen }) => {
  await browser.setViewport({ width: 1280, height: 800 });
  await app.open(mock('/s/studio', '&tasks=many'));
  await expect(screen.getByRole('link', /Checkout API/).first()).toBeVisible();
  await screen.getByRole('button', /Run after: Start now/).click();
  await browser.evaluate(() => { (window as unknown as { __resized: boolean }).__resized = false; addEventListener('resize', () => { (window as unknown as { __resized: boolean }).__resized = true; }); });
  await browser.setViewport({ width: 1280, height: 500 });
  await expect.poll(() => browser.evaluate(() => (window as unknown as { __resized: boolean }).__resized)).toBe(true);
  await expect(screen.getByRole('searchbox', 'Filter')).toBeVisible();
  // Inside the shorter window and still above its chip (scrolled out of the window here), the
  // filter on screen.
  await expect.poll(() => browser.evaluate(() => {
    const menu = document.querySelector('.menu')!.getBoundingClientRect();
    const chip = document.querySelector('.chip-btn[aria-expanded="true"]')!.getBoundingClientRect();
    const filter = document.querySelector('.menu-filter')!.getBoundingClientRect();
    return menu.top >= 8 && menu.bottom <= innerHeight - 8 && menu.bottom <= chip.top - 4 && filter.top >= 8;
  })).toBe(true);
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
  // The page's composer takes the keyboard once it is mounted; the editor goes in after that, or
  // the composer would take it from the slash menu (#322).
  await expect(screen.getByRole('textbox', 'Task prompt')).toBeFocused();
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

test('a menu placed again while filtered still fits once the filter is cleared', async ({ app, browser, screen }) => {
  await browser.setViewport({ width: 1280, height: 800 });
  await app.open(mock('/s/studio', '&tasks=many'));
  await expect(screen.getByRole('link', /Checkout API/).first()).toBeVisible();
  await screen.getByRole('button', /Run after: Start now/).click();
  const filter = screen.getByRole('searchbox', 'Filter');
  await filter.fill('checkout api');
  await expect(screen.getByRole('menuitemradio')).toHaveCount(1);
  await browser.evaluate(() => { (window as unknown as { __resized: boolean }).__resized = false; addEventListener('resize', () => { (window as unknown as { __resized: boolean }).__resized = true; }); });
  await browser.setViewport({ width: 1280, height: 790 });
  await expect.poll(() => browser.evaluate(() => (window as unknown as { __resized: boolean }).__resized)).toBe(true);
  await filter.fill('');
  await expect(screen.getByRole('menuitemradio', /^Start now/)).toBeVisible();
  const box = await browser.evaluate(() => {
    const menu = document.querySelector('.menu')!.getBoundingClientRect();
    return { top: menu.top, bottom: menu.bottom, height: innerHeight };
  });
  expect(box.top).toBeGreaterThanOrEqual(8);
  expect(box.bottom).toBeLessThanOrEqual(box.height - 8);
});

// #288: placing a menu again (a height-only resize, e.g. the address bar hiding) keeps its scroll;
// only editing the filter goes back to the top. The matching tasks scroll in their box (SPEC-102).
test('a filtered menu keeps its scroll position when the window height changes', async ({ app, browser, screen }) => {
  await browser.setViewport({ width: 1280, height: 600 });
  await app.open(mock('/s/studio', '&tasks=many'));
  await expect(screen.getByRole('link', /Checkout API/).first()).toBeVisible();
  await screen.getByRole('button', /Run after: Start now/).click();
  await screen.getByRole('searchbox', 'Filter').fill('queued');
  const scrolled = await browser.evaluate(() => {
    const list = document.querySelector('.menu-scroll')!;
    list.scrollTop = 60;
    return list.scrollTop;
  });
  expect(scrolled).toBeGreaterThan(0);
  await browser.evaluate(() => { (window as unknown as { __resized: boolean }).__resized = false; addEventListener('resize', () => { (window as unknown as { __resized: boolean }).__resized = true; }); });
  await browser.setViewport({ width: 1280, height: 560 });
  await expect.poll(() => browser.evaluate(() => (window as unknown as { __resized: boolean }).__resized)).toBe(true);
  await expect(screen.getByRole('menu')).toBeVisible();
  expect(await browser.evaluate(() => document.querySelector('.menu-scroll')!.scrollTop)).toBe(scrolled);
  // Typing in the filter starts from the top again.
  await screen.getByRole('searchbox', 'Filter').press('End');
  await browser.keyboard.type(' ');
  await expect.poll(() => browser.evaluate(() => document.querySelector('.menu-scroll')!.scrollTop)).toBe(0);
});

// SPEC-102: a menu's task list shows five tasks and says when there is more to scroll.

/** The start menu's task box: rows wholly inside it, whether it scrolls, its fades and its "more" line. */
const taskBox = (browser: any) => browser.evaluate(() => {
  const group = document.querySelector('.menu [role="group"]') as HTMLElement | null;
  if (!group) return null;
  const box = group.querySelector('.menu-scroll') as HTMLElement;
  const rect = box.getBoundingClientRect();
  const rows = [...box.querySelectorAll('.menu-item:not([hidden])')] as HTMLElement[];
  const inside = rows.filter((el) => { const r = el.getBoundingClientRect(); return r.top >= rect.top - 1 && r.bottom <= rect.bottom + 1; });
  const more = group.querySelector('.menu-more') as HTMLElement | null;
  const header = document.getElementById(group.getAttribute('aria-labelledby') || '');
  return {
    name: header ? header.textContent : null,
    rows: rows.length,
    inside: inside.map((el) => el.querySelector('.menu-label')!.textContent),
    scrolls: box.scrollHeight > box.clientHeight,
    above: box.classList.contains('more-above'),
    below: box.classList.contains('more-below'),
    more: more ? { text: (more.textContent || '').trim(), height: Math.round(more.getBoundingClientRect().height), hidden: more.hidden, ariaHidden: more.getAttribute('aria-hidden') } : null,
  };
});

/** Whether the focused element lies wholly inside the task box. */
const focusedInBox = (browser: any) => browser.evaluate(() => {
  const box = document.querySelector('.menu [role="group"] .menu-scroll')!.getBoundingClientRect();
  const item = (document.activeElement as HTMLElement).getBoundingClientRect();
  return item.top >= box.top - 1 && item.bottom <= box.bottom + 1;
});

test('the start menu shows five tasks in a box that scrolls, fading at its bottom, "4 more" under it (SPEC-102)', async ({ app, browser, screen }) => {
  await browser.setViewport({ width: 1280, height: 800 });
  await openRunAfter({ app, screen });
  await expect(screen.getByRole('group', 'After a task is done')).toBeVisible();
  const state = await taskBox(browser);
  expect(state).toMatchObject({ name: 'After a task is done', rows: 9, scrolls: true, above: false, below: true });
  expect(state.inside).toEqual(['#14 Start the preview server', '#13 Improve the code', '#12 Checkout API', '#11 3D model viewer', '#10 AWS login for deploy']);
  expect(state.more).toMatchObject({ text: '4 more', hidden: false, ariaHidden: 'true' });
  // The choices below the tasks stay in view: the whole menu does not scroll at this height.
  const outer = await browser.evaluate(() => {
    const list = document.querySelector('.menu-list')!;
    const rule = [...document.querySelectorAll('.menu-item')].find((el) => /When a rule is true/.test(el.textContent || ''))!.getBoundingClientRect();
    const rect = list.getBoundingClientRect();
    return { scrolls: list.scrollHeight > list.clientHeight, ruleInside: rule.top >= rect.top && rule.bottom <= rect.bottom };
  });
  expect(outer).toEqual({ scrolls: false, ruleInside: true });
  expect(await axeViolations(browser)).toEqual([]);
});

test('scrolling to the end of the tasks fades the top instead and blanks the "more" line (SPEC-102)', async ({ app, browser, screen }) => {
  await browser.setViewport({ width: 1280, height: 800 });
  await app.open(mock('/s/studio', '&tasks=many'));
  await expect(screen.getByRole('link', /Checkout API/).first()).toBeVisible();
  await screen.getByRole('button', /Run after: Start now/).click();
  const top = await taskBox(browser);
  expect(top).toMatchObject({ rows: 39, below: true, above: false });
  expect(top.inside).toHaveLength(5);
  expect(top.more.text).toBe('34 more');
  // Clicking the line scrolls a page down, keeping the filter focused.
  await browser.locator('.menu-more').click();
  await expect.poll(async () => (await taskBox(browser)).above).toBe(true);
  await expect(screen.getByRole('searchbox', 'Filter')).toBeFocused();
  await expect(screen.getByRole('menu')).toBeVisible();
  await browser.evaluate(() => { const box = document.querySelector('.menu-scroll')!; box.scrollTop = box.scrollHeight; });
  await expect.poll(async () => (await taskBox(browser)).below).toBe(false);
  const end = await taskBox(browser);
  expect(end).toMatchObject({ above: true, below: false });
  // Blank but in place, so the menu does not jump.
  expect(end.more).toMatchObject({ text: '', hidden: false });
  expect(end.more.height).toBe(top.more.height);
});

test('a group of five items or fewer shows them all; a longer one shows five whole rows, however tall (unit, in the page)', async ({ app, browser }) => {
  await app.open(mock('/s/studio'));
  const result = await browser.evaluate(async () => {
    const url = '/assets/dom.js';
    const { openMenu, closeMenu, MENU_GROUP_ROWS } = await import(url);
    const anchor = document.createElement('button');
    anchor.style.cssText = 'position: fixed; left: 40px; top: 40px';
    document.body.append(anchor);
    const read = () => {
      const box = document.querySelector('.menu-scroll') as HTMLElement;
      const rect = box.getBoundingClientRect();
      const rows = [...box.querySelectorAll('.menu-item')];
      return {
        scrolls: box.scrollHeight > box.clientHeight,
        inside: rows.filter((el) => { const r = el.getBoundingClientRect(); return r.top >= rect.top - 1 && r.bottom <= rect.bottom + 1; }).length,
        more: Boolean(document.querySelector('.menu-more')),
        below: box.classList.contains('more-below'),
      };
    };
    const few = (n: number, description?: string) => Array.from({ length: n }, (_, i) => ({ label: `Item ${i + 1}`, description, checked: false }));
    openMenu(anchor, [{ label: 'First' }, { header: 'Group', items: few(5) }]);
    const five = read();
    openMenu(anchor, [{ label: 'First' }, { header: 'Group', items: few(7, 'A description long enough to wrap onto a second line inside the menu, at least') }]);
    const rich = read();
    closeMenu();
    anchor.remove();
    return { rows: MENU_GROUP_ROWS, five, rich };
  });
  expect(result.rows).toBe(5);
  expect(result.five).toEqual({ scrolls: false, inside: 5, more: false, below: false });
  expect(result.rich).toEqual({ scrolls: true, inside: 5, more: true, below: true });
});
