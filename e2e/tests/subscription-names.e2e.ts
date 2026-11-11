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

test("leaving one subscription's name for another's Rename keeps the second one open", async ({ app, browser, screen }) => {
  await app.open(mock('/admin/accounts', '&ambient=0'));
  await screen.getByRole('button', 'Rename Claude #1').click();
  await expect(screen.getByRole('textbox', 'New name for Claude #1')).toBeFocused();
  // Unchanged, then saved by leaving: neither takes the keyboard from the next field.
  await screen.getByRole('button', 'Rename Claude #4').click();
  await expect(screen.getByRole('textbox', 'New name for Claude #4')).toBeFocused();
  await screen.getByRole('textbox', 'New name for Claude #4').fill('personal');
  await screen.getByRole('button', 'Rename Claude #5').click();
  const third = screen.getByRole('textbox', 'New name for Claude #5');
  await expect(third).toBeFocused();
  await expect(screen.getByText('Subscription renamed to personal.')).toBeVisible();
  await expect(third).toBeFocused();
  await expect(browser.locator('main .account-card:has-text("Claude #1") .account-name strong')).toHaveText('Claude #1');
  await third.press('Escape');
  await expect(screen.getByRole('button', 'Rename Claude #5')).toBeFocused();
});

// The email line is typed in place like the name (SPEC-463): production's subscriptions read
// "email unknown", because Claude gives no email to a `setup-token` sign-in.

test("a subscription's email is typed in place under its name: \"Agent TEAM\" over its address", async ({ app, browser, screen }) => {
  await app.open(mock('/admin/accounts', '&ambient=0'));
  const emailLine = (name: string) => browser.locator(`main .account-card:has-text("${name}") .account-sub`);
  await screen.getByRole('button', 'Change the email of Claude spare').click();
  const field = screen.getByRole('textbox', 'Email of Claude spare');
  await expect(field).toBeFocused();
  await expect(field).toHaveValue('');

  // EML-001: what is not an address is refused before anything is sent, and the field stays open.
  await field.fill('not an address');
  await field.press('Enter');
  await expect(screen.getByText('Type an address like name@example.com.')).toBeVisible();
  await expect(field).toBeFocused();
  expect(await axeViolations(browser)).toEqual([]);
  // Leaving the refused field does not pull focus back into it.
  await browser.locator('main h1').click();
  expect(await browser.evaluate(() => document.activeElement?.classList.contains('account-email-input') ?? null)).toBe(false);
  await expect(field).toBeVisible();
  // Escape keeps it unknown, also after an address was typed (its blur must not save it), and
  // gives the keyboard back to the pencil.
  await field.fill('typed@example.com');
  await field.press('Escape');
  await expect(emailLine('Claude spare')).toHaveText('email unknown');
  await expect(screen.getByRole('button', 'Change the email of Claude spare')).toBeFocused();
  expect(await browser.evaluate(() => document.body.innerText.includes('typed@example.com'))).toBe(false);

  // A click on "email unknown" edits it too; Enter saves the trimmed address.
  await emailLine('Claude spare').click();
  await screen.getByRole('textbox', 'Email of Claude spare').fill('  agent@example.com ');
  await screen.getByRole('textbox', 'Email of Claude spare').press('Enter');
  await expect(screen.getByText('Email of Claude spare is now agent@example.com.')).toBeVisible();
  await expect(screen.getByRole('button', 'Change the email of Claude spare')).toBeFocused();
  await expect(emailLine('Claude spare')).toHaveText('agent@example.com');

  // The name above it is edited the same way: the card reads "Agent TEAM" over its address.
  await screen.getByRole('button', 'Rename Claude spare').click();
  await screen.getByRole('textbox', 'New name for Claude spare').fill('Agent TEAM');
  await screen.getByRole('textbox', 'New name for Claude spare').press('Enter');
  await expect(screen.getByText('Subscription renamed to Agent TEAM.')).toBeVisible();
  expect((await cards(browser)).filter((c) => c.name === 'Agent TEAM')).toEqual([{ name: 'Agent TEAM', email: 'agent@example.com' }]);

  // EML-005: every view that names the subscription shows what was typed; only this page edits it.
  await browser.locator('.btn-status').click();
  await expect(browser.locator('.status-pop .account-fold:has-text("Agent TEAM") .account-sub')).toHaveText('agent@example.com');
  expect(await browser.evaluate(() => document.querySelectorAll('.status-pop .account-rename').length)).toBe(0);
  await browser.keyboard.press('Escape');

  // EML-002: the field holds the email. Emptied and left, it removes nothing: a slip must not
  // lose the address.
  await screen.getByRole('button', 'Change the email of Agent TEAM').click();
  await expect(screen.getByRole('textbox', 'Email of Agent TEAM')).toHaveValue('agent@example.com');
  await screen.getByRole('textbox', 'Email of Agent TEAM').fill('');
  await browser.locator('main h1').click();
  await expect(screen.getByRole('textbox', 'Email of Agent TEAM')).toBeHidden();
  await expect(emailLine('Agent TEAM')).toHaveText('agent@example.com');
  // Enter in the empty field removes it: unknown again.
  await screen.getByRole('button', 'Change the email of Agent TEAM').click();
  const again = screen.getByRole('textbox', 'Email of Agent TEAM');
  await again.fill('');
  await again.press('Enter');
  await expect(screen.getByText('Email of Agent TEAM removed.')).toBeVisible();
  await expect(emailLine('Agent TEAM')).toHaveText('email unknown');

  expect(await axeViolations(browser)).toEqual([]);
});

