import { test } from '@e2e-dev/web';
import { expect } from 'e2e';
import { axeViolations, eventsConnected, failOnPageErrors, firstShown, mock, touchTask } from './support.ts';

failOnPageErrors();

// Antigravity as a third harness (SPEC-505 AGY-010): Antigravity subscriptions by Gemini API key
// or Google sign-in, Gemini 4 Carbon in the composer's model menu (high effort by default), the
// agent of an automation's tasks, and an Antigravity task that names its own agent.

type Mocked = {
  __yuliusMock: {
    tasks: { harness: string; harness_choice?: string; model: string; effort: string }[];
    harnessRolls: number[];
    accounts: { label: string; provider: string; status: string }[];
    automations: { name: string; harness: string }[];
    connectAntigravity: () => void;
  };
};

test('an Antigravity subscription is added with a Gemini API key', async ({ app, browser, screen }) => {
  await app.open(mock('/admin/accounts', '&ambient=0'));
  await firstShown(screen.getByRole('button', 'Add subscription').first());
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
  await expect(card).toBeVisible();
  await expect(browser.locator('main .card:has-text("Gemini key") button:has-text("Paste a new token")')).toHaveCount(0);
  await expect(browser.locator('main .card:has-text("Gemini key") button:has-text("Import auth.json")')).toHaveCount(0);
  await screen.getByRole('button', 'Refresh usage of Gemini key').click();
  await expect(card).toContainText('Antigravity gave no new usage reading: this subscription’s usage comes from its running tasks.');
  await expect(card).not.toContainText('Claude');
});

test('an Antigravity subscription signs in with Google in the login terminal', async ({ app, browser, screen }) => {
  await app.open(mock('/admin/accounts?add=antigravity', '&ambient=0'));
  const add = screen.getByRole('dialog', 'Add an Antigravity subscription');
  await firstShown(add);
  await expect(add.getByRole('radio', 'Google sign-in')).toBeChecked();
  await add.getByRole('textbox', /^Name/).fill('Antigravity dev');
  await add.getByRole('button', 'Continue to sign-in').click();
  const dialog = screen.getByRole('dialog', 'Sign in · Antigravity dev');
  await expect(dialog.getByText(/^Finish Antigravity’s sign-in: choose Google in the terminal below/)).toBeVisible();
  const terminal = browser.locator('.login-terminal');
  await expect(terminal).toContainText('Sign in with Google');
  await expect(terminal).not.toContainText('Claude');
  // The 704-character link stays on one row of the 1024-column terminal.
  await expect.poll(() => browser.evaluate(() => {
    const rows = [...document.querySelectorAll('.login-terminal .xterm-rows > div')].map((row) => (row.textContent || '').trim());
    return (rows.find((row) => row.startsWith('https://accounts.google.com/')) || '').length;
  })).toBe(704);
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
  await firstShown(screen.getByRole('button', /^Model:/));
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
  await firstShown(screen.getByRole('textbox', 'Automation name'));
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
  });
  await touchTask(browser, 'sb_studio', 12, { harness: 'antigravity', account_label: 'Antigravity #1', account_id: 'acc_google_1' });
  const chip = browser.locator('.taskview .stepstrip-main button.account-chip');
  await firstShown(chip, 'Antigravity #1');
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
  await touchTask(browser, 'sb_studio', 12, { harness: 'antigravity', status: 'waiting_capacity', activity: null, account_label: null, waits_for_account: true });
  await firstShown(browser.locator('.strip-status'), 'Paused · waits for an Antigravity account');
  // Its line in the lists and its time's tooltip say so too.
  await expect(screen.getByRole('link', /, Waiting for an Antigravity account: none is usable/).first()).toBeVisible();
  await expect(browser.locator('[title^="Waiting for an Antigravity account since"]').first()).toBeAttached();
  await touchTask(browser, 'sb_studio', 12, { status: 'failed_no_account', waits_for_account: false });
  await expect(browser.locator('main')).toContainText('The task couldn’t start because no Antigravity account was usable.');
  await expect(screen.getByText('Failed (no Antigravity account)').first()).toBeVisible();
});

