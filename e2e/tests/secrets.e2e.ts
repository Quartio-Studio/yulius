import { test } from '@e2e-dev/web';
import { expect } from 'e2e';
import { axeViolations, failOnPageErrors, mock } from './support.ts';

failOnPageErrors();

// Synthetic mock values (assets/mock.js, "secrets vault"); not real credentials.
const TEAM_VALUE = 'mock-not-a-real-password';
const AGENT_VALUE = 'mock-agent-value-1';
const MOCK_CODE = '123456';

/** Sets up two-factor in the dialog a first reveal opens (SPEC-040): a wrong code is refused. */
async function setUpTwoFactor(screen: any, browser: any) {
  const setup = screen.getByRole('dialog', 'Set up two-factor');
  await expect(setup).toBeVisible();
  await expect(setup.getByText(/Add this key to your authenticator app/)).toBeVisible();
  expect(await axeViolations(browser)).toEqual([]);
  await expect(setup.getByRole('link', 'Open in an authenticator app')).toHaveAttribute('href', /^otpauth:\/\/totp\//);
  await screen.getByLabel('Code from your app').fill('000000');
  await setup.getByRole('button', 'Unlock').click();
  await expect(setup.getByRole('alert')).toHaveText(/wrong code/i);
  await screen.getByLabel('Code from your app').fill(MOCK_CODE);
  await setup.getByRole('button', 'Unlock').click();
  await expect(setup).toBeHidden();
}

// F 2026-10-12 (task #39): "It should be only value title notes. 2FA should be per wallet and
// should be mandatory." A team secret is a title, a value and notes; reading one needs your own
// two-factor code.
test('team secrets are a title, a value and notes, shown after a two-factor code', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/secrets'));
  await expect(screen.getByRole('heading', { level: 1 })).toHaveText('Secrets');
  await expect(screen.getByText('Shared Gmail')).toBeVisible();
  expect(await browser.evaluate((v: string) => document.body.innerHTML.includes(v), TEAM_VALUE)).toBe(false);
  expect(await axeViolations(browser)).toEqual([]);

  await screen.getByRole('button', 'Show Shared Gmail').click();
  await setUpTwoFactor(screen, browser);
  const shown = screen.getByRole('dialog', 'Shared Gmail');
  await expect(shown).toBeVisible();
  await expect(shown.getByText(TEAM_VALUE)).toBeVisible();
  await expect(shown.getByText(/Recovery codes/)).toBeVisible();
  await shown.getByRole('button', 'Done').click();
  await expect(shown).toBeHidden();
  // The setup and the reveal are in the activity trail.
  await expect(screen.getByText(/turned on two-factor/).first()).toBeVisible();
  await expect(screen.getByText(/revealed/).first()).toBeVisible();

  await screen.getByRole('button', 'Add team secret').click();
  const add = screen.getByRole('dialog', 'Add team secret');
  const labels = await browser.evaluate(() => [...document.querySelectorAll('dialog.modal .field-label')].map((l) => l.textContent!.trim()));
  expect(labels).toEqual(['Title', 'Value', 'Notes']);
  expect(await axeViolations(browser)).toEqual([]);
  await screen.getByLabel('Title').fill('AWS root');
  await screen.getByLabel('Value').fill('mock-aws-root-value');
  await add.getByRole('button', 'Add').click();
  await expect(add).toBeHidden();
  // Listed, and recorded in the activity trail ("added AWS root").
  await expect(screen.getByText('AWS root').first()).toBeVisible();
  await expect(screen.getByText(/added/).first()).toBeVisible();
  expect(await browser.evaluate(() => document.body.innerHTML.includes('mock-aws-root-value'))).toBe(false);
});

