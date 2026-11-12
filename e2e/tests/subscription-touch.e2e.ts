import { test } from '@e2e-dev/web';
import { expect } from 'e2e';
import { chromium, type Browser, type Page } from 'playwright-core';

// A subscription's two lines on a touch screen (SPEC-463). The runner's browser has no touch
// screen (its options take none), so these tests open their own Chromium with one, on the app
// the runner serves. A tap is not a quick mouse click: the pointer is already up when the
// field that is left hears of it (with the tap's compatibility mousedown).

/** A phone with a touch screen on the Subscriptions page, and a way to open that page afresh. */
async function phone(baseUrl: string | undefined): Promise<{ browser: Browser; page: Page; open: () => Promise<void> }> {
  if (!baseUrl) throw new Error('the runner serves no app to open');
  const browser = await chromium.launch();
  const context = await browser.newContext({ viewport: { width: 360, height: 800 }, hasTouch: true, isMobile: true });
  const page = await context.newPage();
  const open = async () => {
    // A hash-only change would not load the page again: leave it first.
    await page.goto('about:blank');
    await page.goto(new URL('/?mock=1&ambient=0#/admin/accounts', baseUrl).href);
    await page.locator('main .account-card').first().waitFor({ timeout: 30_000 });
  };
  await open();
  // After the first load, a step that does not come is a failure within the usual budget.
  page.setDefaultTimeout(5_000);
  return { browser, page, open };
}

/** The card of the subscription whose field `name` is open (its texts are fields then). */
const cardOf = (page: Page, field: string) => page.locator('main .card', { has: page.getByRole('textbox', { name: field, exact: true }) });

test('a tap on a control arrives although the field it leaves shows its refusal (#1174)', async ({ app }) => {
  const { browser, page, open } = await phone(app.baseUrl);
  try {
    const dialog = page.locator('dialog[open]', { hasText: 'Remove Claude #4?' });

    // The name emptied, then a tap on Remove of the same card.
    await page.getByRole('button', { name: 'Rename Claude #4', exact: true }).tap();
    await page.getByRole('textbox', { name: 'New name for Claude #4', exact: true }).fill('   ');
    await cardOf(page, 'New name for Claude #4').getByRole('button', { name: 'Remove', exact: true }).tap();
    await dialog.waitFor();
    await page.getByText('Give the subscription a name.').waitFor();

    // The name emptied, then a tap on the email's pencil right under it.
    await open();
    await page.getByRole('button', { name: 'Rename Claude #4', exact: true }).tap();
    await page.getByRole('textbox', { name: 'New name for Claude #4', exact: true }).fill('   ');
    await page.getByRole('button', { name: 'Change the email of Claude #4', exact: true }).tap();
    await page.getByRole('textbox', { name: 'Email of Claude #4', exact: true }).waitFor();
    await page.getByText('Give the subscription a name.').waitFor();

    // What is not an address under the email, then a tap on Remove.
    await open();
    await page.getByRole('button', { name: 'Change the email of Claude #4', exact: true }).tap();
    await page.getByRole('textbox', { name: 'Email of Claude #4', exact: true }).fill('not an address');
    await cardOf(page, 'Email of Claude #4').getByRole('button', { name: 'Remove', exact: true }).tap();
    await dialog.waitFor();
    await page.getByText('Type an address like name@example.com.').waitFor();

    // Nothing was sent by any of it.
    const sent = await page.evaluate(() => Object.keys((window as unknown as { __yuliusMock: { sent?: Record<string, number> } }).__yuliusMock.sent ?? {}).filter((path) => !path.endsWith('/usage')));
    expect(sent).toEqual([]);
  } finally {
    await browser.close();
  }
});

test('by touch: the name, then a tap on the email under it; a card reads "Agent TEAM" over its address', async ({ app }) => {
  const { browser, page } = await phone(app.baseUrl);
  try {
    await page.getByRole('button', { name: 'Rename Claude #4', exact: true }).tap();
    await page.getByRole('textbox', { name: 'New name for Claude #4', exact: true }).fill('Agent TEAM');
    // The tap leaves the name, which is saved, and opens the email.
    await page.getByRole('button', { name: 'Change the email of Claude #4', exact: true }).tap();
    const email = page.getByRole('textbox', { name: /^Email of / });
    await email.waitFor();
    await page.getByText('Subscription renamed to Agent TEAM.').waitFor();
    await email.fill('agent@example.com');
    await email.press('Enter');
    await page.getByText('Email of Agent TEAM is now agent@example.com.').waitFor();
    await expect.poll(() => page.evaluate(() => {
      const card = document.querySelector('main .account-card');
      return { name: card?.querySelector('.account-name strong')?.textContent ?? null, email: card?.querySelector('.account-sub')?.textContent ?? null };
    })).toEqual({ name: 'Agent TEAM', email: 'agent@example.com' });
    const sent = await page.evaluate(() => (window as unknown as { __yuliusMock: { sent?: Record<string, number> } }).__yuliusMock.sent?.['PATCH /api/admin/accounts/acc_x1'] ?? 0);
    expect(sent).toBe(2);
  } finally {
    await browser.close();
  }
});
