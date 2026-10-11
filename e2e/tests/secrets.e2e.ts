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
