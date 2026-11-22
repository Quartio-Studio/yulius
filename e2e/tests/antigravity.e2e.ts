import { test } from '@e2e-dev/web';
import { expect } from 'e2e';
import { axeViolations, eventsConnected, failOnPageErrors, firstShown, mock, touchTask } from './support.ts';

failOnPageErrors();

// Antigravity as a third harness (SPEC-505 AGY-010, SPEC-519 NEO-004): Antigravity subscriptions
// by Gemini API key or Google sign-in, Gemini 4 Neon first and Gemini 4 Carbon second in the
// composer's model menu (high effort by default), the agent of an automation's tasks, and an
// Antigravity task that names its own agent.

type Mocked = {
  __yuliusMock: {
    tasks: { harness: string; harness_choice?: string; model: string; model_default: boolean; effort: string; effort_default: boolean }[];
    harnessRolls: number[];
    accounts: { id?: string; label: string; provider: string; status: string }[];
    automations: { name: string; harness: string }[];
    connectAntigravity: () => void;
    loginCancels: string[];
    logins: Map<string, { accountId: string; done: boolean; ws: { push: (frame: string) => void } | null }>;
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
  // The 704-character link stays on one row of the 1000-column terminal.
  await expect.poll(() => browser.evaluate(() => {
    const rows = [...document.querySelectorAll('.login-terminal .xterm-rows > div')].map((row) => (row.textContent || '').trim());
    return (rows.find((row) => row.startsWith('https://accounts.google.com/')) || '').length;
  })).toBe(704);
  const waiting = await browser.evaluate(() => (window as unknown as Mocked).__yuliusMock.accounts.find((a) => a.label === 'Antigravity dev'));
  expect(waiting).toMatchObject({ provider: 'google', status: 'pending_login' });
  await expect(browser.locator('.login-terminal .xterm-helper-textarea')).toBeFocused();
  await browser.keyboard.press('Enter');
  // The CLI's setup goes on after the code (SPEC-515, #1421): the window stays open until Done,
  // and says so as a status, which a screen reader announces with the focus in the terminal.
  await expect(dialog.getByText(/^Antigravity is signed in\. Finish the rest of its setup in the terminal below/)).toBeVisible();
  await expect(browser.locator('dialog [role="status"]:has-text("Antigravity is signed in")')).toBeVisible();
  expect(await axeViolations(browser)).toEqual([]);
  await expect(terminal).toContainText('Help improve Antigravity');
  const signedIn = await browser.evaluate(() => (window as unknown as Mocked).__yuliusMock.accounts.find((a) => a.label === 'Antigravity dev'));
  expect(signedIn).toMatchObject({ status: 'pending_login' });
  await browser.keyboard.type('n');
  await expect(terminal).toContainText('(y/N) *');
  await browser.keyboard.press('Enter');
  await expect(dialog.getByRole('button', 'Cancel sign-in')).toHaveCount(0);
  await dialog.getByRole('button', 'Done').click();
  await expect(dialog).toBeHidden();
  await expect(browser.locator('.toast-ok .toast-msg:has-text("Antigravity dev is connected")')).toContainText('Antigravity tasks can use it now.');
  expect(await browser.evaluate(() => (window as unknown as Mocked).__yuliusMock.loginCancels.length)).toBe(1);
});

