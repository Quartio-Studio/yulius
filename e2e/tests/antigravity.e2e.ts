import { test } from '@e2e-dev/web';
import { expect } from 'e2e';
import { axeViolations, failOnPageErrors, mock } from './support.ts';

failOnPageErrors();

// Antigravity as a third harness (SPEC-505 AGY-010): Antigravity subscriptions by Gemini API key
// or Google sign-in, Gemini 4 Carbon in the composer's model menu (high effort by default), the
// agent of an automation's tasks, and an Antigravity task that names its own agent.

type Mocked = {
  __yuliusMock: {
    tasks: { harness: string; model: string; effort: string }[];
    accounts: { label: string; provider: string; status: string }[];
    automations: { name: string; harness: string }[];
    touchTask: (sandbox: string, number: number, fields: Record<string, unknown>) => void;
  };
};

test('an Antigravity subscription is added with a Gemini API key', async ({ app, browser, screen }) => {
  await app.open(mock('/admin/accounts', '&ambient=0'));
  await screen.getByRole('button', 'Add subscription').first().click();
  await browser.locator('dialog.modal .seg label:has-text("Antigravity")').click();
  const add = screen.getByRole('dialog', 'Add an Antigravity subscription');
  await expect(add.getByRole('radio', 'Google sign-in')).toBeChecked();
  // A pasted token is Claude's way in, an auth.json Codex's.
  await expect(add.getByRole('radio', 'Paste token')).toHaveCount(0);
  await expect(add.getByRole('radio', 'Import auth.json')).toHaveCount(0);
  await browser.locator('dialog.modal .seg label:has-text("Gemini API key")').click();
  await expect(add.getByRole('radio', 'Gemini API key')).toBeChecked();
  await add.getByRole('textbox', /^Name/).fill('Gemini key');
  expect(await axeViolations(browser)).toEqual([]);
  await add.getByRole('button', 'Connect').click();
  await expect(add.getByText('Paste the Gemini API key.')).toBeVisible();
  // Text that cannot be a key is refused as the server refuses it.
  await browser.locator('dialog.modal input[placeholder="AIza…"]').fill('short');
  await add.getByRole('button', 'Connect').click();
  await expect(add.getByText(/not a Gemini API key/)).toBeVisible();
  await browser.locator('dialog.modal input[placeholder="AIza…"]').fill('AIzaSYNTHETIC0123456789abcdefghij');
  await add.getByRole('button', 'Connect').click();
  await expect(add).toBeHidden();
  await expect(browser.locator('.toast-ok .toast-msg:has-text("Gemini key is connected")')).toContainText('Antigravity tasks can use it now.');
  const added = await browser.evaluate(() => (window as unknown as Mocked).__yuliusMock.accounts.find((a) => a.label === 'Gemini key'));
  expect(added).toMatchObject({ provider: 'google', status: 'ok' });
  // Its card offers no token paste or auth.json import, and speaks of Antigravity.
  const card = browser.locator('main .account-card:has-text("Gemini key")');
  await expect(browser.locator('main .card:has-text("Gemini key") button:has-text("Paste a new token")')).toHaveCount(0);
  await expect(browser.locator('main .card:has-text("Gemini key") button:has-text("Import auth.json")')).toHaveCount(0);
  await screen.getByRole('button', 'Refresh usage of Gemini key').click();
  await expect(card).toContainText('Antigravity gave no new usage reading: this subscription’s usage comes from its running tasks.');
  await expect(card).not.toContainText('Claude');
});

test('an Antigravity subscription signs in with Google in the login terminal', async ({ app, browser, screen }) => {
  await app.open(mock('/admin/accounts?add=antigravity', '&ambient=0'));
  const add = screen.getByRole('dialog', 'Add an Antigravity subscription');
  await expect(add.getByRole('radio', 'Google sign-in')).toBeChecked();
  await add.getByRole('textbox', /^Name/).fill('Antigravity dev');
  await add.getByRole('button', 'Continue to sign-in').click();
  const dialog = screen.getByRole('dialog', 'Sign in · Antigravity dev');
  await expect(dialog.getByText(/^Finish Antigravity’s sign-in: choose Google in the terminal below/)).toBeVisible();
  const terminal = browser.locator('.login-terminal');
  await expect(terminal).toContainText('Sign in with Google');
  await expect(terminal).not.toContainText('Claude');
  // The 704-character link stays on one row of the 1024-column terminal.
  const rows = await browser.evaluate(() => [...document.querySelectorAll('.login-terminal .xterm-rows > div')].map((row) => row.textContent || ''));
  const link = rows.find((row) => row.includes('https://accounts.google.com/'));
  expect(link && link.trim().length).toBe(704);
  const waiting = await browser.evaluate(() => (window as unknown as Mocked).__yuliusMock.accounts.find((a) => a.label === 'Antigravity dev'));
  expect(waiting).toMatchObject({ provider: 'google', status: 'pending_login' });
  await expect(browser.locator('.login-terminal .xterm-helper-textarea')).toBeFocused();
  await browser.keyboard.press('Enter');
  await expect(dialog).toBeHidden();
  await expect(browser.locator('.toast-ok .toast-msg:has-text("Antigravity dev is connected")')).toContainText('Antigravity tasks can use it now.');
});

