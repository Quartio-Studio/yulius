import { mkdtempSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { test } from '@e2e-dev/web';
import { expect } from 'e2e';
import { axeViolations, failOnPageErrors, mock } from './support.ts';

failOnPageErrors();

// Codex as a second harness (SPEC-451 CDX-010): Codex subscriptions in Accounts,
// Codex in the composer's model menu, and the agent of an automation's tasks.

type Mocked = {
  __yuliusMock: {
    tasks: { harness: string }[];
    accounts: { label: string; provider: string; status: string }[];
    automations: { name: string; harness: string }[];
  };
};

test('a Codex subscription is added with an OpenAI API key, without a sign-in', async ({ app, browser, screen }) => {
  await app.open(mock('/admin/accounts', '&ambient=0'));
  await screen.getByRole('button', 'Add subscription').first().click();
  await expect(screen.getByRole('dialog', 'Add a Claude subscription')).toBeVisible();
  // A segmented choice: its label is what is pressed.
  await browser.locator('dialog.modal .seg label:has-text("Codex")').click();
  const add = screen.getByRole('dialog', 'Add a Codex subscription');
  await expect(add.getByRole('radio', 'ChatGPT sign-in')).toBeChecked();
  await browser.locator('dialog.modal .seg label:has-text("API key")').click();
  await expect(add.getByRole('radio', 'API key')).toBeChecked();
  await add.getByRole('textbox', /^Name/).fill('Codex key');
  expect(await axeViolations(browser)).toEqual([]);
  await add.getByRole('button', 'Add subscription').click();
  await expect(add.getByText('Paste the OpenAI API key.')).toBeVisible();
  await browser.locator('dialog.modal input[type=password]').fill('sk-proj-SYNTHETIC0123456789abcdefghij');
  await add.getByRole('button', 'Add subscription').click();
  await expect(add).toBeHidden();
  await expect(browser.locator('.toast-ok .toast-msg:has-text("Codex key is connected")')).toBeVisible();
  await expect(browser.locator('main .card:has-text("Codex key")')).toBeVisible();
  const added = await browser.evaluate(() => (window as unknown as Mocked).__yuliusMock.accounts.find((a) => a.label === 'Codex key'));
  expect(added).toMatchObject({ provider: 'openai', status: 'ok' });
});

// A ChatGPT auth.json as `codex login` writes it (synthetic).
const AUTH_JSON = JSON.stringify({ auth_mode: 'chatgpt', OPENAI_API_KEY: null, tokens: { id_token: 'h.e30.s', access_token: 'SYNTHETIC-access', refresh_token: 'SYNTHETIC-refresh', account_id: 'acc-synthetic' }, last_refresh: '2026-11-09T10:00:00Z' });

test('a Codex subscription is added by importing an auth.json, pasted or from a file', async ({ app, browser, screen }) => {
  await app.open(mock('/admin/accounts', '&ambient=0'));
  await screen.getByRole('button', 'Add subscription').first().click();
  await browser.locator('dialog.modal .seg label:has-text("Codex")').click();
  const add = screen.getByRole('dialog', 'Add a Codex subscription');
  await browser.locator('dialog.modal .seg label:has-text("Import auth.json")').click();
  await expect(add.getByRole('radio', 'Import auth.json')).toBeChecked();
  // The note says why the file should come from a separate Codex home.
  await expect(add.getByText(/separate Codex home \(CODEX_HOME=~\/codex-yulius codex login\)/)).toBeVisible();
  await add.getByRole('textbox', /^Name/).fill('Codex laptop');
  expect(await axeViolations(browser)).toEqual([]);
  await add.getByRole('button', 'Add subscription').click();
  await expect(add.getByText('Paste the auth.json or choose the file.')).toBeVisible();
  // An API-key file is not a ChatGPT sign-in: the server's reason shows, the dialog stays.
  await add.getByRole('textbox', 'auth.json').fill('{"auth_mode":"apikey","OPENAI_API_KEY":"sk-SYNTHETIC0123456789abcdef"}');
  await add.getByRole('button', 'Add subscription').click();
  await expect(add.getByText(/not a ChatGPT sign-in/)).toBeVisible();
  await add.getByRole('textbox', 'auth.json').fill(AUTH_JSON);
  await add.getByRole('button', 'Add subscription').click();
  await expect(add).toBeHidden();
  await expect(browser.locator('.toast-ok .toast-msg:has-text("Codex laptop is connected")')).toBeVisible();
  const added = await browser.evaluate(() => (window as unknown as Mocked).__yuliusMock.accounts.find((a) => a.label === 'Codex laptop'));
  expect(added).toMatchObject({ provider: 'openai', status: 'ok' });

  // Signed in again on the laptop: the card imports the new file, chosen from disk.
  await browser.locator('main .card:has-text("Codex laptop")').getByRole('button', { name: 'Import auth.json' }).click();
  const reimport = screen.getByRole('dialog', 'Import an auth.json into Codex laptop');
  const file = join(mkdtempSync(join(tmpdir(), 'codex-auth-')), 'auth.json');
  writeFileSync(file, AUTH_JSON);
  await browser.locator('dialog.modal input.auth-json-file').setInputFiles(file);
  await expect(reimport.getByText('auth.json is loaded.')).toBeVisible();
  // The file's content is never shown.
  await expect(reimport.getByRole('textbox', 'auth.json')).toHaveValue('');
  expect(await axeViolations(browser)).toEqual([]);
  await reimport.getByRole('button', 'Import').click();
  await expect(reimport).toBeHidden();
  await expect(browser.locator('.toast-ok .toast-msg:has-text("Codex laptop is signed in again")')).toBeVisible();
});

test('choosing Codex in the composer starts a Codex task', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio'));
  // A Codex task needs a Codex subscription (SPEC-246 CLA-002, SPEC-451).
  await browser.evaluate(() => (window as unknown as Mocked).__yuliusMock.accounts.push({ id: 'acc_codex', provider: 'openai', label: 'Codex #1', status: 'ok' } as never));
  await screen.getByRole('button', /^Model:/).click();
  await screen.getByRole('menuitemradio', /^Codex/).click();
  await expect(screen.getByRole('button', 'Model: Codex')).toBeVisible();
  const prompt = screen.getByRole('textbox', 'Task prompt');
  await prompt.fill('Add a health check');
  await prompt.press('ControlOrMeta+Enter');
  await expect(browser).toHaveURL(/#\/s\/studio\/t\/\d+$/);
  const harness = await browser.evaluate(() => {
    const tasks = (window as unknown as Mocked).__yuliusMock.tasks;
    return tasks[tasks.length - 1].harness;
  });
  expect(harness).toBe('codex');
});

test('an automation chooses the agent of its tasks', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/automations/new'));
  await screen.getByRole('textbox', 'Automation name').fill('Nightly Codex review');
  await screen.getByRole('textbox', 'What the task does').fill('Review yesterday’s commits.');
  const agent = screen.getByRole('radiogroup', 'Agent');
  await expect(agent.getByRole('radio', 'Claude Code')).toHaveAttribute('aria-checked', 'true');
  await agent.getByRole('radio', 'Codex').click();
  await expect(agent.getByRole('radio', 'Codex')).toHaveAttribute('aria-checked', 'true');
  expect(await axeViolations(browser)).toEqual([]);
  await screen.getByRole('button', 'Create automation').click();
  await expect.poll(() => browser.evaluate(() => (window as unknown as Mocked).__yuliusMock.automations.find((a) => a.name === 'Nightly Codex review')?.harness)).toBe('codex');
});

