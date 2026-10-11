import { test } from '@e2e-dev/web';
import { expect } from 'e2e';
import { axeViolations, failOnPageErrors, mock } from './support.ts';

failOnPageErrors();

test('the task page fits a 390 px phone screen', async ({ app, browser, screen }) => {
  await browser.setViewport({ width: 390, height: 844 });
  await app.open(mock('/s/studio/t/12'));
  await expect(screen.getByRole('heading', { level: 1 })).toHaveText('#12 Checkout API');

  expect(await browser.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(390);
  await expect(screen.getByRole('textbox', 'Quick input')).toBeVisible();

  const menu = screen.getByRole('button', 'Open navigation');
  await menu.click();
  await expect(menu).toHaveAttribute('aria-expanded', 'true');
  await expect(screen.getByRole('link', /3D model viewer/)).toBeVisible();
  await browser.keyboard.press('Escape');
  await expect(menu).toHaveAttribute('aria-expanded', 'false');

  expect(await axeViolations(browser)).toEqual([]);
});

// F 2026-10-11: on phones the section tabs of Secrets (and MCP, Sessions) fell below the list —
// a phone rule meant for the Tasks toolbar reordered every filter tab strip.
test('section tabs stay under the page header on a phone', async ({ app, browser, screen }) => {
  await browser.setViewport({ width: 390, height: 844 });
  for (const path of ['/s/studio/secrets?section=agents', '/s/studio/mcp', '/s/studio/sessions']) {
    await app.open(mock(path));
    await expect(screen.getByRole('heading', { level: 1 })).toBeVisible();
    // Nothing after the tabs (the list, a filter beside them) is laid out above them.
    const misplaced = await browser.evaluate(() => {
      const tabs = document.querySelector('.page .tl-tabs')!;
      const top = tabs.getBoundingClientRect().top;
      const after: Element[] = [];
      for (let el = tabs.nextElementSibling; el; el = el.nextElementSibling) after.push(el);
      return after.filter((el) => el.getBoundingClientRect().height > 0 && el.getBoundingClientRect().bottom <= top).map((el) => el.className);
    });
    expect({ path, misplaced }).toEqual({ path, misplaced: [] });
    expect(await browser.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(390);
  }
});
