import { test, type Browser, type Screen } from '@e2e-dev/web';
import { expect } from 'e2e';
import { axeViolations, DRAG_GRIP, DRAG_HINT, dragTab, failOnPageErrors, firstShown, mock, pressAndMove, tabGrips, tabMenu, tabNames, tabsInPlace, terminalShown } from './support.ts';

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

// Review of #439 (SPEC-170 TKS-002): a sandbox visited for the first time with the Shell open builds
// its own Shell, which loads its terminals; shown for the new page, it leaves the keyboard there too.
test('the Shell of a sandbox first visited with it open leaves the keyboard on the page', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/t/12'));
  await openShellPanel(screen);
  await expect(shownInput(browser)).toBeFocused();

  // A page without a text field (the composer's prompt keeps the keyboard anyway).
  await browser.evaluate(() => { window.location.hash = '#/s/product/git'; });
  await expect(screen.getByRole('heading', { level: 1 })).toHaveText('Git');
  const productTerminal = browser.locator('.panel-body > .panel-view:not([hidden]) .term-slot:not([hidden]) .xterm-rows');
  await expect(productTerminal).toHaveText(/Contained sandbox shell/);
  // The Shell focused as its terminal was shown: the timers queued before this one ran.
  await browser.evaluate(() => new Promise((resolve) => { setTimeout(() => resolve(true), 0); }));
  expect(await browser.evaluate(() => Boolean(document.activeElement?.closest('.term-views')))).toBe(false);
});

// SPEC-450 (the e2e job of #1113's pull request): a sandbox's page that still loads is not the
// tools' context yet. The Shell chosen then was the one outside a sandbox ("Open a sandbox first."),
// shown again for the page without the keyboard; it waits for its sandbox and opens as chosen.
const studioTasks = '&hold=/api/sandboxes/sb_studio/tasks';
const studioLoading = (screen: Screen) => expect(screen.getByRole('navigation', 'Sandboxes').getByRole('link', /^studio\b/)).toHaveAttribute('aria-current', 'page');
const releaseHeld = (browser: Browser) => browser.evaluate(() => (window as unknown as { __yuliusMock: { release: () => void } }).__yuliusMock.release());

test('the Shell chosen while its sandbox’s page loads opens there with the keyboard', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/t/12', studioTasks));
  await studioLoading(screen);
  await openShellPanel(screen);
  const shell = screen.getByRole('navigation', 'Tools').getByRole('button', 'Shell');
  await expect(shell).toHaveAttribute('aria-pressed', 'true');
  await expect(screen.getByRole('region', 'Tool panel')).toBeHidden();

  await releaseHeld(browser);
  await expect(screen.getByRole('heading', { level: 1 })).toHaveText('#12 Checkout API');
  await expect(shownInput(browser)).toBeFocused();
  await expect(shell).toHaveAttribute('aria-pressed', 'true');
});

test('a tool chosen twice while its sandbox’s page loads does not open', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/t/12', studioTasks));
  await studioLoading(screen);
  await openShellPanel(screen);
  const shell = screen.getByRole('navigation', 'Tools').getByRole('button', 'Shell');
  await expect(shell).toHaveAttribute('aria-pressed', 'true');
  await expect(screen.getByRole('region', 'Tool panel')).toBeHidden();
  await shell.click();
  await expect(shell).toHaveAttribute('aria-pressed', 'false');

  await releaseHeld(browser);
  await expect(screen.getByRole('heading', { level: 1 })).toHaveText('#12 Checkout API');
  await expect(screen.getByRole('region', 'Tool panel')).toBeHidden();
  await expect(shell).toHaveAttribute('aria-pressed', 'false');
});

test('a tool chosen while its sandbox’s page loads does not open once the tools are hidden', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/t/12', studioTasks));
  await studioLoading(screen);
  await openShellPanel(screen);
  await expect(screen.getByRole('navigation', 'Tools').getByRole('button', 'Shell')).toHaveAttribute('aria-pressed', 'true');
  await expect(screen.getByRole('region', 'Tool panel')).toBeHidden();
  await screen.getByRole('button', /^Hide tools/).click();

  await releaseHeld(browser);
  await expect(screen.getByRole('heading', { level: 1 })).toHaveText('#12 Checkout API');
  await expect(screen.getByRole('region', 'Tool panel')).toBeHidden();
  await expect(screen.getByRole('navigation', 'Tools')).toBeHidden();
});

