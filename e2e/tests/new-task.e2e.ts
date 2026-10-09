import { test } from '@e2e-dev/web';
import { expect } from 'e2e';
import { failOnPageErrors, mock } from './support.ts';

failOnPageErrors();

test('the new-task shortcut opens the composer and Enter starts the task', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/t/12'));
  await expect(screen.getByRole('heading', { level: 1 })).toHaveText('#12 Checkout API');

  await browser.keyboard.press('ControlOrMeta+Shift+O');

  await expect(browser).toHaveURL(/#\/s\/studio$/);
  const prompt = screen.getByRole('textbox', 'Task prompt');
  await expect(prompt).toBeFocused();

  await prompt.fill('Add a health check endpoint');
  await prompt.press('Enter');

  await expect(browser).toHaveURL(/#\/s\/studio\/t\/\d+$/);
  await expect(screen.getByRole('heading', { level: 1 })).toHaveText(/^#\d+ Add a health check endpoint$/);
});
