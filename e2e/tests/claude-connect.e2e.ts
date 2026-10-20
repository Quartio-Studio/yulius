import { test, type Browser } from '@e2e-dev/web';
import { expect } from 'e2e';
import { axeViolations, failOnPageErrors, mock, promptText } from './support.ts';

failOnPageErrors();

type Mocked = Window & { __yuliusMock: { connectClaude(): void } };

/** An administrator connects a Claude account elsewhere; the page hears `account.updated`. */
const connectClaude = (browser: Browser) => browser.evaluate(() => { (window as unknown as Mocked).__yuliusMock.connectClaude(); });

// SPEC-246 (task #232), an early user: "probably because I didn't connect Claude and the first job
// failed. I'd make an onboarding flow that requires connecting Claude."
test('a fresh instance asks to connect Claude before anything that runs an agent', async ({ app, browser, screen }) => {
  await app.open(mock('/', '&sandboxes=0&claude=0'));

  await expect(screen.getByRole('heading', { level: 1 })).toHaveText('Try Yulius on your own code');
  const step = browser.locator('.claude-connect');
  await expect(step).toHaveText('Connect Claude first. Agents run on a Claude subscription, and none is usable yet: connect one, or sign one in again. Playgrounds and tasks start once one is usable. Connect Claude');
  await expect(step.getByRole('link', { name: 'Connect Claude' })).toHaveAttribute('href', /\/admin\/accounts\?add=1$/);
  // Nothing that would start an agent is offered before it.
  await expect(screen.getByRole('group', 'Project folder')).toHaveCount(0);
  await expect(screen.getByRole('button', 'Start from a template')).toHaveCount(0);
  await expect(screen.getByRole('button', 'Import repository')).toHaveCount(0);
  expect(await axeViolations(browser)).toEqual([]);

  // Connected (here in another tab): the first steps appear without a reload.
  await connectClaude(browser);
  await expect(screen.getByRole('group', 'Project folder')).toBeVisible();
  await expect(step).toHaveCount(0);
  await expect(screen.getByRole('button', 'Import repository')).toBeEnabled();
});