type Touching = { __yuliusMock: Mocked['__yuliusMock'] & { touchTask: (sandbox: string, number: number, fields: Record<string, unknown>) => void } };

test('a Codex task runs only on Codex subscriptions (SPEC-451 CDX-013, SPEC-447 MOV-001)', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/t/12'));
  await browser.evaluate(() => {
    const m = (window as unknown as Touching).__yuliusMock;
    m.accounts.push({ id: 'acc_codex_1', provider: 'openai', label: 'Codex #1', status: 'ok' } as never, { id: 'acc_codex_2', provider: 'openai', label: 'Codex #2', status: 'ok' } as never);
    m.touchTask('sb_studio', 12, { harness: 'codex', account_label: 'Codex #1', account_id: 'acc_codex_1' });
  });
  const chip = browser.locator('.taskview .stepstrip-main button.account-chip');
  await expect(chip).toHaveText('Codex #1');
  await chip.click();
  await expect(screen.getByText('Run #12 on…')).toBeVisible();
  await expect(screen.getByRole('menuitem', /^Codex #1\s*Runs on it now/)).toHaveAttribute('aria-disabled', 'true');
  await expect(screen.getByRole('menuitem', /^Codex #2/)).toBeVisible();
  // A task never changes harness: no Claude subscription is offered.
  await expect(screen.getByRole('menuitem', /^Claude #/)).toHaveCount(0);
  expect(await axeViolations(browser)).toEqual([]);
  await screen.getByRole('menuitem', /^Codex #2/).click();
  await expect(screen.getByText('#12 moves to Codex #2…')).toBeVisible();
  await expect(chip).toHaveText('Codex #2');
});
