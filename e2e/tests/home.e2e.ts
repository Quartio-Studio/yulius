import { test } from '@e2e-dev/web';
import { expect } from 'e2e';
import { axeViolations, failOnPageErrors, mock } from './support.ts';

failOnPageErrors();

// SPEC-001 UI-045 (F 2026-10-10): "I'd like to have main screen with sandboxes as default view".
test('the home screen is the default view and lists every sandbox as a card', async ({ app, browser, screen }) => {
  await app.open(mock('/'));

  await expect(screen.getByRole('heading', { level: 1 })).toHaveText('Sandboxes');
  // No redirect into the sandbox used last.
  await expect(browser).toHaveURL(/#\/$/);
  for (const [name, repo] of [['studio', 'Quartio-Studio/studio'], ['product', 'Quartio-Studio/product'], ['sandbox-3', 'Quartio-Studio/sandbox-3']]) {
    const card = screen.getByRole('article', name);
    await expect(card).toBeVisible();
    await expect(card.getByText(repo)).toBeVisible();
    // The sandbox's counters, in words: "2 need you 3 running 1 queued".
    await expect(card.getByRole('list', `Tasks in ${name}`)).toHaveText(/^\d+ needs? you \d+ running \d+ queued$/);
    await expect(card.getByText(/^Last activity /)).toBeVisible();
  }
  // Read access cannot start tasks.
  await expect(screen.getByRole('button', 'New task in sandbox-3')).toBeDisabled();
  // The sandbox tabs stay, with All sandboxes current.
  await expect(screen.getByRole('navigation', 'Sandboxes').getByRole('link', 'All sandboxes')).toHaveAttribute('aria-current', 'page');

  expect(await axeViolations(browser)).toEqual([]);
});

test('a card starts a new task in its sandbox or opens it', async ({ app, browser, screen }) => {
  await app.open(mock('/'));

  await screen.getByRole('button', 'New task in product').click();
  await expect(browser).toHaveURL(/#\/s\/product$/);
  await expect(screen.getByRole('textbox', 'Task prompt')).toBeFocused();

  // The logo goes home.
  await screen.getByRole('link', /all sandboxes$/).click();
  await expect(browser).toHaveURL(/#\/$/);
  await expect(screen.getByRole('heading', { level: 1 })).toHaveText('Sandboxes');

  await screen.getByRole('link', 'Open studio').click();
  await expect(browser).toHaveURL(/#\/s\/studio$/);
  await expect(screen.getByRole('heading', { level: 1 })).toHaveText('New task');
});

test('deep links open directly; the All sandboxes tab and the keyboard shortcut go home', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/t/12'));
  await expect(screen.getByRole('heading', { level: 1 })).toHaveText('#12 Checkout API');

  // ⌘⇧H / Ctrl+Shift+H, also from the task's terminal.
  await browser.keyboard.press('ControlOrMeta+Shift+H');
  await expect(browser).toHaveURL(/#\/$/);
  await expect(screen.getByRole('heading', { level: 1 })).toHaveText('Sandboxes');

  await app.open(mock('/s/product/tasks'));
  await expect(screen.getByRole('heading', { level: 1 })).toHaveText('Tasks');
  await screen.getByRole('navigation', 'Sandboxes').getByRole('link', 'All sandboxes').click();
  await expect(browser).toHaveURL(/#\/$/);

  // The shortcut sheet lists it.
  await browser.keyboard.press('ControlOrMeta+/');
  const sheet = screen.getByRole('dialog', 'Keyboard shortcuts');
  await expect(sheet.getByText('All sandboxes (home)')).toBeVisible();
  await browser.keyboard.press('Escape');
});

test('with no sandbox the home screen offers to import or create one', async ({ app, browser, screen }) => {
  await app.open(mock('/', '&sandboxes=0'));

  await expect(screen.getByRole('heading', { level: 1 })).toHaveText('No sandboxes yet');
  await expect(screen.getByRole('button', 'Import repository')).toBeEnabled();
  await expect(screen.getByRole('button', 'Create from template')).toBeEnabled();
  expect(await axeViolations(browser)).toEqual([]);
});
