import { test } from '@e2e-dev/web';
import { expect } from 'e2e';
import { axeViolations, failOnPageErrors, mock } from './support.ts';

failOnPageErrors();

// SPEC-247 RMT-001, RMT-040: Settings → Connect your local agent makes a personal API token, shows
// it once with the commands that use it, lists tokens with scope and last use, and revokes them.
// The mock's tokens are not real credentials.

const ORIGIN_COMMAND = /^claude mcp add --transport http yulius https?:\/\/[^ ]+\/mcp --header "Authorization: Bearer <token>"$/;

test('a token is made, shown once with its commands, listed and revoked', async ({ app, browser, screen }) => {
  await app.open(mock('/settings'));
  await expect(screen.getByRole('heading', 'Connect your local agent')).toBeVisible();
  const example = await browser.evaluate(() => document.querySelector('[aria-label="example Claude Code command"]')?.textContent ?? null);
  expect(example).toMatch(ORIGIN_COMMAND);
  // The CLI's own token is listed as such, never with its value.
  await expect(browser.locator('.agent-token:has-text("yulius CLI")')).toContainText('yulius login');

  // A name is needed first.
  await screen.getByRole('button', 'Create token').click();
  await expect(browser.locator('.agent-created-box')).toHaveCount(0);

  await screen.getByLabel('Token name').fill('laptop Claude Code');
  await screen.getByRole('button', 'Create token').click();
  await expect(browser.locator('.agent-created-box')).toContainText('the token is shown only this once');
  const shown = await browser.evaluate(() => ({
    token: document.querySelector('[aria-label="token"]')?.textContent ?? null,
    claude: document.querySelector('[aria-label="Claude Code command"]')?.textContent ?? null,
    cli: document.querySelector('[aria-label="CLI command"]')?.textContent ?? null,
  }));
  expect(shown.token).toMatch(/^yul_mock\d+$/);
  expect(shown.claude).toContain(`--header "Authorization: Bearer ${shown.token}"`);
  expect(shown.cli).toMatch(/^yulius remote login https?:\/\//);
  await expect(browser.locator('button[aria-label="Copy Claude Code command"]')).toBeVisible();
  const row = browser.locator('.agent-token:has-text("laptop Claude Code")');
  await expect(row).toContainText('Operate');
  await expect(row).toContainText('never used');
  expect(await axeViolations(browser)).toEqual([]);

  await screen.getByRole('button', 'Revoke laptop Claude Code').click();
  const dialog = screen.getByRole('dialog', 'Revoke “laptop Claude Code”?');
  await expect(dialog).toBeVisible();
  await browser.locator('dialog.modal button:has-text("Revoke")').click();
  await expect(browser.locator('.agent-token:has-text("laptop Claude Code")')).toHaveCount(0);
  await expect(browser.locator('.agent-created-box')).toHaveCount(0);
});

test('a read-only token says what it may do', async ({ app, browser, screen }) => {
  await app.open(mock('/settings'));
  await screen.getByLabel('Token name').fill('dashboards');
  await browser.locator('label.field:has(> .field-label:text-is("Scope")) select').selectOption('Read only');
  await expect(browser.locator('label.field:has(> .field-label:text-is("Scope")) .field-hint')).toHaveText('Lists, tasks, transcripts, CI and files; changes nothing');
  await screen.getByRole('button', 'Create token').click();
  await expect(browser.locator('.agent-token:has-text("dashboards")')).toContainText('Read only');
});