test('Connect Claude opens the subscription sign-in', async ({ app, browser, screen }) => {
  await app.open(mock('/', '&sandboxes=0&claude=0'));
  await browser.locator('.claude-connect').getByRole('link', { name: 'Connect Claude' }).click();
  // Subscriptions, with its sign-in open (the address drops `?add=1` once it is).
  await expect(screen.getByRole('dialog', 'Add a Claude subscription')).toBeVisible();
  await expect(browser).toHaveURL(/#\/admin\/accounts$/);
});

test('with sandboxes, Home and the composer say so first and a refused start links to the sign-in', async ({ app, browser, screen }) => {
  await app.open(mock('/', '&claude=0'));
  await expect(screen.getByRole('heading', { level: 1 })).toHaveText('Sandboxes');
  await expect(browser.locator('.claude-connect')).toContainText('Connect Claude first.');
  await expect(screen.getByRole('button', 'New playground')).toBeDisabled();
  await expect(screen.getByRole('button', 'Import repository')).toBeDisabled();

  await app.open(mock('/s/studio', '&claude=0'));
  await expect(browser.locator('.claude-connect')).toContainText('Tasks start once one is usable.');
  await screen.getByRole('textbox', 'Task prompt').fill('Write the checkout UI tests');
  await screen.getByRole('button', 'Start task').click();
  // The server refuses before anything exists (CLA-002); the toast offers the way out.
  const toast = browser.locator('.toast-error');
  await expect(toast).toContainText('Connect a Claude account first');
  await toast.getByRole('button', { name: 'Connect Claude' }).click();
  await expect(screen.getByRole('dialog', 'Add a Claude subscription')).toBeVisible();
});

test('onboarding waits for Claude and a start that failed for it is offered again', async ({ app, browser, screen }) => {
  await app.open(mock('/s/product', '&claude=0&onboarding=failed'));
  const gate = screen.getByRole('region', 'Onboard product first');
  await expect(browser.locator('.onboarding-gate .claude-connect')).toContainText('Onboarding starts once one is usable.');
  await expect(gate).toContainText('Onboarding couldn’t start because no Claude account was connected.');
  await expect(gate.getByRole('button', 'Retry onboarding')).toBeDisabled();
  expect(await axeViolations(browser)).toEqual([]);

  await connectClaude(browser);
  await expect(gate).toContainText('One is connected now.');
  await expect(browser.locator('.onboarding-gate .claude-connect')).toHaveCount(0);
  await gate.getByRole('button', 'Retry onboarding').click();
  // It started: the page opens the onboarding task, where its session runs (SPEC-111).
  await expect(browser).toHaveURL(/#\/s\/product\/t\/\d+$/);
});

// The review of SPEC-246 (CLA-004): a task started by itself while no account is connected waits
// for one, and says so wherever it is shown, not "no subscription has capacity left".
test('a task waiting with no Claude account says it waits for one, not for capacity', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/t/7', '&claude=0'));
  await expect(screen.getByRole('link', /Search indexing\s*, Waiting for a Claude account: none is usable/)).toBeVisible();
  await expect(browser.locator('.strip-status')).toHaveText('Paused · waits for a Claude account');
  await expect(screen.getByText(/no subscription has capacity left|resets ~/)).toHaveCount(0);
});

// The review of SPEC-246 (rounds 2 and 3): a request the composer would split asks Claude for a
// proposal first; without an account that is refused too, and said once, with the prompt kept.
const MIXED = [
  'Finish the release before Friday, all of this has to land together please.',
  '1. Make the attention bar opaque over the terminal and keep its colours readable.',
  '2. Stop the tools rail pills from overflowing the rail on narrow windows.',
  '3. Route new tasks past exhausted accounts in the backend subscription pool.',
  '4. Make the server mark tasks Done reliably once the pull request is tested.',
].join('\n');

test('a mixed request without a Claude account is refused, not "created as one task"', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio', '&claude=0'));
  await browser.evaluate(() => {
    const mocked = window as unknown as { __yuliusMock: { holdProposal?: Promise<void>; releaseProposal?: () => void } };
    mocked.__yuliusMock.holdProposal = new Promise<void>((resolve) => { mocked.__yuliusMock.releaseProposal = resolve; });
  });
  const tasksBefore = await browser.evaluate(() => (window as unknown as { __yuliusMock: { tasks: unknown[] } }).__yuliusMock.tasks.length);
  const prompt = screen.getByRole('textbox', 'Task prompt');
  await prompt.fill(MIXED);
  await prompt.press('ControlOrMeta+Enter');
  // It is the split proposal that is asked first, and refused.
  await expect(screen.getByRole('region', 'Split proposal')).toHaveText(/Splitting your request…/);
  await browser.evaluate(() => (window as unknown as { __yuliusMock: { releaseProposal: () => void } }).__yuliusMock.releaseProposal());
  const toast = browser.locator('.toast-error');
  await expect(toast).toContainText('Connect a Claude account first');
  await expect(toast).toHaveCount(1);
  // Said once: no "Created as one task." before the refusal.
  expect(await browser.evaluate(() => [...document.querySelectorAll('.toast .toast-msg')].map((m) => m.textContent))).toEqual([
    'Couldn’t start the task: Connect a Claude account first: no Claude account is connected, so no agent can start. An instance administrator connects one in Subscriptions (/admin/accounts).',
  ]);
  await expect(toast.getByRole('button', { name: 'Connect Claude' })).toBeVisible();
  await expect(screen.getByRole('region', 'Split proposal')).toHaveCount(0);
  await expect(browser).toHaveURL(/#\/s\/studio$/);
  expect(await promptText(browser)).toContain('Finish the release before Friday');
  expect(await browser.evaluate(() => (window as unknown as { __yuliusMock: { tasks: unknown[] } }).__yuliusMock.tasks.length)).toBe(tasksBefore);
});
