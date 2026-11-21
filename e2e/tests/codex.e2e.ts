import { mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { test } from '@e2e-dev/web';
import { expect } from 'e2e';
import { axeViolations, failOnPageErrors, firstShown, mock } from './support.ts';

failOnPageErrors();

// Codex as a second harness (SPEC-503 CDX-010): Codex subscriptions in Accounts,
// Codex in the composer's model menu, and the agent of an automation's tasks.

type Mocked = {
  __yuliusMock: {
    tasks: { prompt: string; harness: string; model: string; effort: string; number: number }[];
    lastTaskCreate: { effort?: string };
    accounts: { label: string; provider: string; status: string }[];
    automations: { name: string; harness: string }[];
    askSuperagent(source: string, request: unknown): string;
    request(method: string, path: string, body: unknown): Promise<number>;
    release(): void;
  };
};

test('the mock routes a GPT model to Codex and refuses a conflicting harness', async ({ app, browser }) => {
  await app.open(mock('/s/studio/tasks', '&ambient=0'));
  const result = await browser.evaluate(async () => {
    const db = (window as unknown as Mocked).__yuliusMock;
    const body = { prompt: 'GPT routing', model: ' gpt-6.2-astra ', prepared: true };
    const path = '/api/sandboxes/sb_studio/tasks';
    const created = await db.request('POST', path, body);
    const task = db.tasks.find((t) => t.prompt === body.prompt);
    const refused = await db.request('POST', path, { ...body, harness: 'claude' });
    return { created, harness: task?.harness, model: task?.model, effort: task?.effort, refused };
  });
  expect(result).toEqual({ created: 201, harness: 'codex', model: 'gpt-6.2-astra', effort: 'high', refused: 422 });
});

test('an approved GPT request shows the explicit model in task details', async ({ app, browser }) => {
  await app.open(mock('/s/studio/tasks', '&ambient=0'));
  const { status, number } = await browser.evaluate(async () => {
    const db = (window as unknown as Mocked).__yuliusMock;
    const prompt = 'Model-only follow-up';
    const id = db.askSuperagent('sb_studio', { sandbox: 'product', prompt, options: { model: 'gpt-6.2-sol', prepared: true } });
    const status = await db.request('POST', `/api/sandboxes/sb_studio/superagent/requests/${id}`, { approve: true });
    return { status, number: db.tasks.find((t) => t.prompt === prompt)?.number };
  });
  expect(status).toBe(200);
  await browser.evaluate((n) => { location.hash = `/s/product/tasks/${n}`; }, number);
  await expect(browser.locator('.ti-kv dt:text-is("Model") + dd')).toHaveText('GPT-6.2 Sol · High effort (default)');
});

test('a Codex subscription is added with an OpenAI API key, without a sign-in', async ({ app, browser, screen }) => {
  await app.open(mock('/admin/accounts', '&ambient=0'));
  await screen.getByRole('button', 'Add subscription').first().click();
  await expect(screen.getByRole('dialog', 'Add a Claude subscription')).toBeVisible();
  // A segmented choice: its label is what is pressed.
  await browser.locator('dialog.modal .seg label:has-text("Codex")').click();
  const add = screen.getByRole('dialog', 'Add a Codex subscription');
  await expect(add.getByRole('radio', 'ChatGPT sign-in')).toBeChecked();
  // A pasted token is Claude's way in only (SPEC-503 CDX-015).
  await expect(add.getByRole('radio', 'Paste token')).toHaveCount(0);
  await browser.locator('dialog.modal .seg:not([hidden]) label:has-text("API key")').click();
  await expect(add.getByRole('radio', 'API key')).toBeChecked();
  await add.getByRole('textbox', /^Name/).fill('Codex key');
  expect(await axeViolations(browser)).toEqual([]);
  await add.getByRole('button', 'Connect').click();
  await expect(add.getByText('Paste the OpenAI API key.')).toBeVisible();
  await browser.locator('dialog.modal input[placeholder="sk-…"]').fill('sk-proj-SYNTHETIC0123456789abcdefghij');
  await add.getByRole('button', 'Connect').click();
  await expect(add).toBeHidden();
  await expect(browser.locator('.toast-ok .toast-msg:has-text("Codex key is connected")')).toBeVisible();
  await expect(browser.locator('main .card:has-text("Codex key")')).toBeVisible();
  const added = await browser.evaluate(() => (window as unknown as Mocked).__yuliusMock.accounts.find((a) => a.label === 'Codex key'));
  expect(added).toMatchObject({ provider: 'openai', status: 'ok' });
  // An API key has no usage windows (SPEC-503 CDX-008): Refresh says so in Codex's name.
  await screen.getByRole('button', 'Refresh usage of Codex key').click();
  await expect(browser.locator('.toast .toast-msg:has-text("usage reading for Codex key")')).toHaveText('Codex gave no new usage reading for Codex key.');
  await expect(browser.locator('main .account-card:has-text("Codex key")')).toContainText('Codex gave no new usage reading: this subscription’s usage comes from its running tasks.');
  await expect(browser.locator('main .account-card:has-text("Codex key")')).not.toContainText('Claude');
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
  await add.getByRole('button', 'Connect').click();
  await expect(add.getByText('Paste the auth.json or choose the file.')).toBeVisible();
  // An API-key file is not a ChatGPT sign-in: the server's reason shows, the dialog stays.
  await add.getByRole('textbox', 'auth.json').fill('{"auth_mode":"apikey","OPENAI_API_KEY":"sk-SYNTHETIC0123456789abcdef"}');
  await add.getByRole('button', 'Connect').click();
  await expect(add.getByText(/not a ChatGPT sign-in/)).toBeVisible();
  // A file without sign-in tokens is said as the server says it (SPEC-503 CDX-011).
  await add.getByRole('textbox', 'auth.json').fill('{"auth_mode":"chatgpt","tokens":{}}');
  await add.getByRole('button', 'Connect').click();
  await expect(add.getByText(/hold neither an API key nor sign-in tokens/)).toBeVisible();
  await add.getByRole('textbox', 'auth.json').fill(AUTH_JSON);
  await add.getByRole('button', 'Connect').click();
  await expect(add).toBeHidden();
  await expect(browser.locator('.toast-ok .toast-msg:has-text("Codex laptop is connected")')).toBeVisible();
  const added = await browser.evaluate(() => (window as unknown as Mocked).__yuliusMock.accounts.find((a) => a.label === 'Codex laptop'));
  expect(added).toMatchObject({ provider: 'openai', status: 'ok' });

  // A Codex card offers no token paste (SPEC-503 CDX-015).
  await expect(browser.locator('main .card:has-text("Codex laptop") button:has-text("Paste a new token")')).toHaveCount(0);
  // Signed in again on the laptop: the card imports the new file, chosen from disk.
  await browser.locator('main .card:has-text("Codex laptop")').getByRole('button', { name: 'Import auth.json' }).click();
  const reimport = screen.getByRole('dialog', 'Import an auth.json · Codex laptop');
  const dir = mkdtempSync(join(tmpdir(), 'codex-auth-'));
  const file = join(dir, 'auth.json');
  writeFileSync(file, AUTH_JSON);
  try {
    await browser.locator('dialog.modal input.auth-json-file').setInputFiles(file);
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
  await expect(reimport.getByText('auth.json is loaded.')).toBeVisible();
  // The file's content is never shown.
  await expect(reimport.getByRole('textbox', 'auth.json')).toHaveValue('');
  expect(await axeViolations(browser)).toEqual([]);
  await reimport.getByRole('button', 'Import').click();
  await expect(reimport).toBeHidden();
  await expect(browser.locator('.toast-ok .toast-msg:has-text("Codex laptop is signed in again")')).toBeVisible();
});

// SPEC-607: the Codex default inherits High and task details distinguish explicit choices.
for (const choice of [
  { label: 'Default', effort: undefined },
  { label: 'High', effort: 'high' },
  { label: 'Extra high', effort: 'xhigh' },
  { label: 'Max', effort: 'max' },
]) {
  test(`choosing ${choice.label} on Codex default records the effort in task details`, async ({ app, browser, screen }) => {
    await app.open(mock('/s/studio'));
    // A Codex task needs a Codex subscription (SPEC-246 CLA-002, SPEC-503).
    await browser.evaluate(() => (window as unknown as Mocked).__yuliusMock.accounts.push({ id: 'acc_codex', provider: 'openai', label: 'Codex #1', status: 'ok' } as never));
    await screen.getByRole('button', /^Model:/).click();
    await expect(screen.getByRole('menuitemradio', /^Codex/)).toContainText('GPT-6.2 Astra at high effort by default');
    await screen.getByRole('menuitemradio', /^Codex/).click();
    await expect(screen.getByRole('button', 'Model: Codex default')).toBeVisible();
    await screen.getByRole('button', /^Effort:/).click();
    await expect(screen.getByRole('menuitemradio', /^Default/)).toContainText('High');
    await screen.getByRole('menuitemradio', new RegExp('^' + choice.label)).click();
    const prompt = screen.getByRole('textbox', 'Task prompt');
    await prompt.fill('Add a health check');
    await prompt.press('ControlOrMeta+Enter');
    await expect(browser).toHaveURL(/#\/s\/studio\/t\/\d+$/);
    const task = await browser.evaluate(() => {
      const tasks = (window as unknown as Mocked).__yuliusMock.tasks;
      const last = tasks[tasks.length - 1];
      return { harness: last.harness, model: last.model, effort: last.effort, number: last.number };
    });
    const submitted = await browser.evaluate(() => (window as unknown as Mocked).__yuliusMock.lastTaskCreate);
    expect(submitted.effort).toBe(choice.effort);
    expect(task).toMatchObject({ harness: 'codex', model: 'gpt-6.2-astra', effort: choice.effort ?? 'high' });
    // Its details name the default as the server does (`model_default`).
    await app.open(mock(`/s/studio/tasks/${task.number}`));
    await firstShown(screen.getByRole('heading', { level: 2, name: 'Details' }));
    const model = () => browser.evaluate(() => {
      const dt = [...document.querySelectorAll('.ti-kv dt')].find((el) => (el as HTMLElement).innerText === 'Model');
      return dt ? (dt.nextElementSibling as HTMLElement).innerText : null;
    });
    await expect.poll(model).toBe(`GPT-6.2 Astra (default) · ${choice.effort ? choice.label + ' effort' : 'High effort (default)'}`);
  });
}

test('an automation chooses the agent of its tasks', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/automations/new', '&hold=/api/sandboxes/sb_studio/workflows'));
  const name = screen.getByRole('textbox', 'Automation name');
  await firstShown(name);
  // A name matching another recipe used to be replaced when the workflow read finished (#1416).
  const enteredName = 'Weekly summary';
  await name.fill(enteredName);
  await expect(screen.getByRole('button', 'Create automation')).toBeDisabled();
  await name.press('ControlOrMeta+s');
  await browser.evaluate(() => (window as unknown as Mocked).__yuliusMock.release());
  await expect(screen.getByRole('radio', /^Daily feedback/)).toHaveAttribute('aria-checked', 'true');
  await expect(name).toHaveValue(enteredName);
  await screen.getByRole('textbox', 'What the task does').fill('Review yesterday’s commits.');
  const agent = screen.getByRole('radiogroup', 'Agent');
  // Random is the default (SPEC-520 RHM-006).
  await expect(agent.getByRole('radio', 'Random')).toHaveAttribute('aria-checked', 'true');
  await agent.getByRole('radio', 'Codex').click();
  await expect(agent.getByRole('radio', 'Codex')).toHaveAttribute('aria-checked', 'true');
  expect(await axeViolations(browser)).toEqual([]);
  await screen.getByRole('button', 'Create automation').click();
  await expect.poll(() => browser.evaluate((savedName: string) => (window as unknown as Mocked).__yuliusMock.automations.find((a) => a.name === savedName)?.harness, enteredName)).toBe('codex');
});

type Touching = { __yuliusMock: Mocked['__yuliusMock'] & { touchTask: (sandbox: string, number: number, fields: Record<string, unknown>) => void } };

test('a Codex task runs only on Codex subscriptions (SPEC-503 CDX-013, SPEC-447 MOV-001)', async ({ app, browser, screen }) => {
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

// SPEC-503 CDX-013: what a Codex task says names Codex, never Claude: waiting for an account
// (its status and its line in the lists) and a start that failed for want of one.
test('a Codex task waiting for or failing without an account names Codex', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/t/12', '&ambient=0'));
  await browser.evaluate(() => (window as unknown as Touching).__yuliusMock.touchTask('sb_studio', 12, { harness: 'codex', status: 'waiting_capacity', activity: null, account_label: null, waits_for_account: true }));
  await expect(browser.locator('.strip-status')).toHaveText('Paused · waits for a Codex account');
  await expect(screen.getByRole('link', /, Waiting for a Codex account: none is usable/).first()).toBeVisible();
  await browser.evaluate(() => (window as unknown as Touching).__yuliusMock.touchTask('sb_studio', 12, { status: 'failed_no_account', waits_for_account: false }));
  await expect(browser.locator('main')).toContainText('The task couldn’t start because no Codex account was usable.');
  await expect(screen.getByText('Failed (no Codex account)').first()).toBeVisible();
});

// SPEC-503 CDX-010 (slice D review): the dialog's default Codex way is ChatGPT's device sign-in:
// it creates an openai subscription that waits for its sign-in, the terminal says what Codex
// asks, Enter connects it (the toast names Codex tasks), and a failed one says why.
test('a Codex subscription signs in with ChatGPT in the login terminal', async ({ app, browser, screen }) => {
  await app.open(mock('/admin/accounts', '&ambient=0'));
  await screen.getByRole('button', 'Add subscription').first().click();
  await browser.locator('dialog.modal .seg label:has-text("Codex")').click();
  const add = screen.getByRole('dialog', 'Add a Codex subscription');
  await add.getByRole('textbox', /^Name/).fill('Codex dev');
  await add.getByRole('button', 'Continue to sign-in').click();
  const dialog = screen.getByRole('dialog', 'Sign in · Codex dev');
  await expect(dialog.getByText(/^Finish Codex’s sign-in: open the link in the terminal below, sign in with ChatGPT/)).toBeVisible();
  await expect(browser.locator('.login-terminal')).toContainText('codex login --device-auth');
  await expect(browser.locator('.login-terminal')).not.toContainText('Claude');
  const waiting = await browser.evaluate(() => (window as unknown as Mocked).__yuliusMock.accounts.find((a) => a.label === 'Codex dev'));
  expect(waiting).toMatchObject({ provider: 'openai', status: 'pending_login' });
  const input = browser.locator('.login-terminal .xterm-helper-textarea');
  await expect(input).toBeFocused();
  await browser.keyboard.type('fail');
  await browser.keyboard.press('Enter');
  const outcome = dialog.getByRole('alert');
  await expect(outcome).toContainText('Sign-in failed');
  await expect(outcome).toContainText('The sign-in ended without producing a token');
  await dialog.getByRole('button', 'Try again').click();
  await expect(input).toBeFocused();
  await browser.keyboard.press('Enter');
  await expect(dialog).toBeHidden();
  await expect(browser.locator('.toast-ok .toast-msg:has-text("Codex dev is connected")')).toContainText('Codex tasks can use it now.');
  const signedIn = await browser.evaluate(() => (window as unknown as Mocked).__yuliusMock.accounts.find((a) => a.label === 'Codex dev'));
  expect(signedIn).toMatchObject({ provider: 'openai', status: 'ok' });
});

type Connecting = { __yuliusMock: Mocked['__yuliusMock'] & { connectCodex: () => void } };

// The composer offers the Codex dialog when a Codex task needs an account.
test('the composer offers Codex sign-in until a Codex account is usable', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio', '&claude=0'));
  await expect(browser.locator('.claude-connect')).toContainText('Tasks start once one is usable.');
  await screen.getByRole('button', /^Model:/).click();
  await screen.getByRole('menuitemradio', /^Codex/).click();
  await screen.getByRole('textbox', 'Task prompt').fill('Add a health check');
  await screen.getByRole('button', 'Start task').click();
  const toast = browser.locator('.toast-error');
  await expect(toast).toContainText('Connect a Codex account first');
  await toast.getByRole('button', { name: 'Connect Codex' }).click();
  await expect(screen.getByRole('dialog', 'Add a Codex subscription')).toBeVisible();
  await expect(screen.getByRole('radio', 'ChatGPT sign-in')).toBeChecked();

  await app.open(mock('/s/studio', '&claude=0'));
  await expect(browser.locator('.claude-connect')).toBeVisible();
  await browser.evaluate(() => (window as unknown as Connecting).__yuliusMock.connectCodex());
  await expect(browser.locator('.claude-connect')).toHaveCount(0);
});

type Pool = { __yuliusMock: { accounts: { provider?: string; label: string; status: string; usage: Record<string, unknown> }[] } };

// SPEC-503 (slice D review): a usable Codex subscription is no Claude capacity. With every Claude
// subscription out of Fable, the Status line still says Fable tasks run on the fallback, as the
// server places them.
test('a Codex subscription does not hide that Claude is out of Fable', async ({ app, browser }) => {
  await app.open(mock('/s/studio', '&ambient=0'));
  await browser.evaluate(() => {
    const db = (window as unknown as Pool).__yuliusMock;
    const resets_at = new Date(Date.now() + 40 * 3600e3).toISOString();
    const w = (used_percent: number) => ({ used_percent, resets_at, window_minutes: 10080 });
    db.accounts.forEach((account) => Object.assign(account, { status: 'limited', usage: { primary: w(10), secondary: w(20), weekly_fable: w(100), observed_at: new Date().toISOString() } }));
    db.accounts.push({ id: 'acc_codex', provider: 'openai', label: 'Codex #1', status: 'ok', usage: { primary: w(5), secondary: w(5), observed_at: new Date().toISOString() } } as never);
  });
  await browser.locator('.btn-status').click();
  await expect(browser.locator('.status-pop .status-health')).toHaveText('Fable 5.2 is used up on every subscription: tasks set to it run on Opus 5.5 until it can run again');
});

// SPEC-503 CDX-013 (slice D review): a Codex subscription's card names Codex for a limit it
// reported.
test('a Codex card names Codex for its limit', async ({ app, browser }) => {
  await app.open(mock('/s/studio', '&ambient=0'));
  await browser.evaluate(() => {
    const db = (window as unknown as Pool).__yuliusMock;
    db.accounts.push({ id: 'acc_codex', provider: 'openai', label: 'Codex #1', status: 'limited', email: null, usage: { primary: { used_percent: 40, resets_at: new Date(Date.now() + 3600e3).toISOString(), window_minutes: 300 }, secondary: null, limit_reached: true, limit_resets_at: null, observed_at: new Date().toISOString() } } as never);
  });
  await browser.evaluate(() => { location.hash = '#/admin/accounts'; });
  const card = browser.locator('main .account-card:has-text("Codex #1")');
  await expect(card).toContainText('Codex reported a usage limit without saying when it resets');
  await expect(card).not.toContainText('Claude');
});

// SPEC-503 CDX-013 (slice D review): a Codex task's terminal takes dropped files for Codex, and
// its details name its CLI as Codex.
test('a Codex task’s terminal and details name Codex', async ({ app, browser, screen }) => {
  // The terminal is drawn for the task's harness: the task is made a Codex one, then opened again.
  await app.open(mock('/s/studio/t/12'));
  await expect(browser.locator('.taskview-term .term')).toBeVisible();
  await browser.evaluate(() => (window as unknown as Touching).__yuliusMock.touchTask('sb_studio', 12, { harness: 'codex' }));
  await browser.evaluate(() => { location.hash = '#/s/studio'; });
  await expect(browser.locator('.taskview-term')).toHaveCount(0);
  await browser.evaluate(() => { location.hash = '#/s/studio/t/12'; });
  // The session is attached (and writable) once its screen shows.
  await expect(browser.locator('.taskview-term .xterm-rows')).toHaveText(/\S/);
  await browser.evaluate(() => {
    const target = document.querySelector('.taskview-term .term');
    const data = new DataTransfer();
    data.items.add(new File(['png'], 'shot.png', { type: 'image/png' }));
    for (const type of ['dragenter', 'dragover']) target?.dispatchEvent(new DragEvent(type, { bubbles: true, cancelable: true, composed: true, dataTransfer: data }));
  });
  await expect(browser.locator('.taskview-term .drop-overlay')).toHaveText('Drop to send to Codex');
  await browser.evaluate(() => { location.hash = '#/s/studio/tasks/12'; });
  await expect(screen.getByRole('heading', { level: 2, name: 'Details' })).toBeVisible();
  const labels = await browser.evaluate(() => [...document.querySelectorAll('.ti-kv dt')].map((el) => (el as HTMLElement).innerText));
  expect(labels).toContain('Codex');
  expect(labels).not.toContain('Claude Code');
});

type Automating = { __yuliusMock: { automations: { id: string; harness: string; last_run: { task_id: string } | null }[]; tasks: { id: string; harness: string; effort: string; effort_default: boolean }[] } };

// SPEC-503 CDX-010 (slice D review): Run now of a Codex automation creates a Codex task, as its
// scheduled runs do.
test('Run now of a Codex automation creates a Codex task', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/automations'));
  await browser.evaluate(() => { (window as unknown as Automating).__yuliusMock.automations.find((a) => a.id === 'aut_2')!.harness = 'codex'; });
  await browser.evaluate(() => { location.hash = '#/s/studio/automations/aut_2'; });
  await expect(screen.getByRole('radiogroup', 'Agent').getByRole('radio', 'Codex')).toHaveAttribute('aria-checked', 'true');
  await screen.getByRole('button', 'Run now').click();
  await expect(screen.getByText(/^task created \(#\d+\)/)).toBeVisible();
  const task = await browser.evaluate(() => {
    const db = (window as unknown as Automating).__yuliusMock;
    const run = db.automations.find((a) => a.id === 'aut_2')!.last_run!;
    return db.tasks.find((t) => t.id === run.task_id)!;
  });
  expect(task).toMatchObject({ harness: 'codex', effort: 'high', effort_default: true });
});

// SPEC-503 (slice D review): a split proposal is Claude's to make. With only Codex usable, a
// request the composer would split, started as a Codex task, is created as one Codex task
// instead of being refused for want of a Claude account.
test('a mixed Codex request with only Codex usable is created as one Codex task', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio', '&claude=0'));
  await browser.evaluate(() => (window as unknown as Connecting).__yuliusMock.connectCodex());
  await screen.getByRole('button', /^Model:/).click();
  await screen.getByRole('menuitemradio', /^Codex/).click();
  const prompt = screen.getByRole('textbox', 'Task prompt');
  await prompt.fill([
    'Finish the release before Friday, all of this has to land together please.',
    '1. Make the attention bar opaque over the terminal and keep its colours readable.',
    '2. Stop the tools rail pills from overflowing the rail on narrow windows.',
    '3. Route new tasks past exhausted accounts in the backend subscription pool.',
  ].join('\n'));
  await prompt.press('ControlOrMeta+Enter');
  await expect(browser).toHaveURL(/#\/s\/studio\/t\/\d+$/);
  await expect(browser.locator('.toast-error')).toHaveCount(0);
  const harness = await browser.evaluate(() => {
    const tasks = (window as unknown as Mocked).__yuliusMock.tasks;
    return tasks[tasks.length - 1].harness;
  });
  expect(harness).toBe('codex');
});
