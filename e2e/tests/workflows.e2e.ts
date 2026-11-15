import { test } from '@e2e-dev/web';
import { expect } from 'e2e';
import { axeViolations, failOnPageErrors, mock } from './support.ts';

failOnPageErrors();

// Default workflows (SPEC-001 AUT-006, UI-034; #136): they ship with the server, come first, are
// read-only and are customised by duplicating them.

test('the defaults come first and read-only; Duplicate starts a custom copy', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/workflows'));
  await expect(screen.getByRole('heading', { level: 1 })).toHaveText('Workflows');
  await expect(screen.getByRole('heading', /^Defaults/)).toBeVisible();
  await expect(screen.getByRole('heading', /^Custom/)).toBeVisible();
  for (const name of ['Feature', 'Fix', 'Research']) {
    await expect(screen.getByRole('button', `Open workflow ${name}`)).toBeVisible();
    await expect(screen.getByRole('button', `Delete ${name}`)).toBeHidden();
  }
  await expect(screen.getByText('Use when: The task adds or changes behaviour that users or other code rely on.')).toBeVisible();
  await expect(screen.getByRole('button', 'Delete Release notes')).toBeVisible();

  // A default opens read-only, offering only a copy.
  await screen.getByRole('button', 'Open workflow Feature').click();
  await expect(browser).toHaveURL(/#\/s\/studio\/workflows\/wf-feature$/);
  await expect(screen.getByText(/A default workflow: it ships with Yulius and stays current with each release\./)).toBeVisible();
  await expect(screen.getByRole('button', 'Save')).toBeHidden();
  await expect(screen.getByRole('button', 'Delete')).toBeHidden();
  expect(await axeViolations(browser)).toEqual([]);

  await screen.getByRole('button', 'Duplicate to customise').click();
  await expect(browser).toHaveURL(/#\/s\/studio\/workflows\/new\?from=wf-feature$/);
  await expect(screen.getByRole('textbox', 'Workflow name')).toHaveValue('Feature (custom)');
  await expect(screen.getByRole('button', 'Write the spec')).toBeVisible();
  await screen.getByRole('button', 'Create workflow').click();
  await expect(browser).toHaveURL(/#\/s\/studio\/workflows$/);
  await expect(screen.getByRole('button', 'Edit workflow Feature (custom)')).toBeVisible();
  await expect(screen.getByRole('button', 'Delete Feature (custom)')).toBeVisible();
});

// SPEC-483 slice 28 (W4): an editor with unsaved changes asks before every way out, not only a
// click on a link: Back, and a navigation made by code (a shortcut, the search, a task closing).
// One guard in the router does it for the five editor pages that each had their own.
test('unsaved changes are asked about on Back and on navigation by code too', async ({ app, browser, screen }) => {
  const go = (path: string) => browser.evaluate(async (to) => {
    const { navigate } = await import('/assets/router.js' as string);
    navigate(to);
  }, path);
  const length = () => browser.evaluate(() => window.history.length);
  const leaveDialog = () => screen.getByRole('dialog', 'Leave without saving?');
  const name = screen.getByRole('textbox', 'Workflow name');
  const heading = screen.getByRole('heading', { level: 1 });
  await app.open(mock('/s/studio/tasks'));
  await expect(heading).toHaveText(/Tasks/);
  await go('/s/studio/workflows');
  await expect(heading).toHaveText('Workflows');
  await go('/s/studio/workflows/new?from=wf-feature');
  await expect(name).toHaveValue('Feature (custom)');
  await name.fill('Feature (mine)');
  const entries = await length();

  // Back: the page stays and asks; Cancel keeps the text and the history as it was.
  await browser.evaluate(() => window.history.back());
  await expect(leaveDialog()).toBeVisible();
  await expect(browser).toHaveURL(/workflows\/new\?from=wf-feature$/);
  await leaveDialog().getByRole('button', 'Cancel').click();
  await expect(name).toHaveValue('Feature (mine)');
  await expect(browser).toHaveURL(/workflows\/new\?from=wf-feature$/);
  expect(await length()).toBe(entries);

  // Hash mode sends two events for one move (popstate, hashchange): the second, arriving before
  // the move is undone, changes nothing.
  await browser.evaluate(() => {
    window.addEventListener('popstate', () => window.dispatchEvent(new HashChangeEvent('hashchange')), { once: true });
    window.history.back();
  });
  await expect(leaveDialog()).toBeVisible();
  await leaveDialog().getByRole('button', 'Cancel').click();
  await expect(name).toHaveValue('Feature (mine)');
  await expect(browser).toHaveURL(/workflows\/new\?from=wf-feature$/);
  expect(await length()).toBe(entries);

  // Asked again, Leave goes back, and the history is the browser's own: Back once more reaches
  // the page before, Forward the editor.
  await browser.evaluate(() => window.history.back());
  await leaveDialog().getByRole('button', 'Leave').click();
  await expect(heading).toHaveText('Workflows');
  await expect(browser).toHaveURL(/workflows$/);
  expect(await length()).toBe(entries);
  await browser.evaluate(() => window.history.forward());
  await expect(name).toHaveValue('Feature (custom)');
  await expect(leaveDialog()).toBeHidden();

  // Forward is asked about too; after Cancel, Back and Leave go to the page before the editor,
  // not to the page Forward was declined.
  await browser.evaluate(() => window.history.back());
  await expect(heading).toHaveText('Workflows');
  await browser.evaluate(() => window.history.back());
  await expect(heading).toHaveText(/Tasks/);
  await browser.evaluate(() => window.history.forward());
  await expect(heading).toHaveText('Workflows');
  await browser.evaluate(() => window.history.forward());
  await expect(name).toHaveValue('Feature (custom)');
  await go('/s/studio/workflows/new?from=wf-feature&x=1');
  await expect(name).toHaveValue('Feature (custom)');
  await browser.evaluate(() => window.history.back());
  await expect(browser).toHaveURL(/workflows\/new\?from=wf-feature$/);
  await name.fill('Feature (forward)');
  await browser.evaluate(() => window.history.forward());
  await expect(leaveDialog()).toBeVisible();
  await leaveDialog().getByRole('button', 'Cancel').click();
  await expect(browser).toHaveURL(/workflows\/new\?from=wf-feature$/);
  await expect(name).toHaveValue('Feature (forward)');
  await browser.evaluate(() => window.history.back());
  await leaveDialog().getByRole('button', 'Leave').click();
  await expect(heading).toHaveText('Workflows');
  await expect(browser).toHaveURL(/workflows$/);

  // A navigation by code asks the same; Leave to the address in view keeps the page, and its
  // guard: a later change is asked about again.
  await browser.evaluate(() => window.history.forward());
  await expect(name).toHaveValue('Feature (custom)');
  await name.fill('Feature (code)');
  await go('/s/studio/workflows/new?from=wf-feature');
  await leaveDialog().getByRole('button', 'Leave').click();
  await expect(leaveDialog()).toBeHidden();
  await name.fill('Feature (again)');
  await go('/s/studio/tasks');
  await expect(leaveDialog()).toBeVisible();
  await leaveDialog().getByRole('button', 'Leave').click();
  await expect(heading).toHaveText(/Tasks/);

  // Saved or left, nothing is asked any more.
  await browser.evaluate(() => window.history.back());
  await expect(name).toBeVisible();
  await expect(screen.getByRole('dialog')).toBeHidden();
});

// SPEC-483 slice 28 (review): the tab's first entry is a place like any other (entry 0).
test('Forward from the first page of the tab is asked about and undone once', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/workflows/new?from=wf-feature'));
  const name = screen.getByRole('textbox', 'Workflow name');
  await expect(name).toHaveValue('Feature (custom)');
  await browser.evaluate(async () => {
    const { navigate } = await import('/assets/router.js' as string);
    navigate('/s/studio/workflows');
  });
  await expect(screen.getByRole('heading', { level: 1 })).toHaveText('Workflows');
  await browser.evaluate(() => window.history.back());
  await expect(name).toHaveValue('Feature (custom)');
  await name.fill('Feature (first)');
  await browser.evaluate(() => {
    window.addEventListener('popstate', () => window.dispatchEvent(new HashChangeEvent('hashchange')), { once: true });
    window.history.forward();
  });
  const dialog = screen.getByRole('dialog', 'Leave without saving?');
  await expect(dialog).toBeVisible();
  await dialog.getByRole('button', 'Cancel').click();
  await expect(browser).toHaveURL(/workflows\/new\?from=wf-feature$/);
  await expect(name).toHaveValue('Feature (first)');
  await browser.evaluate(() => window.history.forward());
  await screen.getByRole('dialog', 'Leave without saving?').getByRole('button', 'Leave').click();
  await expect(screen.getByRole('heading', { level: 1 })).toHaveText('Workflows');
});

// SPEC-483 slice 28 (CI): an editor still loading has nothing unsaved; leaving it asks nothing.
test('an editor left before it has loaded leaves without a question', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/tasks'));
  await expect(screen.getByRole('heading', { level: 1 })).toHaveText(/Tasks/);
  for (const path of ['/s/studio/automations/aut_1', '/s/studio/automations/new', '/s/studio/mcp/mcp_apify', '/s/studio/workflows/wf-feature', '/s/studio/skills/new']) {
    await browser.evaluate(async (to) => {
      const { navigate } = await import('/assets/router.js' as string);
      navigate(to);
      navigate('/s/studio/tasks');
    }, path);
    await expect(screen.getByRole('heading', { level: 1 })).toHaveText(/Tasks/);
    await expect(screen.getByRole('dialog')).toBeHidden();
  }
});

