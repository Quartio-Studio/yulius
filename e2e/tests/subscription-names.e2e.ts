import { type Browser, test } from '@e2e-dev/web';
import { expect } from 'e2e';
import { axeViolations, failOnPageErrors, mock } from './support.ts';

failOnPageErrors();

// A subscription's own name, edited in place, and the email of its Claude account (SPEC-244).

/** The name and the email line of each subscription card on the Subscriptions page. */
const cards = (browser: Browser) => browser.evaluate(() =>
  [...document.querySelectorAll('main .account-card')].map((card) => ({
    name: card.querySelector('.account-name strong')?.textContent ?? null,
    email: card.querySelector('.account-sub')?.textContent ?? null,
  })));

test('each subscription shows the email of its Claude account, or that it is unknown', async ({ app, browser }) => {
  await app.open(mock('/admin/accounts', '&ambient=0'));
  await expect(browser.locator('main .account-card').first()).toBeVisible();
  const shown = await cards(browser);
  const emailOf = (name: string) => shown.find((c) => c.name === name)?.email ?? null;
  expect(emailOf('Claude #2')).toBe('team@example.com');
  // SUB-004: no email read yet reads as unknown, never as the provider or an empty line.
  expect(emailOf('Claude spare')).toBe('email unknown');

  expect(await axeViolations(browser)).toEqual([]);
});

test('a subscription is renamed in place and the new name shows with its email everywhere', async ({ app, browser, screen }) => {
  await app.open(mock('/admin/accounts', '&ambient=0'));
  await screen.getByRole('button', 'Rename Claude #2').click();
  const field = screen.getByRole('textbox', 'New name for Claude #2');
  await expect(field).toBeFocused();

  // SUB-001: an empty name is refused and the field stays open.
  await field.fill('   ');
  await field.press('Enter');
  await expect(screen.getByText('Give the subscription a name.')).toBeVisible();
  await expect(field).toBeFocused();
  // Leaving the refused field does not pull focus back into it.
  await browser.locator('main h1').click();
  expect(await browser.evaluate(() => document.activeElement?.classList.contains('account-rename-input') ?? null)).toBe(false);
  await expect(field).toBeVisible();
  // Escape keeps the old name, also after a real name was typed (its blur must not save it),
  // and gives the keyboard back to the pencil.
  await field.fill('typed, then cancelled');
  await field.press('Escape');
  await expect(browser.locator('main .account-card:has-text("Claude #2") .account-name strong')).toHaveText('Claude #2');
  await expect(screen.getByRole('button', 'Rename Claude #2')).toBeFocused();
  expect(await browser.evaluate(() => document.body.innerText.includes('typed, then cancelled'))).toBe(false);

  // A click on the name edits it too; Enter saves.
  await browser.locator('main .account-card:has-text("Claude #2") .account-name-text').click();
  await screen.getByRole('textbox', 'New name for Claude #2').fill('  work Max ');
  await screen.getByRole('textbox', 'New name for Claude #2').press('Enter');
  await expect(screen.getByText('Subscription renamed to work Max.')).toBeVisible();
  await expect(screen.getByRole('button', 'Rename work Max')).toBeFocused();
  await expect(browser.locator('main .account-card:has-text("work Max") .account-sub')).toHaveText('team@example.com');
  expect((await cards(browser)).map((c) => c.name)).not.toContain('Claude #2');

  // The cancelled name was never sent: the subscription is not named that anywhere.
  expect((await cards(browser)).map((c) => c.name)).not.toContain('typed, then cancelled');
  // SUB-005: the Status panel and Task details name it the new way, with its email.
  await browser.locator('.btn-status').click();
  await expect(browser.locator('.status-pop .account-fold:has-text("work Max") .account-sub')).toHaveText('team@example.com');
  await browser.keyboard.press('Escape');
  await browser.evaluate(() => { window.location.hash = '#/s/studio/tasks/14'; });
  await expect(screen.getByText('work Max · team@example.com').first()).toBeVisible();
  await expect(browser.locator('main')).not.toContainText('Claude #2');

  expect(await axeViolations(browser)).toEqual([]);
});

test('someone who does not manage subscriptions sees their names without emails (UI-022)', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/tasks/14', '&ambient=0&role=member'));
  await expect(screen.getByText('Claude #2').first()).toBeVisible();
  await expect(browser.locator('main')).not.toContainText('@example.com');
  await expect(browser.locator('main')).not.toContainText('email unknown');
  await browser.locator('.btn-status').click();
  await expect(browser.locator('.status-pop .account-fold:has-text("Claude #2") .account-sub')).toHaveText('Claude');
  expect(await browser.evaluate(() => document.querySelector('.status-pop')?.textContent?.includes('@') ?? null)).toBe(false);
});

test('a subscription without an email reads "email unknown" in Task details (unit, in the page)', async ({ app, browser }) => {
  await app.open(mock('/s/studio'));
  const lines = await browser.evaluate(async () => {
    const url = '/assets/format.js';
    const { accountRef } = await import(url);
    return [accountRef('personal', null), accountRef('work Max', 'pawel@example.com'), accountRef(null, null), accountRef('personal', null, { unknown: false })];
  });
  // An administrator's view says the email is unknown; anyone else's leaves it out (UI-022).
  expect(lines).toEqual(['personal · email unknown', 'work Max · pawel@example.com', null, 'personal']);
});