test('choosing Gemini 4 Carbon in the composer starts an Antigravity task at high effort', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio'));
  await browser.evaluate(() => (window as unknown as Mocked).__yuliusMock.accounts.push({ id: 'acc_google', provider: 'google', label: 'Antigravity #1', status: 'ok' } as never));
  await screen.getByRole('button', /^Model:/).click();
  await expect(screen.getByText('Google', { exact: true })).toBeVisible();
  await screen.getByRole('menuitemradio', /^Gemini 4 Carbon/).click();
  await expect(screen.getByRole('button', 'Model: Gemini 4 Carbon')).toBeVisible();
  const prompt = screen.getByRole('textbox', 'Task prompt');
  await prompt.fill('Add a health check');
  await prompt.press('ControlOrMeta+Enter');
  await expect(browser).toHaveURL(/#\/s\/studio\/t\/\d+$/);
  const task = await browser.evaluate(() => {
    const tasks = (window as unknown as Mocked).__yuliusMock.tasks;
    const last = tasks[tasks.length - 1];
    return { harness: last.harness, model: last.model, effort: last.effort };
  });
  expect(task).toEqual({ harness: 'antigravity', model: 'gemini-4-carbon', effort: 'high' });
});

test('an automation can run its tasks on Antigravity', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/automations/new'));
  await screen.getByRole('textbox', 'Automation name').fill('Nightly Gemini review');
  await screen.getByRole('textbox', 'What the task does').fill('Review yesterday’s commits.');
  const agent = screen.getByRole('radiogroup', 'Agent');
  await agent.getByRole('radio', 'Antigravity').click();
  await expect(agent.getByRole('radio', 'Antigravity')).toHaveAttribute('aria-checked', 'true');
  expect(await axeViolations(browser)).toEqual([]);
  await screen.getByRole('button', 'Create automation').click();
  await expect.poll(() => browser.evaluate(() => (window as unknown as Mocked).__yuliusMock.automations.find((a) => a.name === 'Nightly Gemini review')?.harness)).toBe('antigravity');
});

test('an Antigravity task runs only on Antigravity subscriptions and names its agent', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/t/12', '&ambient=0'));
  await browser.evaluate(() => {
    const m = (window as unknown as Mocked).__yuliusMock;
    m.accounts.push({ id: 'acc_google_1', provider: 'google', label: 'Antigravity #1', status: 'ok' } as never, { id: 'acc_google_2', provider: 'google', label: 'Antigravity #2', status: 'ok' } as never);
    m.touchTask('sb_studio', 12, { harness: 'antigravity', account_label: 'Antigravity #1', account_id: 'acc_google_1' });
  });
  const chip = browser.locator('.taskview .stepstrip-main button.account-chip');
  await expect(chip).toHaveText('Antigravity #1');
  await chip.click();
  await expect(screen.getByRole('menuitem', /^Antigravity #2/)).toBeVisible();
  await expect(screen.getByRole('menuitem', /^Claude #/)).toHaveCount(0);
  await screen.getByRole('menuitem', /^Antigravity #2/).click();
  await expect(chip).toHaveText('Antigravity #2');
});

// What an Antigravity task says names Antigravity, with its article: waiting for an account and
// a start that failed for want of one.
test('an Antigravity task waiting for or failing without an account names Antigravity', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/t/12', '&ambient=0'));
  await browser.evaluate(() => (window as unknown as Mocked).__yuliusMock.touchTask('sb_studio', 12, { harness: 'antigravity' }));
  await browser.evaluate(() => (window as unknown as Mocked).__yuliusMock.touchTask('sb_studio', 12, { status: 'waiting_capacity', activity: null, account_label: null, waits_for_account: true }));
  await expect(browser.locator('.strip-status')).toHaveText('Paused · waits for an Antigravity account');
  await browser.evaluate(() => (window as unknown as Mocked).__yuliusMock.touchTask('sb_studio', 12, { status: 'failed_no_account', waits_for_account: false }));
  await expect(browser.locator('main')).toContainText('The task couldn’t start because no Antigravity account was usable.');
  await expect(screen.getByText('Failed (no Antigravity account)').first()).toBeVisible();
});