// SPEC-483 slice 28 (review N1): an address typed with unsaved work (hash mode) is put back; the
// typed entry keeps its place in the history, so later moves across it are counted right.
test('an address typed over unsaved work keeps the history countable', async ({ app, browser, screen }) => {
  const name = screen.getByRole('textbox', 'Workflow name');
  const heading = screen.getByRole('heading', { level: 1 });
  const leaveDialog = () => screen.getByRole('dialog', 'Leave without saving?');
  await app.open(mock('/s/studio/tasks'));
  await expect(heading).toHaveText(/Tasks/);
  await browser.evaluate(async () => {
    const { navigate } = await import('/assets/router.js' as string);
    navigate('/s/studio/workflows/new?from=wf-feature');
  });
  await expect(name).toHaveValue('Feature (custom)');
  await name.fill('Feature (typed)');
  await browser.evaluate(() => { window.location.hash = '#/s/studio/sessions'; });
  await leaveDialog().getByRole('button', 'Leave').click();
  await expect(browser).toHaveURL(/sessions$/);

  await browser.evaluate(() => window.history.back());
  await expect(name).toHaveValue('Feature (custom)');
  await browser.evaluate(() => window.history.back());
  await expect(browser).toHaveURL(/sessions$/);
  await expect(heading).toHaveText(/Sessions/);
  await browser.evaluate(() => window.history.forward());
  await expect(name).toHaveValue('Feature (custom)');
  await name.fill('Feature (again)');
  await browser.evaluate(() => window.history.back());
  await leaveDialog().getByRole('button', 'Cancel').click();
  await expect(name).toHaveValue('Feature (again)');
  await browser.evaluate(() => window.history.back());
  await leaveDialog().getByRole('button', 'Leave').click();
  await expect(browser).toHaveURL(/sessions$/);
  await expect(heading).toHaveText(/Sessions/);
});
