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
