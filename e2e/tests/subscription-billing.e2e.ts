import { test } from '@e2e-dev/web';
import { expect } from 'e2e';
import { axeViolations, failOnPageErrors, mock } from './support.ts';

failOnPageErrors();

// When each subscription charges its card (SPEC-651 BIL-006, BIL-007).

test('a person sets, changes and removes when a subscription charges the card (SPEC-651)', async ({ app, browser, screen }) => {
  await app.open(mock('/admin/accounts', '&ambient=0'));
  const card = browser.locator('main .grid-2 > .card:has-text("Claude #1")');
  const billing = browser.locator('main .grid-2 > .card:has-text("Claude #1") .account-billing');
  await expect(billing).toContainText('Next charge');
  await expect(billing).toContainText('Not set');

  // Save without a date is refused under the field, and nothing changes.
  await card.getByRole('button', 'Set charge date').click();
  const dialog = screen.getByRole('dialog', 'Charge date · Claude #1');
  await expect(dialog.getByText(/The day this subscription charges your card/)).toBeVisible();
  expect(await axeViolations(browser)).toEqual([]);
  await dialog.getByRole('button', 'Save').click();
  await expect(dialog.getByText('Pick the date of a charge.')).toBeVisible();
  await expect(dialog.getByRole('button', 'Remove')).toHaveCount(0);

  // A charge due today (the page's own date) reads "today", monthly by default.
  const today = await browser.evaluate(() => {
    const now = new Date();
    const pad = (n: number) => String(n).padStart(2, '0');
    return { iso: `${now.getFullYear()}-${pad(now.getMonth() + 1)}-${pad(now.getDate())}`, day: now.getDate() };
  });
  await dialog.getByLabel(/^A charge date/).fill(today.iso);
  await dialog.getByRole('button', 'Save').click();
  await expect(dialog).toBeHidden();
  await expect(browser.locator('.toast-ok .toast-msg:has-text("Next charge of Claude #1:")')).toBeVisible();
  await expect(billing).toContainText('today');
  const suffix = (n: number) => (n % 10 === 1 && n !== 11 ? 'st' : n % 10 === 2 && n !== 12 ? 'nd' : n % 10 === 3 && n !== 13 ? 'rd' : 'th');
  await expect(billing).toContainText(`Every month on the ${today.day}${suffix(today.day)}`);
  // The other cards are not touched.
  await expect(browser.locator('main .grid-2 > .card:has-text("Claude #3") .account-billing')).toContainText('Not set');

  // The pencil reopens it with the stored date; yearly is said with its day and month.
  await card.getByRole('button', 'Change the charge date of Claude #1').click();
  const again = screen.getByRole('dialog', 'Charge date · Claude #1');
  await expect(again.getByLabel(/^A charge date/)).toHaveValue(today.iso);
  await browser.locator('.modal .seg label:has-text("Every year")').click();
  await again.getByRole('button', 'Save').click();
  await expect(again).toBeHidden();
  await expect(billing).toContainText(/Every year on \S+/);
  await expect(billing).not.toContainText('Every month');

  // Remove brings "Not set" back.
  await card.getByRole('button', 'Change the charge date of Claude #1').click();
  await screen.getByRole('dialog', 'Charge date · Claude #1').getByRole('button', 'Remove').click();
  await expect(browser.locator('.toast-ok .toast-msg:has-text("Charge date of Claude #1 removed.")')).toBeVisible();
  await expect(billing).toContainText('Not set');
  await expect(card.getByRole('button', 'Set charge date')).toBeVisible();
});

test('a charge on the 31st falls on a shorter month\'s last day, and its next charge is counted ahead (SPEC-651 BIL-002)', async ({ app, browser, screen }) => {
  await app.open(mock('/admin/accounts', '&ambient=0'));
  const card = browser.locator('main .grid-2 > .card:has-text("Claude #1")');
  const billing = browser.locator('main .grid-2 > .card:has-text("Claude #1") .account-billing');
  await card.getByRole('button', 'Set charge date').click();
  const dialog = screen.getByRole('dialog', 'Charge date · Claude #1');
  await dialog.getByLabel(/^A charge date/).fill('2026-01-31');
  await dialog.getByRole('button', 'Save').click();
  await expect(dialog).toBeHidden();
  await expect(billing).toContainText('Every month on the 31st, or the month’s last day');
  // The next charge is never in the past: today, tomorrow or in some days.
  await expect(billing).toContainText(/· (today|tomorrow|in \d+ days)/);
});