test('Try again after a signed-in Antigravity sign-in was lost starts over and offers Done again', async ({ app, browser, screen }) => {
  await app.open(mock('/admin/accounts?add=antigravity', '&ambient=0'));
  const add = screen.getByRole('dialog', 'Add an Antigravity subscription');
  await firstShown(add);
  await add.getByRole('textbox', /^Name/).fill('Antigravity retry');
  await add.getByRole('button', 'Continue to sign-in').click();
  const dialog = screen.getByRole('dialog', 'Sign in · Antigravity retry');
  await expect(browser.locator('.login-terminal')).toContainText('Sign in with Google');
  await expect(browser.locator('.login-terminal .xterm-helper-textarea')).toBeFocused();
  await browser.keyboard.press('Enter');
  await expect(dialog.getByText(/^Antigravity is signed in\./)).toBeVisible();
  // The server lost this sign-in while its setup ran (a restart): its terminal is refused in band
  // with `not_found`, as the server refuses one it no longer has (SPEC-515).
  await browser.evaluate(() => {
    const db = (window as unknown as Mocked).__yuliusMock;
    const account = db.accounts.find((a) => a.label === 'Antigravity retry') as unknown as { id: string };
    for (const session of db.logins.values()) {
      if (session.accountId !== account.id || session.done) continue;
      session.done = true;
      if (session.ws) session.ws.push(JSON.stringify({ type: 'error', code: 'not_found', message: 'not found' }));
    }
  });
  await expect(dialog.getByText('This sign-in is no longer available')).toBeVisible();
  await dialog.getByRole('button', 'Try again').click();
  await expect(dialog.getByText(/^Finish Antigravity’s sign-in: choose Google in the terminal below/)).toBeVisible();
  await expect(dialog.getByText(/^Antigravity is signed in\./)).toHaveCount(0);
  await expect(dialog.getByRole('button', 'Cancel sign-in')).toBeVisible();
  await expect(browser.locator('.login-terminal')).toContainText('Sign in with Google');
  await expect(browser.locator('.login-terminal .xterm-helper-textarea')).toBeFocused();
  await browser.keyboard.press('Enter');
  await expect(dialog.getByText(/^Antigravity is signed in\./)).toBeVisible();
  await dialog.getByRole('button', 'Done').click();
  await expect(dialog).toBeHidden();
  await expect(browser.locator('.toast-ok .toast-msg:has-text("Antigravity retry is connected")')).toBeVisible();
});