test('agent secrets are bound to hosts and readable by their managers after a code', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/secrets?section=agents'));
  await expect(screen.getByRole('radio', 'Agent secrets')).toHaveAttribute('aria-checked', 'true');
  await expect(screen.getByText('STRIPE_KEY')).toBeVisible();
  await expect(screen.getByText('api.stripe.com')).toBeVisible();
  expect(await browser.evaluate((v: string) => document.body.innerHTML.includes(v), AGENT_VALUE)).toBe(false);
  expect(await axeViolations(browser)).toEqual([]);

  await screen.getByRole('button', 'Add agent secret').first().click();
  const dialog = screen.getByRole('dialog', 'Add agent secret');
  const labels = await browser.evaluate(() => [...document.querySelectorAll('dialog.modal .field-label')].filter((l) => !l.closest('[hidden]')).map((l) => l.textContent!.trim()));
  expect(labels).toEqual(['Name', 'Value', 'Notes', 'Scope', 'Hosts', 'Added to requests as']);
  await screen.getByLabel('Name').fill('RESEND_KEY');
  await screen.getByLabel('Hosts').fill('api.resend.com');
  await screen.getByLabel('Value').fill('short');
  await dialog.getByRole('button', 'Add').click();
  await expect(browser.locator('.field-error')).toHaveText(/At least 8 characters/);
  await screen.getByLabel('Value').fill('re_mock_value_123');
  // A user name only for Basic credentials.
  await browser.locator('dialog.modal select').nth(1).selectOption({ value: 'basic' });
  await expect(screen.getByLabel('User name')).toBeVisible();
  await browser.locator('dialog.modal select').nth(1).selectOption({ value: 'bearer' });
  await expect(screen.getByLabel('User name')).toBeHidden();
  await dialog.getByRole('button', 'Add').click();
  await expect(dialog).toBeHidden();
  await expect(screen.getByText('RESEND_KEY')).toBeVisible();
  await expect(screen.getByText('api.resend.com')).toBeVisible();
  expect(await browser.evaluate((v: string) => document.body.innerHTML.includes(v), 're_mock_value_123')).toBe(false);
  expect(await browser.evaluate((v: string) => document.body.innerHTML.includes(v), AGENT_VALUE)).toBe(false);

  // Its manager reads it back after a two-factor code.
  await screen.getByRole('button', 'Show STRIPE_KEY').click();
  await setUpTwoFactor(screen, browser);
  const shown = screen.getByRole('dialog', 'STRIPE_KEY');
  await expect(shown.getByText(AGENT_VALUE)).toBeVisible();
  await shown.getByRole('button', 'Done').click();

  // The section survives a reload; the Team section is one click away.
  await browser.reload();
  await expect(screen.getByRole('radio', 'Agent secrets')).toHaveAttribute('aria-checked', 'true');
  await screen.getByRole('radio', 'Team secrets').click();
  await expect(screen.getByText('Shared Gmail')).toBeVisible();
});

// Review of SPEC-040: an older secret's address, user name and seed live in its notes, so its
// notes are edited from their current text, never replaced blindly.
test('editing a team secret loads its current notes after a two-factor code', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/secrets'));
  await screen.getByRole('button', 'Edit Shared Gmail').click();
  const edit = screen.getByRole('dialog', 'Edit Shared Gmail');
  await expect(edit.getByText(/what you type replaces them/)).toBeVisible();
  await screen.getByRole('button', 'Edit current notes of Shared Gmail').click();
  await setUpTwoFactor(screen, browser);
  await expect(browser.locator('dialog.modal textarea')).toHaveValue(/User name: team@example\.test/);
  await browser.locator('dialog.modal textarea').fill('User name: team@example.test\nRecovery codes: new');
  await edit.getByRole('button', 'Save').click();
  await expect(edit).toBeHidden();
  await screen.getByRole('button', 'Show Shared Gmail').click();
  await expect(screen.getByRole('dialog', 'Shared Gmail').getByText(/Recovery codes: new/)).toBeVisible();
});

test('members who only read a sandbox cannot add agent secrets', async ({ app, screen }) => {
  await app.open(mock('/s/sandbox-3/secrets?section=agents'));
  await expect(screen.getByRole('button', 'Add agent secret')).toBeDisabled();
});

// Founder feedback 2026-10-11 ("ux is a little broken"): the add button sat in a row glued to the
// banner and the list. Like every list page, the header holds the shown section's add button, and
// the section's parts keep the page's spacing.
test('each section adds from the page header and its parts keep their spacing', async ({ app, browser, screen }) => {
  const headerButton = () => browser.evaluate(() => [...document.querySelectorAll('.page-head .page-actions button')].map((b) => b.textContent!.trim()));
  const gaps = () => browser.evaluate(() => {
    const parts = [...document.querySelector('.page-inner > .stack > .stack')!.children].map((el) => el.getBoundingClientRect());
    return parts.slice(1).map((r, i) => Math.round(r.top - parts[i].bottom));
  });
  await app.open(mock('/s/studio/secrets?section=agents'));
  await expect(screen.getByText('STRIPE_KEY')).toBeVisible();
  expect(await headerButton()).toEqual(['Add agent secret']);
  for (const gap of await gaps()) expect(gap).toBeGreaterThanOrEqual(8);

  await screen.getByRole('radio', 'Team secrets').click();
  await expect(screen.getByText('Shared Gmail')).toBeVisible();
  expect(await headerButton()).toEqual(['Add team secret']);
  for (const gap of await gaps()) expect(gap).toBeGreaterThanOrEqual(8);
});

// Founder feedback 2026-10-11: "when i click outside any popup i want it to close".
test('a click outside a dialog closes it; a click or a selection inside does not', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/secrets'));
  await screen.getByRole('button', 'Add team secret').first().click();
  const add = screen.getByRole('dialog', 'Add team secret');
  await expect(add).toBeVisible();
  await screen.getByLabel('Title').fill('Draft');
  await screen.getByLabel('Title').click();
  await expect(add).toBeVisible();

  // Selecting text in a field and releasing past the dialog's edge keeps it open.
  const field = await browser.locator('dialog.modal input').first().boundingBox();
  await browser.mouse.move(field!.x + 8, field!.y + field!.height / 2);
  await browser.mouse.down();
  await browser.mouse.move(4, 4);
  await browser.mouse.up();
  await expect(add).toBeVisible();

  // A click on the backdrop, outside the dialog.
  await browser.mouse.move(4, 4);
  await browser.mouse.down();
  await browser.mouse.up();
  await expect(add).toBeHidden();
});
