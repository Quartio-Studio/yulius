import { type Browser, type Screen, test } from '@e2e-dev/web';
import { expect } from 'e2e';
import { axeViolations, failOnPageErrors, mock } from './support.ts';

failOnPageErrors();

// When each subscription charges its card (SPEC-651 BIL-002, BIL-006, BIL-007). The page's clock
// is held at noon on Wed 10 Feb 2027 (its own time zone), so every date and day count is exact.

type Mock = { __yuliusMock: { sent?: Record<string, number> } };

const BILLING = 'PUT /api/admin/accounts/acc_c1/billing';
const CARD = 'main .grid-2 > .card:has-text("Claude #1")';
const card = (browser: Browser) => browser.locator(CARD);
const billing = (browser: Browser) => browser.locator(`${CARD} .account-billing`);
const nextDate = (browser: Browser) => browser.locator(`${CARD} .account-billing strong`);
const sent = (browser: Browser) => browser.evaluate((key) => (window as unknown as Mock).__yuliusMock.sent?.[key] ?? 0, BILLING);

/** Opens the Subscriptions page with its clock at noon on 10 Feb 2027. */
async function openAt10Feb(app: { open: (url: string) => Promise<unknown> }, browser: Browser) {
  await app.open(mock('/admin/accounts', '&ambient=0'));
  await expect(billing(browser)).toContainText('Not set');
  await browser.evaluate(() => {
    const noon = new Date(2027, 1, 10, 12).getTime();
    Date.now = () => noon;
  });
}

/** A date as the card shows it, in the browser's own locale. */
const shown = (browser: Browser, iso: string, weekday = true) => browser.evaluate(([day, withWeekday]) => {
  const [y, m, d] = String(day).split('-').map(Number);
  const options: Intl.DateTimeFormatOptions = { day: 'numeric', month: 'short', year: 'numeric', timeZone: 'UTC' };
  if (withWeekday) options.weekday = 'short';
  return new Date(Date.UTC(y, m - 1, d)).toLocaleDateString(undefined, options);
}, [iso, weekday] as [string, boolean]);

/** Sets the charge date of Claude #1 in its dialog: `anchor` and, when given, Every year. */
async function setCharge(browser: Browser, screen: Screen, anchor: string, yearly = false) {
  const button = browser.locator(`${CARD} .account-billing button`);
  await button.click();
  const dialog = screen.getByRole('dialog', 'Charge date · Claude #1');
  await dialog.getByLabel(/^A charge date/).fill(anchor);
  await browser.locator(`.modal .seg label:has-text("${yearly ? 'Every year' : 'Every month'}")`).click();
  await dialog.getByRole('button', 'Save').click();
  await expect(dialog).toBeHidden();
}

test('a person sets, changes and removes when a subscription charges the card (SPEC-651)', async ({ app, browser, screen }) => {
  await openAt10Feb(app, browser);
  await expect(billing(browser)).toContainText('Next charge');

  // Save without a date is refused under the field, and nothing is sent.
  await card(browser).getByRole('button', 'Set charge date').click();
  const dialog = screen.getByRole('dialog', 'Charge date · Claude #1');
  await expect(dialog.getByText(/The day this subscription charges your card/)).toBeVisible();
  expect(await axeViolations(browser)).toEqual([]);
  await dialog.getByRole('button', 'Save').click();
  await expect(dialog.getByText('Pick the date of a charge.')).toBeVisible();
  await expect(dialog.getByRole('button', 'Remove')).toHaveCount(0);
  expect(await sent(browser)).toBe(0);

  // A refusal from the server stays in the dialog, with nothing changed.
  await dialog.getByLabel(/^A charge date/).fill('1999-12-31');
  await dialog.getByRole('button', 'Save').click();
  await expect(browser.locator('.modal .modal-error')).toHaveText('The charge date must be between the years 2000 and 2100');
  await expect(dialog).toBeVisible();

  // A charge on the 10th is today, monthly by default; the toast names the date.
  await dialog.getByLabel(/^A charge date/).fill('2026-12-10');
  await dialog.getByRole('button', 'Save').click();
  await expect(dialog).toBeHidden();
  await expect(browser.locator('.toast-ok .toast-msg').last()).toHaveText(`Next charge of Claude #1: ${await shown(browser, '2027-02-10', false)}.`);
  await expect(nextDate(browser)).toHaveText(await shown(browser, '2027-02-10'));
  await expect(billing(browser)).toContainText('· today');
  await expect(billing(browser)).toContainText('Every month on the 10th');
  // The keyboard is back on the card's charge-date control.
  await expect.poll(() => browser.evaluate(() => document.activeElement?.getAttribute('aria-label') ?? document.activeElement?.textContent ?? null))
    .toBe('Change the charge date of Claude #1');
  // The other cards are not touched.
  await expect(browser.locator('main .grid-2 > .card:has-text("Claude #3") .account-billing')).toContainText('Not set');

  // The pencil reopens it with the stored date; yearly is said with its day and month.
  await card(browser).getByRole('button', 'Change the charge date of Claude #1').click();
  const again = screen.getByRole('dialog', 'Charge date · Claude #1');
  await expect(again.getByLabel(/^A charge date/)).toHaveValue('2026-12-10');
  await browser.locator('.modal .seg label:has-text("Every year")').click();
  await again.getByRole('button', 'Save').click();
  await expect(again).toBeHidden();
  await expect(nextDate(browser)).toHaveText(await shown(browser, '2027-12-10'));
  await expect(billing(browser)).toContainText('· in 303 days');
  await expect(billing(browser)).not.toContainText('Every month');

  // Remove brings "Not set" back, and the keyboard to Set charge date.
  await card(browser).getByRole('button', 'Change the charge date of Claude #1').click();
  await screen.getByRole('dialog', 'Charge date · Claude #1').getByRole('button', 'Remove').click();
  await expect(browser.locator('.toast-ok .toast-msg:has-text("Charge date of Claude #1 removed.")')).toBeVisible();
  await expect(billing(browser)).toContainText('Not set');
  await expect(card(browser).getByRole('button', 'Set charge date')).toBeVisible();
  await expect.poll(() => browser.evaluate(() => document.activeElement?.textContent ?? null)).toBe('Set charge date');
});

test('the next charge is counted ahead from today: tomorrow, next month, a short month\'s last day (SPEC-651 BIL-002)', async ({ app, browser, screen }) => {
  await openAt10Feb(app, browser);
  // A charge on the 11th is tomorrow.
  await setCharge(browser, screen, '2027-01-11');
  await expect(nextDate(browser)).toHaveText(await shown(browser, '2027-02-11'));
  await expect(billing(browser)).toContainText('· tomorrow');
  // One on the 9th was yesterday: the next is 9 March.
  await setCharge(browser, screen, '2027-01-09');
  await expect(nextDate(browser)).toHaveText(await shown(browser, '2027-03-09'));
  await expect(billing(browser)).toContainText('· in 27 days');
  // One on the 31st falls on 28 February.
  await setCharge(browser, screen, '2026-01-31');
  await expect(nextDate(browser)).toHaveText(await shown(browser, '2027-02-28'));
  await expect(billing(browser)).toContainText('· in 18 days');
  await expect(billing(browser)).toContainText('Every month on the 31st, or the month’s last day');
  // A yearly 29 February falls on 28 February in 2027, and says so.
  await setCharge(browser, screen, '2024-02-29', true);
  await expect(nextDate(browser)).toHaveText(await shown(browser, '2027-02-28'));
  await expect(billing(browser)).toContainText('· in 18 days');
  await expect(billing(browser)).toContainText(/Every year on .+, or .+ in other years/);
});
