import { test } from '@e2e-dev/web';
import { expect } from 'e2e';
import { axeViolations, failOnPageErrors, mock, mockReady, touchTask } from './support.ts';

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

// #303: a prompt is shown as written, and only the task's own attachment folders are served.
test('a prompt starting with --- is text, and files outside the task’s folders are not served', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/t/12'));
  await expect(screen.getByRole('heading', { level: 1 })).toHaveText('#12 Checkout API');
  await mockReady(browser);
  const attachments = '/srv/yulius/sandboxes/sb_studio/attachments';
  await touchTask(browser, 'sb_studio', 12, {
    prompt: [
      '---', 'title: not properties', '---', 'Body of the prompt', '',
      `@${attachments}/drafts/usr_mkarp/own.png`,
      `@${attachments}/tasks/tsk_studio_12/dropped.png`,
      `@${attachments}/drafts/usr_other/theirs.png`,
      `@${attachments}/drafts/usr_mkarp/../usr_other/climb.png`,
      // In an owned folder by name, but `..` itself.
      `@${attachments}/drafts/usr_mkarp/..`,
      `@${attachments}/tasks/tsk_studio_3/other-task.png`,
    ].join('\n'),
  });

  await browser.locator('.taskview .stepstrip-main [aria-label="Task actions"]').click();
  await screen.getByRole('menuitem', 'See the original prompt').click();
  const dialog = screen.getByRole('dialog', 'Original prompt · #12');
  await expect(dialog).toHaveText(/title: not properties\s*Body of the prompt/);
  // Not frontmatter: no property named title.
  await expect(dialog.getByRole('textbox', 'title')).toBeHidden();

  const files = dialog.getByRole('list', 'Attached files');
  await expect(files.getByRole('img', 'own.png')).toBeVisible();
  await expect(files.getByRole('img', 'dropped.png')).toBeVisible();
  for (const name of ['theirs.png', 'climb.png', 'other-task.png']) {
    await expect(files.getByRole('listitem').filter({ hasText: name })).toHaveText(/No longer on the server/);
  }
  await expect(files.getByRole('listitem').filter({ hasText: 'No longer on the server' })).toHaveCount(4);
});
