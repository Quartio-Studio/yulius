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
    ];
  });
  expect(result).toEqual([true, true, true, true, true, false, true, false]);
});
