import { test } from '@e2e-dev/web';
import { expect } from 'e2e';
import { axeViolations, failOnPageErrors, mock } from './support.ts';

failOnPageErrors();

// F 2026-10-10 (#110): "here should be only messages received by other sessions/task. User sends
// messages in Claude." F 2026-10-12: tasks no longer message each other. The panel shows notices
// from Yulius and the agent's messages to people; people answer the agent in its terminal.
test('Messages has no composer, and an agent question is answered in the terminal', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/t/12'));
  await screen.getByRole('button', /^Show tools/).click();
  await screen.getByRole('navigation', 'Tools').getByRole('button', /^Messages/).click();

  const log = screen.getByRole('log', 'Messages');
  // A notice from Yulius, and the agent of #12 asking people.
  await expect(log.getByText('Checks failed on the pull request')).toBeVisible();
  await expect(log.getByText('Stripe keys')).toBeVisible();
  // Nothing to type into: no text box and no Send button in the panel.
  expect(await browser.evaluate(() => {
    const panel = document.querySelector('[aria-label="Tool panel"]')!;
    return {
      fields: panel.querySelectorAll('textarea, input[type="text"]').length,
      send: [...panel.querySelectorAll('button')].filter((b) => /send/i.test(b.textContent ?? '')).length,
    };
  })).toEqual({ fields: 0, send: 0 });
  expect(await axeViolations(browser)).toEqual([]);

  // The question offers its answer in the terminal.
  const terminalInput = browser.locator('.taskview-term .xterm-helper-textarea');
  const reply = log.getByRole('button', 'Reply in the terminal');
  await reply.focus();
  await expect(terminalInput).not.toBeFocused();
  await reply.click();
  await expect(terminalInput).toBeFocused();
});
