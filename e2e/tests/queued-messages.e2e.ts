import { test, type Browser } from '@e2e-dev/web';
import { expect } from 'e2e';
import { axeViolations, failOnPageErrors, mock } from './support.ts';

failOnPageErrors();

// Queued messages (SPEC-052): a task that waits and has never started takes messages into its
// request, so a person can correct it before the agent sees it.

type MockTasks = { __yuliusMock: { tasks: { id: string; prompt?: string; started?: boolean }[] } };

const storedRequest = (browser: Browser, id: string) =>
  browser.evaluate((tid) => (window as unknown as MockTasks).__yuliusMock.tasks.find((t) => t.id === tid)?.prompt ?? '', id);

test('a queued task takes a message into its request', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/t/8'));

  const card = screen.getByRole('region', 'Queued messages');
  await expect(card).toBeVisible();
  const request = card.getByRole('region', 'Request');
  await expect(request).toContainText('Nightly feedback digest');
  expect(await axeViolations(browser)).toEqual([]);

  const box = card.getByRole('textbox', 'Message for the agent');
  await box.click();
  await browser.keyboard.type('Only the last 24 hours, please.');
  await card.getByRole('button', 'Queue').click();

  await expect(box).toHaveValue('');
  await expect(request).toContainText(/Added by mkarp before the task started \(.+ UTC\):/);
  await expect(request).toContainText('Only the last 24 hours, please.');
  expect(await storedRequest(browser, 'tsk_studio_8')).toMatch(/Nightly feedback digest\n\n\*\*Added by mkarp before the task started \(.+ UTC\):\*\*\n\nOnly the last 24 hours, please\.$/);

  // ⌘/Ctrl+Enter queues the next one.
  await box.click();
  await browser.keyboard.type('And skip weekends.');
  await browser.keyboard.press(process.platform === 'darwin' ? 'Meta+Enter' : 'Control+Enter');
  await expect(box).toHaveValue('');
  await expect(request).toContainText('And skip weekends.');
});

test('a chained task takes messages, and a refused one keeps the text with the reason', async ({ app, browser, screen }) => {
  await app.open(mock('/s/product/t/7'));

  const card = screen.getByRole('region', 'Queued messages');
  await expect(card).toBeVisible();
  // The task started meanwhile (its view has not heard yet): the server refuses.
  await browser.evaluate(() => {
    const task = (window as unknown as MockTasks).__yuliusMock.tasks.find((t) => t.id === 'tsk_product_7');
    if (task) task.started = true;
  });
  const box = card.getByRole('textbox', 'Message for the agent');
  await box.click();
  await browser.keyboard.type('Send them in Polish too.');
  await card.getByRole('button', 'Queue').click();

  await expect(card.getByRole('alert')).toContainText('type into its session instead');
  await expect(box).toHaveValue('Send them in Polish too.');
  expect(await storedRequest(browser, 'tsk_product_7')).not.toContain('Polish');
});

test('a running task has no queued messages card', async ({ app, screen }) => {
  await app.open(mock('/s/studio/t/12'));
  await expect(screen.getByRole('heading', { level: 1 })).toHaveText(/#12/);
  await expect(screen.getByRole('region', 'Queued messages')).toBeHidden();
});
