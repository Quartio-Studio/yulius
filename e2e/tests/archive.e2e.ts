import { test, type Browser } from '@e2e-dev/web';
import { expect } from 'e2e';
import { axeViolations, failOnPageErrors, mock, mockReady } from './support.ts';

failOnPageErrors();

// F 2026-10-13 (task #65, SPEC-062): "i want a quick button to archive tasks and 'archive all'
// for done".

/** Whether the mock holds the studio task `number` as archived. */
const archived = (browser: Browser, number: number) =>
  browser.evaluate((n) => (window as unknown as WithMock).__yuliusMock.tasks.find((t) => t.sandbox_id === 'sb_studio' && t.number === n)?.archived, number);
type WithMock = { __yuliusMock: {
  tasks: { number: number; sandbox_id: string; archived: boolean }[];
  touchTask: (sandbox: string, number: number, changes: unknown) => void;
  holdArchive: Promise<void> | null;
  archiveRequests: number;
} };

/** Holds the mock's archive answers until the returned function is called. */
const holdArchives = async (browser: Browser) => {
  await mockReady(browser);
  await browser.evaluate(() => {
    const mock = (window as unknown as WithMock & { __release?: () => void }).__yuliusMock;
    mock.holdArchive = new Promise((resolve) => { (window as unknown as { __release?: () => void }).__release = resolve; });
  });
  return () => browser.evaluate(() => (window as unknown as { __release: () => void }).__release());
};
const archiveRequests = (browser: Browser) => browser.evaluate(() => (window as unknown as WithMock).__yuliusMock.archiveRequests);

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
  const quick = browser.locator('.task-row:has-text("Improve the code") [aria-label="Archive task #13"]');
  await expect(quick).toHaveCount(1);
  await expect(quick).toBeHidden();
  // The group's Archive all is there, a full-size touch target.
  const all = screen.getByRole('region', 'Done').getByRole('button', 'Archive all');
  await expect(all).toBeVisible();
  expect(await browser.evaluate(() => document.querySelector('.task-group.group-done .task-group-action')!.getBoundingClientRect().height)).toBeGreaterThanOrEqual(40);
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

test('Archive all stays focused and disabled while it runs; a row cannot archive the same task again', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/t/12', '&failpost=/api/tasks/tsk_studio_11/archive'));
  const done = screen.getByRole('region', 'Done');
  await expect(done.getByRole('link')).toHaveCount(2);
  const release = await holdArchives(browser);

  const all = done.getByRole('button', 'Archive all');
  await all.focus();
  await browser.keyboard.press('Enter');
  await expect(all).toHaveAttribute('aria-disabled', 'true');
  await expect(all).toBeFocused();
  await expect.poll(() => archiveRequests(browser)).toBe(1);
  // A second click and a row's quick Archive send nothing more while the batch runs.
  await browser.keyboard.press('Enter');
  await browser.locator('.task-row:has-text("Improve the code")').hover();
  await screen.getByRole('button', 'Archive task #13').click();
  expect(await archiveRequests(browser)).toBe(1);

  // Back on Archive all, the keyboard stays there while rows leave and the batch ends.
  await all.focus();
  await release();
  await expect(screen.getByText('Couldn’t archive 1 of 2 tasks: The mock server failed on purpose')).toBeVisible();
  await expect(done.getByRole('link', /Improve the code/)).toBeHidden();
  // The button is back for the one that failed, with the keyboard still on it.
  await expect(all).not.toHaveAttribute('aria-disabled', 'true');
  await expect(all).toBeFocused();
  expect(await archiveRequests(browser)).toBe(1);
});

test('read access shows neither the quick Archive nor Archive all', async ({ app, browser, screen }) => {
  await app.open(mock('/s/sandbox-3'));
  await mockReady(browser);
  // A stopped task in a sandbox the user can only read.
  await browser.evaluate(() => (window as unknown as WithMock).__yuliusMock.touchTask('sb_three', 1, { status: 'stopped_cancelled', activity: null }));
  const row = screen.getByRole('region', 'Stopped').getByRole('listitem').filter({ hasText: 'Explore data model' });
  await row.hover();
  await expect(row.getByRole('button', 'Task actions')).toBeVisible();
  await expect(browser.locator('.task-row:has-text("Explore data model") .task-quick-archive')).toBeHidden();
  await expect(screen.getByRole('button', 'Archive all')).toHaveCount(0);
});
