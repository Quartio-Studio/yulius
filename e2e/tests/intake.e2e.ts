import { test } from '@e2e-dev/web';
import { expect } from 'e2e';
import { axeViolations, failOnPageErrors, mock } from './support.ts';

failOnPageErrors();

// Task intake and chaining (SPEC-001 TSK-011..013): the composer's split card, chained tasks
// and the card of a prompt held by the scope check. The mock answers like the server.

const MIXED = [
  'Finish the release before Friday, all of this has to land together please.',
  '1. Make the attention bar opaque over the terminal and keep its colours readable.',
  '2. Stop the tools rail pills from overflowing the rail on narrow windows.',
  '3. Route new tasks past exhausted accounts in the backend subscription pool.',
  '4. Make the server mark tasks Done reliably once the pull request is tested.',
].join('\n');

test('a request mixing UI and backend work offers a split; a chain waits for its predecessor', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio'));
  const prompt = screen.getByRole('textbox', 'Task prompt');
  await prompt.fill(MIXED);
  await prompt.press('Enter');

  const card = screen.getByRole('region', 'Split proposal');
  await expect(card).toHaveText(/This looks like 2 separate tasks\./);
  await expect(card.getByRole('textbox', 'Title of task 1')).toHaveValue('UX: interface fixes');
  expect(await axeViolations(browser)).toEqual([]);

  await card.getByRole('button', 'Create as a chain').click();
  await expect(browser).toHaveURL(/#\/s\/studio\/t\/\d+$/);
  await expect(screen.getByRole('heading', { level: 1 })).toHaveText(/^#\d+ UX: interface fixes$/);

  // The chain shows on both tasks: "Then #N" here, "Queued after #M" there.
  await screen.getByRole('link', /^Then #\d+$/).click();
  await expect(screen.getByRole('heading', { level: 1 })).toHaveText(/^#\d+ Backend: server work$/);
  await expect(screen.getByRole('link', /^After #\d+$/)).toBeVisible();
  await expect(browser.locator('.attention-bar')).toHaveText(/Queued after #\d+\. It starts by itself when #\d+ is done\./);
  await screen.getByRole('button', 'Start now').click();
  await expect(browser.locator('.strip-status')).not.toHaveText(/Queued after/);
});

test('"Keep as one task" creates the task as written', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio'));
  const prompt = screen.getByRole('textbox', 'Task prompt');
  await prompt.fill(MIXED);
  await prompt.press('Enter');
  const card = screen.getByRole('region', 'Split proposal');
  await card.getByRole('button', 'Keep as one task').click();
  await expect(browser).toHaveURL(/#\/s\/studio\/t\/\d+$/);
  await expect(screen.getByRole('heading', { level: 1 })).toHaveText(/^#\d+ Finish the release before Friday/);
});

test('a short request starts at once and "Run after" chains it', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio'));
  // The choices are the sandbox's tasks: wait until the list has loaded.
  await expect(screen.getByRole('link', /Checkout API/).first()).toBeVisible();
  await screen.getByRole('button', /Run after: Start now/).click();
  await screen.getByRole('menuitemradio', /^After #12 Checkout API/).click();
  const prompt = screen.getByRole('textbox', 'Task prompt');
  await prompt.fill('Write the checkout UI tests');
  await prompt.press('Enter');
  await expect(browser).toHaveURL(/#\/s\/studio\/t\/\d+$/);
  await expect(browser.locator('.strip-status')).toHaveText(/Queued after #12/);
});

test('a held prompt shows its choices; "Run after this task" chains a new task', async ({ app, browser, screen }) => {
  await app.open(mock('/s/product/t/4'));
  const card = screen.getByRole('region', 'Held prompt');
  await expect(card).toHaveText(/New work outside this task/);
  await expect(card).toHaveText(/Also add a dark mode to the invoice settings page\./);
  await expect(card.getByRole('button', 'Send anyway')).toBeVisible();
  expect(await axeViolations(browser)).toEqual([]);

  await card.getByRole('button', 'Run after this task').click();
  await expect(card).toBeHidden();
  await expect(screen.getByRole('link', /^Then #8$/)).toBeVisible();
});

test('a chained task can be started now', async ({ app, browser, screen }) => {
  await app.open(mock('/s/product/t/7'));
  await expect(screen.getByRole('heading', { level: 1 })).toHaveText('#7 Invoice emails');
  await expect(browser.locator('.strip-status')).toHaveText(/Queued after #4/);
  await screen.getByRole('button', 'Start now').click();
  await expect(browser.locator('.strip-status')).not.toHaveText(/Queued after/);
});
