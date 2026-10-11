import { test } from '@e2e-dev/web';
import { expect } from 'e2e';
import { axeViolations, failOnPageErrors, mock } from './support.ts';

failOnPageErrors();

// Synthetic mock values (assets/mock.js, "secrets vault"); not real credentials.
const MOCK_PASSWORD = 'correct horse battery';
const TEAM_PASSWORD = 'mock-not-a-real-password';
const AGENT_VALUE = 'mock-agent-value-1';

// F 2026-10-10 (3a): "2 sections for storing secrets … team secrets … for agents … Agent can
// access them but never see them." A team password shows only after confirming it's you.
test('team secrets reveal only after confirming it is you', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/secrets'));
  await expect(screen.getByRole('heading', { level: 1 })).toHaveText('Secrets');
  await expect(screen.getByText('Shared Gmail')).toBeVisible();
  await expect(screen.getByText(/team@example\.test/)).toBeVisible();
  expect(await browser.evaluate((v: string) => document.body.innerHTML.includes(v), TEAM_PASSWORD)).toBe(false);
  expect(await axeViolations(browser)).toEqual([]);

  await screen.getByRole('button', 'Show Shared Gmail').click();
  const confirm = screen.getByRole('dialog', 'Confirm it’s you');
  await expect(confirm).toBeVisible();
  await screen.getByLabel('Your password').fill('not the password');
  await confirm.getByRole('button', 'Confirm').click();
  await expect(confirm.getByRole('alert')).toHaveText(/wrong password/i);
  await screen.getByLabel('Your password').fill(MOCK_PASSWORD);
  await confirm.getByRole('button', 'Confirm').click();

  const shown = screen.getByRole('dialog', 'Shared Gmail');
  await expect(shown).toBeVisible();
  await expect(shown.getByText(TEAM_PASSWORD)).toBeVisible();
  await expect(shown.getByText(/Recovery codes/)).toBeVisible();
  await shown.getByRole('button', 'Done').click();
  await expect(shown).toBeHidden();
  // The reveal is in the activity trail.
  await expect(screen.getByText(/revealed/).first()).toBeVisible();

  await screen.getByRole('button', 'Add team secret').click();
  const add = screen.getByRole('dialog', 'Add team secret');
  await screen.getByLabel('Title').fill('AWS root');
  await add.getByRole('button', 'Add').click();
  await expect(add).toBeHidden();
  // Listed, and recorded in the activity trail ("added AWS root").
  await expect(screen.getByText('AWS root').first()).toBeVisible();
  await expect(screen.getByText(/added/).first()).toBeVisible();
});

test('agent secrets are write-only and bound to hosts', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/secrets?section=agents'));
  await expect(screen.getByRole('radio', 'Agent secrets')).toHaveAttribute('aria-checked', 'true');
  await expect(screen.getByText('STRIPE_KEY')).toBeVisible();
  await expect(screen.getByText('api.stripe.com')).toBeVisible();
  expect(await browser.evaluate((v: string) => document.body.innerHTML.includes(v), AGENT_VALUE)).toBe(false);
  expect(await axeViolations(browser)).toEqual([]);

  await screen.getByRole('button', 'Add agent secret').first().click();
  const dialog = screen.getByRole('dialog', 'Add agent secret');
  await screen.getByLabel('Name').fill('RESEND_KEY');
  await screen.getByLabel('Hosts').fill('api.resend.com');
  await screen.getByLabel('Value').fill('short');
  await dialog.getByRole('button', 'Add').click();
  await expect(browser.locator('.field-error')).toHaveText(/At least 8 characters/);
  await screen.getByLabel('Value').fill('re_mock_value_123');
  await dialog.getByRole('button', 'Add').click();
  await expect(dialog).toBeHidden();
  await expect(screen.getByText('RESEND_KEY')).toBeVisible();
  await expect(screen.getByText('api.resend.com')).toBeVisible();
  expect(await browser.evaluate((v: string) => document.body.innerHTML.includes(v), 're_mock_value_123')).toBe(false);
  expect(await browser.evaluate((v: string) => document.body.innerHTML.includes(v), AGENT_VALUE)).toBe(false);

  // The section survives a reload; the Team section is one click away.
  await browser.reload();
  await expect(screen.getByRole('radio', 'Agent secrets')).toHaveAttribute('aria-checked', 'true');
  await screen.getByRole('radio', 'Team secrets').click();
  await expect(screen.getByText('Shared Gmail')).toBeVisible();
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
