import { test, type Browser, type Screen } from '@e2e-dev/web';
import { expect } from 'e2e';
import { failOnPageErrors, mock } from './support.ts';

failOnPageErrors();

// The user's own terminals in a sandbox (UI-016, F 2026-10-09): the Shell panel shows a working
// terminal at once, without a button; "+" opens more, each in its own tab.

async function openShellPanel(screen: Screen): Promise<void> {
  await screen.getByRole('button', /^Show tools/).click();
  await screen.getByRole('button', 'Shell').click();
}

/** The visible terminal's rows in the Shell panel. */
const shownTerminal = (browser: Browser) => browser.locator('.term-views .term-slot:not([hidden]) .xterm-rows');

test('the Shell panel shows a working terminal without a button', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/t/12'));
  await openShellPanel(screen);

  const tabs = screen.getByRole('tablist', 'Terminals');
  await expect(tabs.getByRole('tab', 'Terminal 1')).toHaveAttribute('aria-selected', 'true');
  await expect(screen.getByRole('button', 'Open shell')).toBeHidden();
  const terminal = shownTerminal(browser);
  await expect(terminal).toHaveText(/Contained sandbox shell/);

  // The keyboard is already in the terminal: a command runs.
  await expect(browser.locator('.term-views .term-slot:not([hidden]) .xterm-helper-textarea')).toBeFocused();
  for (const key of ['l', 's', 'Enter']) await browser.keyboard.press(key);
  await expect(terminal).toHaveText(/AGENTS\.md/);
});

test('"+" opens another terminal; tabs switch, rename and close', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/t/12'));
  await openShellPanel(screen);
  const tabs = screen.getByRole('tablist', 'Terminals');
  await expect(tabs.getByRole('tab', 'Terminal 1')).toHaveAttribute('aria-selected', 'true');

  await screen.getByRole('button', 'New terminal').click();
  await expect(tabs.getByRole('tab', 'Terminal 2')).toHaveAttribute('aria-selected', 'true');
  await expect(tabs.getByRole('tab', 'Terminal 1')).toHaveAttribute('aria-selected', 'false');
  await expect(shownTerminal(browser)).toHaveText(/Contained sandbox shell/);

  await tabs.getByRole('tab', 'Terminal 1').click();
  await expect(tabs.getByRole('tab', 'Terminal 1')).toHaveAttribute('aria-selected', 'true');

  // F2 renames the focused tab.
  await tabs.getByRole('tab', 'Terminal 2').press('F2');
  const name = screen.getByRole('textbox', 'Rename Terminal 2');
  await name.fill('dev server');
  await name.press('Enter');
  await expect(tabs.getByRole('tab', 'dev server')).toBeVisible();

  // Delete closes the focused tab; the other terminal stays.
  await tabs.getByRole('tab', 'dev server').press('Delete');
  await expect(tabs.getByRole('tab', 'dev server')).toBeHidden();
  await expect(tabs.getByRole('tab', 'Terminal 1')).toHaveAttribute('aria-selected', 'true');
});

test('each sandbox has its own terminals', async ({ app, screen }) => {
  await app.open(mock('/s/studio/t/12'));
  await openShellPanel(screen);
  const tabs = screen.getByRole('tablist', 'Terminals');
  await expect(tabs.getByRole('tab', 'Terminal 1')).toBeVisible();
  await screen.getByRole('button', 'New terminal').click();
  await expect(tabs.getByRole('tab', 'Terminal 2')).toBeVisible();

  // Another sandbox: its own first terminal, none of studio's.
  await screen.getByRole('navigation', 'Sandboxes').getByRole('link', /^product\b/).click();
  await expect(tabs.getByRole('tab', 'Terminal 1')).toHaveAttribute('aria-selected', 'true');
  await expect(tabs.getByRole('tab', 'Terminal 2')).toBeHidden();

  // Back in studio, both of its terminals are there again.
  await screen.getByRole('navigation', 'Sandboxes').getByRole('link', /^studio\b/).click();
  await expect(tabs.getByRole('tab', 'Terminal 1')).toBeVisible();
  await expect(tabs.getByRole('tab', 'Terminal 2')).toBeVisible();
});