test('choosing Gemini 4 Neon or Gemini 4 Carbon in the composer starts an Antigravity task at high effort', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio'));
  await browser.evaluate(() => (window as unknown as Mocked).__yuliusMock.accounts.push({ id: 'acc_google', provider: 'google', label: 'Antigravity #1', status: 'ok' } as never));
  await firstShown(screen.getByRole('button', /^Model:/));
  await screen.getByRole('button', /^Model:/).click();
  await expect(screen.getByText('Google', { exact: true })).toBeVisible();
  const googleModels = await browser.evaluate(() => {
    const group = [...document.querySelectorAll('.menu-group[role="group"]')].find(
      (el) => document.getElementById(el.getAttribute('aria-labelledby') || '')?.textContent?.trim() === 'Google',
    );
    return group ? [...group.querySelectorAll('.menu-label')].map((el) => el.textContent?.trim()) : [];
  });
  expect(googleModels.slice(0, 2)).toEqual(['Gemini 4 Neon', 'Gemini 4 Carbon']);
  await screen.getByRole('menuitemradio', /^Gemini 4 Neon/).click();
  await expect(screen.getByRole('button', 'Model: Gemini 4 Neon')).toBeVisible();
  const prompt = screen.getByRole('textbox', 'Task prompt');
  await prompt.fill('Add a health check');
  await prompt.press('ControlOrMeta+Enter');
  await expect(browser).toHaveURL(/#\/s\/studio\/t\/\d+$/);
  const neonTask = await browser.evaluate(() => {
    const tasks = (window as unknown as Mocked).__yuliusMock.tasks;
    const last = tasks[tasks.length - 1] as Mocked['__yuliusMock']['tasks'][number] & { number: number };
    return { harness: last.harness, model: last.model, effort: last.effort, number: last.number };
  });
  expect(neonTask).toMatchObject({ harness: 'antigravity', model: 'gemini-4-neon', effort: 'high' });
  await browser.evaluate((num) => { location.hash = `#/s/studio/tasks/${num}`; }, neonTask.number);
  await firstShown(screen.getByRole('heading', { level: 2, name: 'Details' }));
  await expect.poll(() => browser.evaluate(() => {
    const dt = [...document.querySelectorAll('.ti-kv dt')].find((el) => (el as HTMLElement).innerText === 'Model');
    return dt ? (dt.nextElementSibling as HTMLElement).innerText : null;
  })).toBe('Gemini 4 Neon · High effort (default)');

  // Gemini 4 Carbon stays selectable for tasks that ask for it.
  await browser.evaluate(() => { location.hash = '#/s/studio'; });
  await firstShown(screen.getByRole('button', 'Model: Gemini 4 Neon'));
  await screen.getByRole('button', /^Model:/).click();
  await screen.getByRole('menuitemradio', /^Gemini 4 Carbon/).click();
  await expect(screen.getByRole('button', 'Model: Gemini 4 Carbon')).toBeVisible();
  await screen.getByRole('textbox', 'Task prompt').fill('Check Carbon');
  await screen.getByRole('textbox', 'Task prompt').press('ControlOrMeta+Enter');
  await expect(browser).toHaveURL(/#\/s\/studio\/t\/\d+$/);
  const carbonTask = await browser.evaluate(() => {
    const tasks = (window as unknown as Mocked).__yuliusMock.tasks;
    const last = tasks[tasks.length - 1];
    return { harness: last.harness, model: last.model, effort: last.effort };
  });
  expect(carbonTask).toEqual({ harness: 'antigravity', model: 'gemini-4-carbon', effort: 'high' });
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

// With no accounts, changing the model updates the connection prompt and the refused start.
test('an Antigravity choice offers its sign-in until an account is usable', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio', '&claude=0'));
  await firstShown(browser.locator('.claude-connect'));
  await screen.getByRole('button', /^Model:/).click();
  await screen.getByRole('menuitemradio', /^Gemini 4 Carbon/).click();
  await expect(screen.getByRole('link', 'Connect Antigravity')).toHaveAttribute('href', /\/admin\/accounts\?add=antigravity$/);
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

// SPEC-520 RHM-001/002/006: selecting Random in the composer persists in Settings and draws
// uniformly across all harnesses (`claude`, `codex`, `antigravity`) that have non-exhausted
// subscriptions regardless of subscription count, skipping finished subscriptions, and showing
// "(drawn at random)" in the task overview; an automation's agent starts on Random (`auto`).
test('a task started on Random draws across available harnesses, and says so', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio'));
  await browser.evaluate(() => {
    const m = (window as unknown as Mocked).__yuliusMock;
    m.accounts.push(
      { id: 'acc_codex', provider: 'openai', label: 'Codex #1', status: 'ok' },
      { id: 'acc_google', provider: 'google', label: 'Antigravity #1', status: 'ok' },
    );
    // Even with multiple Claude subscriptions in the pool, candidate harnesses are
    // [claude, codex, antigravity]: roll 1 picks codex, roll 2 picks antigravity.
    m.harnessRolls.push(1, 2);
  });
  await firstShown(screen.getByRole('button', 'Model: Codex default'));
  await screen.getByRole('button', /^Model:/).click();
  await screen.getByRole('menuitemradio', /^Random/).click();
  await screen.getByRole('button', /^Model:/).click();
  await expect(screen.getByRole('menuitemradio', /^Random/)).toHaveAttribute('aria-checked', 'true');
  await browser.keyboard.press('Escape');
  const prompt = screen.getByRole('textbox', 'Task prompt');
  await prompt.fill('Add a health check');
  await prompt.press('ControlOrMeta+Enter');
  await expect(browser).toHaveURL(/#\/s\/studio\/t\/\d+$/);
  const first = await browser.evaluate(() => {
    const tasks = (window as unknown as Mocked).__yuliusMock.tasks;
    const last = tasks[tasks.length - 1];
    return { harness: last.harness, choice: last.harness_choice };
  });
  expect(first).toEqual({ harness: 'codex', choice: 'random' });

  await browser.evaluate(() => { location.hash = '#/s/studio'; });
  await firstShown(screen.getByRole('button', 'Model: Random agent'));
  await screen.getByRole('textbox', 'Task prompt').fill('Wire up metrics');
  await screen.getByRole('textbox', 'Task prompt').press('ControlOrMeta+Enter');
  await expect(browser).toHaveURL(/#\/s\/studio\/t\/\d+$/);
  const second = await browser.evaluate(() => {
    const tasks = (window as unknown as Mocked).__yuliusMock.tasks;
    const last = tasks[tasks.length - 1];
    return {
      harness: last.harness, choice: last.harness_choice,
      model: last.model, modelDefault: last.model_default,
      effort: last.effort, effortDefault: last.effort_default,
    };
  });
  expect(second).toEqual({
    harness: 'antigravity', choice: 'random',
    model: 'gemini-4-neon', modelDefault: true, effort: 'high', effortDefault: true,
  });
  await browser.evaluate(() => { location.hash = location.hash.replace('/t/', '/tasks/'); });
  await expect(browser.locator('main')).toContainText('Antigravity (drawn at random)');

  // When Codex's sole subscription is finished (bare `limited`), Codex leaves the draw:
  // candidates become [claude, antigravity], so roll 1 picks antigravity instead of codex.
  await browser.evaluate(() => {
    const m = (window as unknown as Mocked).__yuliusMock;
    const codex = m.accounts.find((a) => a.id === 'acc_codex');
    if (codex) codex.status = 'limited';
    m.harnessRolls.push(1);
    location.hash = '#/s/studio';
  });
  await firstShown(screen.getByRole('button', 'Model: Random agent'));
  await screen.getByRole('textbox', 'Task prompt').fill('Skip finished codex');
  await screen.getByRole('textbox', 'Task prompt').press('ControlOrMeta+Enter');
  await expect(browser).toHaveURL(/#\/s\/studio\/t\/\d+$/);
  const third = await browser.evaluate(() => {
    const tasks = (window as unknown as Mocked).__yuliusMock.tasks;
    const last = tasks[tasks.length - 1];
    return { harness: last.harness, choice: last.harness_choice };
  });
  expect(third).toEqual({ harness: 'antigravity', choice: 'random' });

  await app.open(mock('/settings'));
  await firstShown(screen.getByRole('button', 'Model for new tasks: Random agent'));

  await app.open(mock('/s/studio/automations/new'));
  const agent = screen.getByRole('radiogroup', 'Agent');
  await firstShown(agent);
  await expect(agent.getByRole('radio', 'Random')).toHaveAttribute('aria-checked', 'true');
});

// SPEC-520 RHM-006: a split proposal is Claude's to make. With only Antigravity usable, a request
// the composer would split, started on Random, is created as one task drawn to Antigravity instead
// of being refused for want of a Claude account.
test('a mixed request on Random with only Antigravity usable is created as one drawn task', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio', '&claude=0'));
  await firstShown(browser.locator('.claude-connect'));
  await eventsConnected(browser);
  await browser.evaluate(() => (window as unknown as Mocked).__yuliusMock.connectAntigravity());
  await expect(browser.locator('.claude-connect')).toHaveCount(0);
  await screen.getByRole('button', /^Model:/).click();
  await screen.getByRole('menuitemradio', /^Random/).click();
  await expect(screen.getByRole('button', 'Model: Random agent')).toBeVisible();
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
  const task = await browser.evaluate(() => {
    const tasks = (window as unknown as Mocked).__yuliusMock.tasks;
    const last = tasks[tasks.length - 1];
    return { harness: last.harness, choice: last.harness_choice };
  });
  expect(task).toEqual({ harness: 'antigravity', choice: 'random' });
});
