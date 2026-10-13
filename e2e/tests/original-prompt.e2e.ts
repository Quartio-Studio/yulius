import { test } from '@e2e-dev/web';
import { expect } from 'e2e';
import { axeViolations, failOnPageErrors, mock } from './support.ts';

failOnPageErrors();

// F 2026-10-12 (task #42, SPEC-043): "Here I want a button: 'see the original prompt' that shows
// the prompt I uploaded here, in the task window".

test('the task menu shows the original prompt with its attached files', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/t/12'));
  await expect(screen.getByRole('heading', { level: 1 })).toHaveText('#12 Checkout API');

  await browser.locator('.taskview .stepstrip-main [aria-label="Task actions"]').click();
  await screen.getByRole('menuitem', 'See the original prompt').click();

  const dialog = screen.getByRole('dialog', 'Original prompt · #12');
  await expect(dialog).toBeVisible();
  // The text, formatted as written; the attachment lines are files below it, not text.
  await expect(dialog).toHaveText(/Checkout API\s*Build the checkout endpoint:\s*Stripe payment intents\s*an idempotency key per order/);
  await expect(dialog).not.toHaveText(/@\//);

  // An image shows as a thumbnail that opens full size; another file downloads; a file that is
  // gone is named.
  const files = dialog.getByRole('list', 'Attached files');
  await expect(files.getByRole('img', 'checkout-error.png')).toBeVisible();
  await expect(files.getByRole('button', 'Download notes.txt')).toBeVisible();
  await expect(files).toHaveText(/old-mockup\.png\s*No longer on the server/);
  expect(await axeViolations(browser)).toEqual([]);

  await files.getByRole('button', 'Open checkout-error.png').click();
  const preview = screen.getByRole('dialog', 'checkout-error.png');
  await expect(preview.getByRole('img', 'checkout-error.png')).toBeVisible();
  await browser.keyboard.press('Escape');
  await expect(preview).toBeHidden();

  await browser.keyboard.press('Escape');
  await expect(dialog).toBeHidden();
});

test('the sidebar menu opens it too, and a failed load says why', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/t/12', '&fail=/api/tasks/tsk_studio_12/prompt'));
  await expect(screen.getByRole('heading', { level: 1 })).toHaveText('#12 Checkout API');

  const row = browser.locator('.task-row:has-text("Checkout API")');
  await row.hover();
  await browser.locator('.task-row:has-text("Checkout API") [aria-label="Task actions"]').click();
  await screen.getByRole('menuitem', 'See the original prompt').click();
  const dialog = screen.getByRole('dialog', 'Original prompt · #12');
  await expect(dialog).toHaveText(/The mock server failed on purpose/);
});
