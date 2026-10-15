import { test, type Browser, type Screen } from '@e2e-dev/web';
import { expect } from 'e2e';
import { failOnPageErrors, mock, terminalShown } from './support.ts';

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

/** Two terminals the server already has (as after a reload): only the shown one is attached. */
async function seedTwoShells(browser: Browser): Promise<void> {
  await browser.evaluate(() => {
    const db = (window as unknown as { __yuliusMock: { shells: Map<string, object> } }).__yuliusMock;
    for (const [n, name] of [[1, 'Terminal 1'], [2, 'Terminal 2']] as const) {
      const id = `sh_sb_studio_seed${n}`;
      db.shells.set(id, { id, sandbox_id: 'sb_studio', name, task_id: null, created_at: new Date().toISOString(), session: null, folder: '/srv/yulius/sandboxes/studio/repo' });
    }
  });
}

const shownInput = (browser: Browser) => browser.locator('.term-views .term-slot:not([hidden]) .xterm-helper-textarea');

test('a Shell tab whose terminal has not opened yet takes the keyboard when chosen', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/t/12'));
  await seedTwoShells(browser);
  await openShellPanel(screen);
  const tabs = screen.getByRole('tablist', 'Terminals');
  // The panel shows the last terminal; the first has not opened.
  await expect(tabs.getByRole('tab', 'Terminal 2')).toHaveAttribute('aria-selected', 'true');
  await tabs.getByRole('tab', 'Terminal 1').click();
  await expect(tabs.getByRole('tab', 'Terminal 1')).toHaveAttribute('aria-selected', 'true');
  await expect(shownInput(browser)).toBeFocused();
});

test('closing the shown Shell tab puts the keyboard in the neighbour that had not opened yet', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/t/12'));
  await seedTwoShells(browser);
  await openShellPanel(screen);
  const tabs = screen.getByRole('tablist', 'Terminals');
  await expect(tabs.getByRole('tab', 'Terminal 2')).toHaveAttribute('aria-selected', 'true');
  await browser.locator('.term-tab.active .term-tab-close').click(); // the × of Terminal 2
  await expect(tabs.getByRole('tab', 'Terminal 1')).toHaveAttribute('aria-selected', 'true');
  await expect(shownInput(browser)).toBeFocused();
});

// #439 (SPEC-170 TKS-002): shown again for another page, the Shell leaves the keyboard on the page,
// as the Superagent and Notes do (SPEC-056); opened by the person, it takes it.
test('the Shell shown again for another task leaves the keyboard there; the rail gives it back', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/t/12'));
  await openShellPanel(screen);
  await expect(shownInput(browser)).toBeFocused();

  await screen.getByRole('link', /3D model viewer/).click();
  await expect(screen.getByRole('heading', { level: 1 })).toHaveText('#11 3D model viewer');
  await terminalShown(browser);
  // The Shell focused in a timer queued as the task showed: the timers queued before this one ran.
  await browser.evaluate(() => new Promise((resolve) => { setTimeout(() => resolve(true), 0); }));
  expect(await browser.evaluate(() => Boolean(document.activeElement?.closest('.term-views')))).toBe(false);

  // The rail's Shell closes the panel, and opening it again puts the keyboard in the terminal.
  const shell = screen.getByRole('navigation', 'Tools').getByRole('button', 'Shell');
  await shell.click();
  await expect(screen.getByRole('region', 'Tool panel')).toBeHidden();
  await shell.click();
  await expect(shownInput(browser)).toBeFocused();
});
