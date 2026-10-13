import { test } from '@e2e-dev/web';
import { expect } from 'e2e';
import { axeViolations, failOnPageErrors, mock } from './support.ts';

failOnPageErrors();

// Supertasks (SPEC-071): picked on the composer's start chip, a task waits for the running work
// of its project and then runs alone; the project's other tasks wait for it. The mock answers
// like the server (`held_by`).

test('Supertask is a start choice; the supertask and a task after it say what they wait for (AC-SUP-6)', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio'));
  await expect(screen.getByRole('link', /Checkout API/).first()).toBeVisible();
  await screen.getByRole('button', /Run after: Start now/).click();
  const item = screen.getByRole('menuitemradio', /^Supertask/);
  await expect(item).toHaveAttribute('aria-checked', 'false');
  await item.click();
  await expect(screen.getByRole('button', /Run after: Supertask/)).toBeVisible();
  expect(await axeViolations(browser)).toEqual([]);

  // A long prompt is still one task: no split is offered for a supertask.
  const prompt = screen.getByRole('textbox', 'Task prompt');
  await prompt.fill(`Harden the whole app: ${'review every endpoint, header and secret. '.repeat(8)}`);
  await screen.getByRole('button', 'Start task').click();
  await expect(browser).toHaveURL(/#\/s\/studio\/t\/\d+$/);
  const supertask = await browser.evaluate(() => Number(location.hash.match(/\/t\/(\d+)$/)?.[1]));
  await expect(browser.locator('.strip-status')).toHaveText(/Supertask · waits for #12/);
  await expect(browser.locator('.supertask-chip')).toHaveText('Supertask');
  await expect(browser.locator('.attention-bar')).toHaveText(/Supertask\. It starts once #12.* finish(es)?, then runs alone/);

  // The composer is back to Start now; a task started now waits for the supertask.
  await browser.evaluate(() => { location.hash = '#/s/studio'; });
  await expect(screen.getByRole('button', /Run after: Start now/)).toBeVisible();
  await screen.getByRole('textbox', 'Task prompt').fill('Fix the footer link');
  await screen.getByRole('button', 'Start task').click();
  await expect(browser).toHaveURL(/#\/s\/studio\/t\/\d+$/);
  await expect(browser.locator('.strip-status')).toHaveText(`Queued after supertask #${supertask}`);
  await expect(browser.locator('.attention-bar')).toHaveText(new RegExp(`Queued after supertask #${supertask}\\. It starts once #${supertask} is done`));
});

test('Supertask and a chain exclude each other in the start menu', async ({ app, screen }) => {
  await app.open(mock('/s/studio'));
  await expect(screen.getByRole('link', /Checkout API/).first()).toBeVisible();
  await screen.getByRole('button', /Run after: Start now/).click();
  await screen.getByRole('menuitemradio', /^Supertask/).click();
  await screen.getByRole('button', /Run after: Supertask/).click();
  await screen.getByRole('menuitemradio', /^#12 Checkout API/).click();
  await expect(screen.getByRole('button', /Run after: After #12/)).toBeVisible();
  await screen.getByRole('button', /Run after: After #12/).click();
  await expect(screen.getByRole('menuitemradio', /^Supertask/)).toHaveAttribute('aria-checked', 'false');
});
