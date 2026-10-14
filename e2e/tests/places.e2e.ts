import { test, type Browser, type Screen } from '@e2e-dev/web';
import { expect } from 'e2e';
import { failOnPageErrors, mock, openSuperagent } from './support.ts';

failOnPageErrors();

// Task #98 (SPEC-113, issue #435): "When I switch between screens, I want to keep the state. I went
// to studio sandbox, clicked claude section, went back to yulius, clicked something, then went to
// studio. In studio I had default screen instead the one I left."

const tab = (screen: Screen, name: string) => screen.getByRole('navigation', 'Sandboxes').getByRole('link', new RegExp(`^${name}\\b`));
const heading = (screen: Screen) => screen.getByRole('heading', { level: 1 });
/** Opens a sandbox tab's menu (right-click). */
const tabMenu = (browser: Browser, name: string) => browser.evaluate((name) => {
  const tabEl = [...document.querySelectorAll('.tabrow .tab')].find((el) => el.querySelector('.tab-name')?.textContent === name)!;
  const box = tabEl.getBoundingClientRect();
  tabEl.dispatchEvent(new MouseEvent('contextmenu', { bubbles: true, cancelable: true, button: 2, clientX: box.left + 10, clientY: box.top + 10 }));
}, name);

test('a sandbox tab returns to the screen left in that sandbox', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/t/12'));
  await expect(heading(screen)).toHaveText('#12 Checkout API');

  await tab(screen, 'product').click();
  await expect(browser).toHaveURL(/#\/s\/product$/);
  await expect(heading(screen)).toHaveText('New task');
  await screen.getByRole('navigation', 'Sandbox').getByRole('link', 'Git').click();
  await expect(heading(screen)).toHaveText('Git');

  await tab(screen, 'studio').click();
  await expect(browser).toHaveURL(/#\/s\/studio\/t\/12$/);
  await expect(heading(screen)).toHaveText('#12 Checkout API');

  // And product comes back on Git, where it was left.
  await tab(screen, 'product').click();
  await expect(browser).toHaveURL(/#\/s\/product\/git$/);
});

test('a page’s filters come back with the sandbox and with its sidebar link', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/tasks'));
  await screen.getByRole('radiogroup', 'Task state').getByRole('radio', /^Working/).click();
  await expect(browser).toHaveURL(/#\/s\/studio\/tasks\?state=working$/);

  await tab(screen, 'product').click();
  await expect(heading(screen)).toHaveText('New task');
  await tab(screen, 'studio').click();
  await expect(browser).toHaveURL(/#\/s\/studio\/tasks\?state=working$/);
  await expect(screen.getByRole('radiogroup', 'Task state').getByRole('radio', /^Working/)).toHaveAttribute('aria-checked', 'true');

  // Another page of the sandbox, then Tasks in the sidebar: still filtered.
  const nav = screen.getByRole('navigation', 'Sandbox');
  await nav.getByRole('link', 'Git').click();
  await expect(heading(screen)).toHaveText('Git');
  await nav.getByRole('link', 'Tasks').click();
  await expect(browser).toHaveURL(/#\/s\/studio\/tasks\?state=working$/);

  // The link to the page in view opens it plain, as before.
  await nav.getByRole('link', 'Tasks').click();
  await expect(browser).toHaveURL(/#\/s\/studio\/tasks$/);
  await expect(screen.getByRole('radiogroup', 'Task state').getByRole('radio', /^All/)).toHaveAttribute('aria-checked', 'true');
});

test('the tab of the sandbox in view opens its composer', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/t/12'));
  await tab(screen, 'studio').click();
  await expect(browser).toHaveURL(/#\/s\/studio$/);
  await expect(heading(screen)).toHaveText('New task');
});

test('Home’s sandbox card returns to the screen left there', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/t/12'));
  await expect(heading(screen)).toHaveText('#12 Checkout API');
  await screen.getByRole('link', 'All sandboxes').first().click();
  await expect(browser).toHaveURL(/#\/$/);
  await screen.getByRole('link', 'Open studio').click();
  await expect(browser).toHaveURL(/#\/s\/studio\/t\/12$/);
});

test('a missing task is not remembered as the sandbox’s place', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/t/999'));
  await expect(heading(screen)).toHaveText('Task not found');
  await tab(screen, 'product').click();
  await expect(heading(screen)).toHaveText('New task');
  await tab(screen, 'studio').click();
  await expect(browser).toHaveURL(/#\/s\/studio$/);
  await expect(heading(screen)).toHaveText('New task');
});

/** Marks the shown Superagent terminal, so the test can tell the same view from a new one. */
const markAgent = (browser: Browser) => browser.evaluate(() => { (document.querySelector('.agent-term .xterm') as HTMLElement & { __mark?: boolean }).__mark = true; });
const agentMarked = (browser: Browser) => browser.evaluate(() => [...document.querySelectorAll('.agent-term .xterm')]
  .some((el) => (el as HTMLElement & { __mark?: boolean }).__mark && (el as HTMLElement).offsetParent !== null));

test('each sandbox comes back with its own tool panel and the same Superagent session', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/t/12'));
  await openSuperagent(browser, screen);
  await expect(browser.locator('.agent-term .xterm-rows').first()).toHaveText(/Welcome to Claude Code/);
  await markAgent(browser);

  // product (not visited yet) shows the tool in view; there it is closed.
  await tab(screen, 'product').click();
  await expect(heading(screen)).toHaveText('New task');
  await expect(screen.getByRole('region', 'Tool panel')).toBeVisible();
  await screen.getByRole('navigation', 'Tools').getByRole('button', 'Superagent').click();
  await expect(screen.getByRole('region', 'Tool panel')).toBeHidden();

  // studio comes back with its Superagent open: the same session view, not a new one.
  await tab(screen, 'studio').click();
  await expect(heading(screen)).toHaveText('#12 Checkout API');
  await expect(screen.getByRole('region', 'Tool panel')).toBeVisible();
  await expect(screen.getByRole('navigation', 'Tools').getByRole('button', 'Superagent')).toHaveAttribute('aria-pressed', 'true');
  await expect.poll(() => agentMarked(browser)).toBe(true);

  // product comes back with no panel.
  await tab(screen, 'product').click();
  await expect(heading(screen)).toHaveText('New task');
  await expect(screen.getByRole('region', 'Tool panel')).toBeHidden();
});

type WithMonaco = Window & { monaco: { editor: { getModels(): { getValue(): string }[] } } };
const modelTexts = (browser: Browser) => browser.evaluate(() => (window as unknown as WithMonaco).monaco.editor.getModels().map((m) => m.getValue()));

test('the editor keeps open files and unsaved text across a sandbox switch, without feeding the other sandbox', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/t/12'));
  await screen.getByRole('button', /^Show tools/).click();
  await screen.getByRole('button', 'Editor').click();
  await screen.getByRole('tree', 'Files').getByRole('treeitem', 'README.md').click();
  const content = screen.getByRole('textbox', 'Editor content');
  await expect(content).toBeAttached();
  await content.focus();
  await browser.keyboard.type('studioUnsavedWord ');
  await expect(screen.getByRole('tab', /^README\.md\s*, modified/)).toBeVisible();

  await tab(screen, 'product').click();
  await expect(heading(screen)).toHaveText('New task');
  await expect(screen.getByRole('tab', /^README\.md/)).toBeHidden();
  // studio's files are not Monaco models while product is in front (they would feed its suggestions).
  await expect.poll(async () => (await modelTexts(browser)).some((text) => text.includes('studioUnsavedWord'))).toBe(false);

  await tab(screen, 'studio').click();
  await expect(screen.getByRole('tab', /^README\.md\s*, modified/)).toHaveAttribute('aria-selected', 'true');
  await expect.poll(async () => (await modelTexts(browser)).some((text) => text.includes('studioUnsavedWord'))).toBe(true);
});

test('MCP’s scope and the Dashboard’s period come back with the page', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/mcp'));
  await screen.getByRole('radio', /^Every sandbox/).click();
  await expect(browser).toHaveURL(/#\/s\/studio\/mcp\?scope=instance$/);

  const nav = screen.getByRole('navigation', 'Sandbox');
  await nav.getByRole('link', 'Dashboard').click();
  await expect(heading(screen)).toHaveText('Dashboard');
  // The radio itself is see-through under its label (a segmented control): the label takes the click.
  await screen.getByRole('radiogroup', 'Date range').getByText('7 days').click();
  await expect(browser).toHaveURL(/#\/s\/studio\/dashboard\?days=7$/);

  await nav.getByRole('button', 'More').click(); // MCP is under More, closed on the Dashboard
  await nav.getByRole('link', 'MCP').click();
  await expect(browser).toHaveURL(/#\/s\/studio\/mcp\?scope=instance$/);
  await expect(screen.getByRole('radio', /^Every sandbox/)).toHaveAttribute('aria-checked', 'true');

  await nav.getByRole('link', 'Dashboard').click();
  await expect(browser).toHaveURL(/#\/s\/studio\/dashboard\?days=7$/);
  await expect(screen.getByRole('radiogroup', 'Date range').getByRole('radio', '7 days')).toBeChecked();
});

// Review round 1 (SPEC-113): a sandbox left with no tool open comes back with none, even when the
// other sandbox opened one meanwhile.
test('a sandbox left without a tool comes back without one', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/t/12'));
  await expect(screen.getByRole('region', 'Tool panel')).toBeHidden();
  await tab(screen, 'product').click();
  await expect(heading(screen)).toHaveText('New task');
  await openSuperagent(browser, screen);
  await expect(screen.getByRole('region', 'Tool panel')).toBeVisible();

  await tab(screen, 'studio').click();
  await expect(heading(screen)).toHaveText('#12 Checkout API');
  await expect(screen.getByRole('region', 'Tool panel')).toBeHidden();
});

test('the back links of a session and an MCP server return to the list’s filters', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/sessions'));
  await screen.getByRole('searchbox', 'Search sessions').fill('stripe');
  await expect(browser).toHaveURL(/#\/s\/studio\/sessions\?q=stripe$/);
  await screen.getByRole('list', 'Sessions').getByRole('link', /Add Stripe checkout/).first().click();
  await browser.locator('.gt-back').click();
  await expect(browser).toHaveURL(/#\/s\/studio\/sessions\?q=stripe$/);
  await expect(screen.getByRole('searchbox', 'Search sessions')).toHaveValue('stripe');

  await browser.evaluate(() => { window.location.hash = '#/s/studio/mcp?scope=instance'; });
  await expect(heading(screen)).toHaveText('MCP');
  await browser.evaluate(() => { window.location.hash = '#/s/studio/mcp/mcp_apify'; });
  await expect(screen.getByRole('textbox', /name/i).first()).toHaveValue('apify');
  await browser.locator('.gt-back').click();
  await expect(browser).toHaveURL(/#\/s\/studio\/mcp\?scope=instance$/);
});

test('the Tasks back link and the tab menu’s Open return to where each was left', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/tasks'));
  await screen.getByRole('radiogroup', 'Task state').getByRole('radio', /^Working/).click();
  await browser.evaluate(() => { window.location.hash = '#/s/studio/tasks/12'; });
  await browser.locator('.gt-back').click();
  await expect(browser).toHaveURL(/#\/s\/studio\/tasks\?state=working$/);

  await tab(screen, 'product').click();
  await expect(heading(screen)).toHaveText('New task');
  await tabMenu(browser, 'studio');
  await screen.getByRole('menuitem', 'Open').click();
  await expect(browser).toHaveURL(/#\/s\/studio\/tasks\?state=working$/);
});

test('a deleted skill is not remembered as the sandbox’s place', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/skills/no-such-skill'));
  await expect(heading(screen)).toHaveText('Skill not found');
  await tab(screen, 'product').click();
  await expect(heading(screen)).toHaveText('New task');
  await tab(screen, 'studio').click();
  await expect(browser).toHaveURL(/#\/s\/studio$/);
});

test('closing a CI run by leaving the page does not rewrite the new address', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/git'));
  await expect(heading(screen)).toHaveText('Git');
  await browser.evaluate(() => { window.location.hash = '#/s/studio/ci?run=ci_s3'; });
  await expect(screen.getByRole('dialog', 'CI run')).toBeVisible();
  await browser.evaluate(() => history.back());
  await expect(heading(screen)).toHaveText('Git');
  await expect(browser).toHaveURL(/#\/s\/studio\/git$/);
});

test('a removed sandbox’s tool views go with it', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/t/12'));
  await openSuperagent(browser, screen);
  await expect(browser.locator('.agent-term .xterm-rows').first()).toHaveText(/Welcome to Claude Code/);
  await tab(screen, 'product').click();
  await expect(browser.locator('.panel-view:not([hidden]) > .agent .xterm-rows')).toHaveText(/cwd: .*product/);
  await expect(browser.locator('.agent')).toHaveCount(2);

  await tabMenu(browser, 'studio');
  await screen.getByRole('menuitem', /^Remove from/).click();
  const dialog = screen.getByRole('dialog', /^Remove studio/);
  await dialog.getByRole('textbox').fill('studio');
  await dialog.getByRole('button', 'Remove sandbox').click();
  await expect(tab(screen, 'studio')).toBeHidden();
  await expect(browser.locator('.agent')).toHaveCount(1);
});

test('signing out forgets where each sandbox was left', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/t/12'));
  await expect(heading(screen)).toHaveText('#12 Checkout API');
  await expect.poll(() => browser.evaluate(() => sessionStorage.getItem('yulius.places'))).toContain('/s/studio/t/12');
  await screen.getByRole('button', /account menu$/).click();
  await screen.getByRole('menuitem', 'Sign out').click();
  await expect(browser).toHaveURL(/#\/signin/);
  expect(await browser.evaluate(() => sessionStorage.getItem('yulius.places'))).toBeNull();
});

test('a file opened just before a sandbox switch shows when the sandbox is back', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/t/12', '&hold=/api/sandboxes/sb_studio/files/content'));
  await screen.getByRole('button', /^Show tools/).click();
  await screen.getByRole('button', 'Editor').click();
  await screen.getByRole('tree', 'Files').getByRole('treeitem', 'README.md').click();
  await tab(screen, 'product').click();
  await expect(heading(screen)).toHaveText('New task');
  await browser.evaluate(() => (window as unknown as { __yuliusMock: { release: () => void } }).__yuliusMock.release());
  // The answer lands while studio is in the background: its tab exists, hidden, with no model.
  await expect.poll(() => browser.evaluate(() => document.querySelectorAll('.ed-tab').length)).toBe(1);
  await tab(screen, 'studio').click();
  await expect(screen.getByRole('tab', /^README\.md/)).toHaveAttribute('aria-selected', 'true');
  await expect(screen.getByRole('textbox', 'Editor content')).toBeAttached();
});

// Review round 2: a filter kept in the address and the page in view agree after a plain link.
test('the link to the page in view resets a filter kept in the address', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/dashboard'));
  await screen.getByRole('radiogroup', 'Date range').getByText('7 days').click();
  await expect(browser).toHaveURL(/#\/s\/studio\/dashboard\?days=7$/);
  await screen.getByRole('navigation', 'Sandbox').getByRole('link', 'Dashboard').click();
  await expect(browser).toHaveURL(/#\/s\/studio\/dashboard$/);
  await expect(screen.getByRole('radiogroup', 'Date range').getByRole('radio', '30 days')).toBeChecked();
});

test('leaving a new skill unsaved for another sandbox goes where that sandbox was left', async ({ app, browser, screen }) => {
  await app.open(mock('/s/product/git'));
  await expect(heading(screen)).toHaveText('Git');
  await browser.evaluate(() => { window.location.hash = '#/s/studio/skills/new'; });
  await screen.getByRole('textbox', /name/i).first().fill('half-written');
  await tab(screen, 'product').click();
  await screen.getByRole('dialog', 'Leave without creating?').getByRole('button', 'Leave').click();
  await expect(browser).toHaveURL(/#\/s\/product\/git$/);
});

test('the Git back link of a task’s changes returns to Git', async ({ app, browser }) => {
  await app.open(mock('/s/studio/git/t/12'));
  await browser.locator('.gt-back').click();
  await expect(browser).toHaveURL(/#\/s\/studio\/git$/);
});