// On the way in from a page outside a sandbox (only the Superagent in the rail) the tools show
// while the sandbox's page loads, and could not be chosen. The one chosen last opens there.
test('the tool chosen last on the way into a sandbox opens there', async ({ app, browser, screen }) => {
  await app.open(mock('/settings', studioTasks));
  await expect(screen.getByRole('heading', { level: 1 })).toHaveText('Settings');
  await screen.getByRole('button', /^Show tools/).click();
  const tools = screen.getByRole('navigation', 'Tools');
  await expect(tools.getByRole('button', 'Superagent')).toBeVisible();
  await expect(tools.getByRole('button', 'Shell')).toBeHidden();

  await browser.evaluate(() => { window.location.hash = '#/s/studio/t/12'; });
  await studioLoading(screen);
  await tools.getByRole('button', 'Shell').click();
  await expect(tools.getByRole('button', 'Shell')).toHaveAttribute('aria-pressed', 'true');
  await tools.getByRole('button', 'Notes').click();
  await expect(tools.getByRole('button', 'Notes')).toHaveAttribute('aria-pressed', 'true');
  await expect(tools.getByRole('button', 'Shell')).toHaveAttribute('aria-pressed', 'false');
  await expect(screen.getByRole('region', 'Tool panel')).toBeHidden();

  await releaseHeld(browser);
  await expect(screen.getByRole('heading', { level: 1 })).toHaveText('#12 Checkout API');
  await expect(browser.locator('.panel .panel-title')).toHaveText('Notes');
  await expect(tools.getByRole('button', 'Notes')).toHaveAttribute('aria-pressed', 'true');
});

// SPEC-434 MTB-002/004: the terminal tabs move as the sandbox and Notes tabs do, and keep their order.
const termStrip = '[role="tablist"][aria-label="Terminals"]';

/** Three terminals the server already has, named Terminal 1 to 3 (seeded again after a reload). */
async function seedThreeShells(browser: Browser): Promise<void> {
  await browser.evaluate(() => {
    const db = (window as unknown as { __yuliusMock: { shells: Map<string, object> } }).__yuliusMock;
    for (const n of [1, 2, 3]) {
      const id = `sh_sb_studio_seed${n}`;
      db.shells.set(id, { id, sandbox_id: 'sb_studio', name: `Terminal ${n}`, task_id: null, created_at: new Date().toISOString(), session: null, folder: '/srv/yulius/sandboxes/studio/repo' });
    }
  });
}

