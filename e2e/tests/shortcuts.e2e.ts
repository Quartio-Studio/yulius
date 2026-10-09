import { test } from '@e2e-dev/web';
import { expect } from 'e2e';
import { failOnPageErrors, mock } from './support.ts';

failOnPageErrors();

// On the Tasks page: a task page focuses its terminal, where Ctrl+/ belongs to the shell outside
// macOS (app.js), so the sheet opens there only with ⌘/.
test('the shortcut list opens and closes from the keyboard', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/tasks'));
  await expect(screen.getByRole('heading', { level: 1 })).toHaveText('Tasks');

  await browser.keyboard.press('ControlOrMeta+/');
  const sheet = screen.getByRole('dialog', 'Keyboard shortcuts');
  await expect(sheet).toBeVisible();
  await expect(sheet.getByText('Keyboard shortcuts (this list)')).toBeVisible();

  await browser.keyboard.press('Escape');
  await expect(sheet).toBeHidden();
});
