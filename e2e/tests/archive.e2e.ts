import { test, type Browser } from '@e2e-dev/web';
import { expect } from 'e2e';
import { axeViolations, failOnPageErrors, mock } from './support.ts';

failOnPageErrors();

// F 2026-10-13 (task #65, SPEC-062): "i want a quick button to archive tasks and 'archive all'
// for done".

/** Whether the mock holds the studio task `number` as archived. */
const archived = (browser: Browser, number: number) =>
  browser.evaluate((n) => (window as unknown as WithMock).__yuliusMock.tasks.find((t) => t.sandbox_id === 'sb_studio' && t.number === n)?.archived, number);
type WithMock = { __yuliusMock: { tasks: { number: number; sandbox_id: string; archived: boolean }[] } };

test('a row’s quick Archive button archives its task, with Undo', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/t/13'));
  await expect(screen.getByRole('heading', { level: 1 })).toHaveText('#13 Improve the code');
  const done = screen.getByRole('region', 'Done');

  // A working task cannot be archived: its row has no such button.
  await browser.locator('.task-row:has-text("Checkout API")').hover();
  await expect(browser.locator('.task-row:has-text("Checkout API") [aria-label^="Archive task"]')).toBeHidden();

  // A Done one shows it on hover, beside the task menu's button.
  const row = browser.locator('.task-row:has-text("Improve the code")');
  await row.hover();
  const quick = screen.getByRole('button', 'Archive task #13');
  await expect(quick).toBeVisible();
  expect(await axeViolations(browser)).toEqual([]);

  await quick.click();
  await expect(screen.getByText('Task #13 archived.')).toBeVisible();
  await expect(done.getByRole('link', /Improve the code/)).toBeHidden();
  expect(await archived(browser, 13)).toBe(true);
  // The open task was archived: its page is left for the sandbox's.
  await expect(screen.getByRole('heading', { level: 1, name: '#13 Improve the code' })).toBeHidden();

  // Undo restores it. Archiving closed its finished session (TSK-014), so it is back under Tasks,
  // not in the sidebar's active list.
  await screen.getByRole('button', 'Undo').click();
  await expect.poll(() => archived(browser, 13)).toBe(false);
});

test('a phone keeps one row action: no quick Archive beside the task menu', async ({ app, browser, screen }) => {
  await browser.setViewport({ width: 390, height: 844 });
  await app.open(mock('/s/studio/t/12'));
  await screen.getByRole('button', 'Open navigation').click();
  await expect(screen.getByRole('region', 'Done').getByRole('link', /Improve the code/)).toBeVisible();
  // Focus in the row would show a desktop row's actions.
  await browser.locator('.task-row:has-text("Improve the code") .task-link').focus();
  await expect(browser.locator('.task-row:has-text("Improve the code") [aria-label="Task actions"]')).toBeVisible();
  await expect(browser.locator('.task-row:has-text("Improve the code") [aria-label="Archive task #13"]')).toBeHidden();
  // The group's Archive all is there.
  await expect(screen.getByRole('region', 'Done').getByRole('button', 'Archive all')).toBeVisible();
});

test('Archive all archives every Done task in one click, with one Undo', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/t/12'));
  const done = screen.getByRole('region', 'Done');
  await expect(done.getByRole('link')).toHaveCount(2);
  // Only the Done group has it.
  await expect(screen.getByRole('region', 'Working').getByRole('button', 'Archive all')).toHaveCount(0);
  await expect(screen.getByRole('region', 'Stopped').getByRole('button', 'Archive all')).toHaveCount(0);
  expect(await axeViolations(browser)).toEqual([]);

  await done.getByRole('button', 'Archive all').click();
  await expect(screen.getByText('Archived 2 Done tasks.')).toBeVisible();
  await expect(done).toBeHidden();
  expect([await archived(browser, 13), await archived(browser, 11)]).toEqual([true, true]);

  await screen.getByRole('button', 'Undo').click();
  await expect.poll(async () => [await archived(browser, 13), await archived(browser, 11)]).toEqual([false, false]);
});

test('Archive all keeps what it archived when one fails and says how many failed', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/t/12', '&failpost=/api/tasks/tsk_studio_11/archive'));
  const done = screen.getByRole('region', 'Done');
  await done.getByRole('button', 'Archive all').click();

  await expect(screen.getByText(/Couldn’t archive 1 of 2 tasks: The mock server failed on purpose/)).toBeVisible();
  await expect(done.getByRole('link', /3D model viewer/)).toBeVisible();
  await expect(done.getByRole('link', /Improve the code/)).toBeHidden();
  expect([await archived(browser, 13), await archived(browser, 11)]).toEqual([true, false]);
  // The one that failed can be tried again.
  await expect(done.getByRole('button', 'Archive all')).toBeEnabled();
});