test('terminal tabs move by drag, keyboard and menu and stay so', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/t/12', '&ambient=0'));
  await seedThreeShells(browser);
  await openShellPanel(screen);
  const tabs = screen.getByRole('tablist', 'Terminals');
  const tab = (name: string) => tabs.getByRole('tab', name);
  await expect(tab('Terminal 3')).toHaveAttribute('aria-selected', 'true');
  expect(await tabNames(browser, termStrip)).toEqual(['Terminal 1', 'Terminal 2', 'Terminal 3']);
  const grips = await tabGrips(browser, termStrip);
  expect(grips.map((g) => g.grip)).toEqual([DRAG_GRIP, DRAG_GRIP, DRAG_GRIP]);
  for (const g of grips) expect(g.title.endsWith(`\n${DRAG_HINT}`)).toBe(true);
  await expect(tab('Terminal 1')).toHaveAttribute('aria-keyshortcuts', 'F2 Delete Backspace Alt+ArrowLeft Alt+ArrowRight');

  // A drag moves Terminal 1 last and chooses nothing.
  await dragTab(browser, tab('Terminal 1'), tab('Terminal 3'));
  await expect.poll(() => tabNames(browser, termStrip)).toEqual(['Terminal 2', 'Terminal 3', 'Terminal 1']);
  await expect(tab('Terminal 3')).toHaveAttribute('aria-selected', 'true');

  // Alt+←/→ moves the focused tab and keeps the keyboard on it.
  await tab('Terminal 1').focus();
  await browser.keyboard.press('Alt+ArrowLeft');
  expect(await tabNames(browser, termStrip)).toEqual(['Terminal 2', 'Terminal 1', 'Terminal 3']);
  await expect(tab('Terminal 1')).toBeFocused();
  await expect(tab('Terminal 3')).toHaveAttribute('aria-selected', 'true');

  // The menu moves it without dragging (WCAG 2.5.7), next to Rename and Close.
  await tabMenu(browser, '#term-tab-sh_sb_studio_seed2');
  await expect(screen.getByRole('menuitem', 'Move left')).toHaveAttribute('aria-disabled', 'true');
  await expect(screen.getByRole('menuitem', 'Rename')).toBeVisible();
  await expect(screen.getByRole('menuitem', 'Close')).toBeVisible();
  await screen.getByRole('menuitem', 'Move right').click();
  expect(await tabNames(browser, termStrip)).toEqual(['Terminal 1', 'Terminal 2', 'Terminal 3']);
  await tabMenu(browser, '#term-tab-sh_sb_studio_seed3');
  await expect(screen.getByRole('menuitem', 'Move right')).toHaveAttribute('aria-disabled', 'true');
  await screen.getByRole('menuitem', 'Move left').click();
  expect(await tabNames(browser, termStrip)).toEqual(['Terminal 1', 'Terminal 3', 'Terminal 2']);
  expect(await axeViolations(browser)).toEqual([]);

  // A tab being renamed does not drag: the press edits its name (review round 1).
  await tab('Terminal 3').focus();
  await browser.keyboard.press('F2');
  await expect(screen.getByRole('textbox', 'Rename Terminal 3')).toBeFocused();
  expect(await pressAndMove(browser, '.term-tab-rename', 80)).toBe(false);
  await browser.keyboard.press('Escape');
  expect(await tabNames(browser, termStrip)).toEqual(['Terminal 1', 'Terminal 3', 'Terminal 2']);

  // A rename ending as another tab is pressed (the press takes the focus) redraws the strip once
  // the tab is let go, with both the new name and the new place (review round 1).
  await tab('Terminal 2').focus();
  await browser.keyboard.press('F2');
  await screen.getByRole('textbox', 'Rename Terminal 2').fill('dev server');
  // Pressed and moved: the rename ends (the press takes the focus), yet the strip is not drawn
  // again under the pointer (#477) until the tab is let go.
  const from = await tab('Terminal 1').boundingBox();
  const to = await browser.locator('#term-tab-sh_sb_studio_seed2').boundingBox();
  if (!from || !to) throw new Error('tabs not laid out');
  const y = from.y + from.height / 2;
  await browser.mouse.move(from.x + 12, y);
  await browser.mouse.down();
  await browser.mouse.move(from.x + 40, y);
  expect(await browser.evaluate((selector) => {
    const held = document.querySelector<HTMLElement>(`${selector} .term-tab.dragging`);
    return held ? { id: held.dataset.id, connected: held.isConnected } : null;
  }, termStrip)).toEqual({ id: 'sh_sb_studio_seed1', connected: true });
  await browser.mouse.move(to.x + to.width - 4 - (from.width / 2 - 12), y);
  await browser.mouse.up();
  await expect.poll(() => tabNames(browser, termStrip)).toEqual(['Terminal 3', 'dev server', 'Terminal 1']);
  await expect.poll(() => tabsInPlace(browser, termStrip)).toBe(true);

  // A press on × closes the tab and moves nothing, however far the pointer goes.
  expect(await pressAndMove(browser, '#term-tab-sh_sb_studio_seed1 .term-tab-close', -80)).toBe(false);
  expect(await tabNames(browser, termStrip)).toEqual(['Terminal 3', 'dev server', 'Terminal 1']);
  await browser.locator('#term-tab-sh_sb_studio_seed1 .term-tab-close').click();
  await expect(tab('Terminal 1')).toBeHidden();
  expect(await tabNames(browser, termStrip)).toEqual(['Terminal 3', 'dev server']);

  // Kept after a reload (the panel closed first, so the terminals are read once seeded); a new
  // terminal comes last.
  await screen.getByRole('navigation', 'Tools').getByRole('button', 'Shell').click();
  await expect(tabs).toBeHidden();
  await browser.reload();
  await firstShown(screen.getByRole('heading', { level: 1 }), '#12 Checkout API');
  await seedThreeShells(browser);
  if (await browser.locator('.rail:not([hidden])').count() === 0) await screen.getByRole('button', /^Show tools/).click();
  await screen.getByRole('navigation', 'Tools').getByRole('button', 'Shell').click();
  await expect(tab('Terminal 3')).toBeVisible();
  expect(await tabNames(browser, termStrip)).toEqual(['Terminal 3', 'Terminal 2', 'Terminal 1']);
  await screen.getByRole('button', 'New terminal').click();
  await expect(tab('Terminal 4')).toHaveAttribute('aria-selected', 'true');
  expect(await tabNames(browser, termStrip)).toEqual(['Terminal 3', 'Terminal 2', 'Terminal 1', 'Terminal 4']);
});
