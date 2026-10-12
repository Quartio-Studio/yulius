import { test } from '@e2e-dev/web';
import { expect } from 'e2e';
import { failOnPageErrors, mock } from './support.ts';

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
  // The last item is reachable: it scrolls into view when focused.
  await browser.keyboard.press('End');
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