test("leaving a subscription's email for another field saves it and keeps that field open", async ({ app, browser, screen }) => {
  await app.open(mock('/admin/accounts', '&ambient=0'));
  // An email Claude gave is changed the same way; unchanged, it is not sent.
  await screen.getByRole('button', 'Change the email of Claude #1').click();
  await expect(screen.getByRole('textbox', 'Email of Claude #1')).toHaveValue('mkarp@example.com');
  // From the email to the name of the same card: the email closes as it was.
  await screen.getByRole('button', 'Rename Claude #1').click();
  await expect(screen.getByRole('textbox', 'New name for Claude #1')).toBeFocused();
  await expect(browser.locator('main .account-card:has-text("mkarp@example.com") .account-sub')).toHaveText('mkarp@example.com');
  // From a typed email to another card's email: saved by leaving, and the update it causes
  // does not redraw the second field away.
  await screen.getByRole('button', 'Change the email of Claude #4').click();
  await screen.getByRole('textbox', 'Email of Claude #4').fill('personal@example.com');
  await screen.getByRole('button', 'Change the email of Claude #5').click();
  const next = screen.getByRole('textbox', 'Email of Claude #5');
  await expect(next).toBeFocused();
  await expect(next).toHaveValue('studio@example.com');
  await next.fill('half-typed@');
  await expect(screen.getByText('Email of Claude #4 is now personal@example.com.')).toBeVisible();
  // EML-001: the update of Claude #4 that arrived meanwhile neither closed nor saved this field.
  await expect(browser.locator('main .account-card:has-text("Claude #4") .account-sub')).toHaveText('personal@example.com');
  await expect(next).toBeFocused();
  await expect(next).toHaveValue('half-typed@');
  await next.press('Escape');
  await expect(screen.getByRole('button', 'Change the email of Claude #5')).toBeFocused();
  // Nothing was sent for the emails left unchanged or cancelled.
  expect((await cards(browser)).map((c) => c.email)).toEqual(['mkarp@example.com', 'team@example.com', 'ops@example.com', 'personal@example.com', 'studio@example.com', 'email unknown']);
});

type Holding = { __yuliusMock: { holdPath: string | null; held: Promise<void> | null; release: () => void; released: boolean; heldWaiting?: number } };

test('a field opened while the list is read again is neither closed nor saved by that read', async ({ app, browser, screen }) => {
  await app.open(mock('/admin/accounts', '&ambient=0'));
  await expect(screen.getByRole('button', 'Change the email of Claude spare')).toBeVisible();
  // From now on the list's reads wait, as on a slow connection.
  await browser.evaluate(() => {
    const db = (window as unknown as Holding).__yuliusMock;
    db.holdPath = '/api/admin/accounts';
    db.released = false;
    db.held = new Promise<void>((resolve) => { db.release = () => { db.released = true; resolve(); }; });
  });
  const waiting = () => browser.evaluate(() => (window as unknown as Holding).__yuliusMock.heldWaiting ?? 0);

  // The email is saved; its update makes the page read the list again.
  await screen.getByRole('button', 'Change the email of Claude spare').click();
  await screen.getByRole('textbox', 'Email of Claude spare').fill('agent@example.com');
  await screen.getByRole('textbox', 'Email of Claude spare').press('Enter');
  await expect(screen.getByText('Email of Claude spare is now agent@example.com.')).toBeVisible();
  await expect.poll(waiting).toBe(1);
  // The person goes on to the name while that read is on its way.
  await screen.getByRole('button', 'Rename Claude spare').click();
  const name = screen.getByRole('textbox', 'New name for Claude spare');
  await name.fill('Agent TE');
  await browser.evaluate(() => (window as unknown as Holding).__yuliusMock.release());
  await expect.poll(waiting).toBe(0);
  // The read has landed (two frames later the page would have drawn it): the field is as it was.
  await browser.evaluate(() => new Promise((resolve) => { requestAnimationFrame(() => requestAnimationFrame(() => resolve(null))); }));
  await expect(name).toBeFocused();
  await expect(name).toHaveValue('Agent TE');
  expect((await cards(browser)).map((c) => c.name)).not.toContain('Agent TE');

  // Once the name is saved, the page shows both lines from the server.
  await name.fill('Agent TEAM');
  await name.press('Enter');
  await expect(screen.getByText('Subscription renamed to Agent TEAM.')).toBeVisible();
  await expect.poll(async () => (await cards(browser)).filter((c) => c.name === 'Agent TEAM')).toEqual([{ name: 'Agent TEAM', email: 'agent@example.com' }]);
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