// With no Claude account usable, the composer asks to connect Claude only until an Antigravity
// account is usable; an Antigravity start refused for want of one offers Connect Antigravity,
// which opens the Antigravity dialog.
test('the composer asks for Claude only while no Antigravity account is usable either', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio', '&claude=0'));
  await firstShown(browser.locator('.claude-connect'));
  await screen.getByRole('button', /^Model:/).click();
  await screen.getByRole('menuitemradio', /^Gemini 4 Carbon/).click();
  await screen.getByRole('textbox', 'Task prompt').fill('Add a health check');
  await screen.getByRole('button', 'Start task').click();
  const toast = browser.locator('.toast-error');
  await expect(toast).toContainText('Connect an Antigravity account first');
  await toast.getByRole('button', { name: 'Connect Antigravity' }).click();
  await expect(screen.getByRole('dialog', 'Add an Antigravity subscription')).toBeVisible();
  await expect(screen.getByRole('radio', 'Google sign-in')).toBeChecked();

  await app.open(mock('/s/studio', '&claude=0'));
  await firstShown(browser.locator('.claude-connect'));
  // The announcement reaches a page that hears announcements.
  await eventsConnected(browser);
  await browser.evaluate(() => (window as unknown as Mocked).__yuliusMock.connectAntigravity());
  await expect(browser.locator('.claude-connect')).toHaveCount(0);
});

// Only the agents' names open the Add dialog from the address: any other `add` opens nothing.
test('an unknown add in the address opens no dialog', async ({ app, browser, screen }) => {
  await app.open(mock('/admin/accounts?add=constructor', '&ambient=0'));
  await firstShown(screen.getByRole('button', 'Add subscription').first());
  await expect(browser.locator('dialog.modal')).toHaveCount(0);
});

// SPEC-505 RND-001/002: the composer starts with Random: a task started without choosing an agent
// is drawn among the usable ones (here Antigravity's roll) and its details say so; an automation's
// agent is Random by default.
test('a task started without choosing an agent is drawn, and says so', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio'));
  await browser.evaluate(() => {
    const m = (window as unknown as Mocked).__yuliusMock;
    m.accounts.push({ id: 'acc_google', provider: 'google', label: 'Antigravity #1', status: 'ok' } as never);
    m.harnessRolls.push(1);
  });
  await firstShown(screen.getByRole('button', 'Model: Random agent'));
  await screen.getByRole('button', /^Model:/).click();
  await expect(screen.getByRole('menuitemradio', /^Random/)).toHaveAttribute('aria-checked', 'true');
  await browser.keyboard.press('Escape');
  const prompt = screen.getByRole('textbox', 'Task prompt');
  await prompt.fill('Add a health check');
  await prompt.press('ControlOrMeta+Enter');
  await expect(browser).toHaveURL(/#\/s\/studio\/t\/\d+$/);
  const task = await browser.evaluate(() => {
    const tasks = (window as unknown as Mocked).__yuliusMock.tasks;
    const last = tasks[tasks.length - 1];
    return { harness: last.harness, choice: last.harness_choice };
  });
  expect(task).toEqual({ harness: 'antigravity', choice: 'random' });
  // Its overview (the same page, not a reload: the mock keeps the task) says it was drawn.
  await browser.evaluate(() => { location.hash = location.hash.replace('/t/', '/tasks/'); });
  await expect(browser.locator('main')).toContainText('Antigravity (drawn at random)');

  await app.open(mock('/s/studio/automations/new'));
  await firstShown(screen.getByRole('radiogroup', 'Agent'));
  await expect(screen.getByRole('radiogroup', 'Agent').getByRole('radio', 'Random')).toHaveAttribute('aria-checked', 'true');
});
