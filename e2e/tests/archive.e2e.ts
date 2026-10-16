import { test, type Browser } from '@e2e-dev/web';
import { expect } from 'e2e';
import { axeViolations, failOnPageErrors, mock, terminalShown } from './support.ts';

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

  await terminalShown(browser);
  await quick.focus();
  await browser.keyboard.press('Enter');
  await expect(screen.getByText('Task #13 archived.')).toBeVisible();
  await expect(done.getByRole('link', /Improve the code/)).toBeHidden();
  expect(await archived(browser, 13)).toBe(true);
  // Its row left with the keyboard on it: the keyboard stays in the task list.
  await expect.poll(() => browser.evaluate(() => Boolean(document.activeElement?.closest('.task-list')))).toBe(true);
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

  // From the keyboard: once the group is gone, the keyboard stays in the task list. The task's
  // terminal takes the keyboard when it shows: the keyboard moves on after that.
  await terminalShown(browser);
  await done.getByRole('button', 'Archive all').focus();
  await browser.keyboard.press('Enter');
  await expect(screen.getByText('Archived 2 Done tasks.')).toBeVisible();
  await expect(done).toBeHidden();
  await expect.poll(() => browser.evaluate(() => Boolean(document.activeElement?.closest('.task-list')))).toBe(true);
  expect([await archived(browser, 13), await archived(browser, 11)]).toEqual([true, true]);

  await screen.getByRole('button', 'Undo').click();
  await expect(screen.getByText('Restored 2 tasks.')).toBeVisible();
  await expect.poll(async () => [await archived(browser, 13), await archived(browser, 11)]).toEqual([false, false]);
});

test('Archive all keeps what it archived when one fails and says how many failed', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/t/12', '&failpost=/api/tasks/tsk_studio_11/archive'));
  const done = screen.getByRole('region', 'Done');
  // Both Done tasks are listed: a task is Done once its pull request has been read, which can
  // come after the first list (SPEC-082 TST-003; before, Archive all could find one task only).
  await expect(done.getByRole('link')).toHaveCount(2);
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
  // The task's terminal takes the keyboard when it shows: the keyboard moves on after that.
  await terminalShown(browser);

  const all = done.getByRole('button', 'Archive all');
  await all.focus();
  await browser.keyboard.press('Enter');
  await expect(all).toHaveAttribute('aria-disabled', 'true');
  await expect(all).toBeFocused();
  await expect.poll(() => archiveRequests(browser)).toBe(2);
  // A second click and a row's quick Archive send nothing more while the batch runs.
  await browser.keyboard.press('Enter');
  await browser.locator('.task-row:has-text("Improve the code")').hover();
  await screen.getByRole('button', 'Archive task #13').click();
  expect(await archiveRequests(browser)).toBe(2);

  // Back on Archive all, the keyboard stays there while rows leave and the batch ends.
  await all.focus();
  await release();
  await expect(screen.getByText('Couldn’t archive 1 of 2 tasks: The mock server failed on purpose')).toBeVisible();
  await expect(done.getByRole('link', /Improve the code/)).toBeHidden();
  // The button is back for the one that failed, with the keyboard still on it.
  await expect(all).not.toHaveAttribute('aria-disabled', 'true');
  await expect(all).toBeFocused();
  expect(await archiveRequests(browser)).toBe(2);
});

test('read access shows neither the quick Archive nor Archive all', async ({ app, browser, screen }) => {
  await app.open(mock('/s/sandbox-3'));
  // A stopped task in a sandbox the user can only read.
  await browser.evaluate(() => (window as unknown as WithMock).__yuliusMock.touchTask('sb_three', 1, { status: 'stopped_cancelled', activity: null }));
  const row = screen.getByRole('region', 'Stopped').getByRole('listitem').filter({ hasText: 'Explore data model' });
  await row.hover();
  await expect(row.getByRole('button', 'Task actions')).toBeVisible();
  await expect(browser.locator('.task-row:has-text("Explore data model") .task-quick-archive')).toBeHidden();
  // Now Done: the group is there, without Archive all.
  await browser.evaluate(() => (window as unknown as WithMock).__yuliusMock.touchTask('sb_three', 1, { status: 'running', activity: 'done' }));
  await expect(screen.getByRole('region', 'Done').getByRole('link', /Explore data model/)).toBeVisible();
  await expect(screen.getByRole('button', 'Archive all')).toHaveCount(0);
});

// #356: the Stopped toggle's state is the sidebar's own (`endedOpen`), never the list's: a rebuild
// while it is folded and focused (another task changing group) keeps it folded and focused, and
// Enter still unfolds it.
test('the Stopped toggle keeps its state and the keyboard when the list is rebuilt between presses', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/t/12'));
  await terminalShown(browser);
  const stopped = screen.getByRole('region', 'Stopped').getByRole('button', /Stopped/);
  await stopped.focus();
  await browser.keyboard.press('Enter');
  await expect(stopped).toHaveAttribute('aria-expanded', 'false');
  const head = '.task-group.group-ended .task-group-head';
  await browser.evaluate((css) => { (document.querySelector(css) as HTMLElement).dataset.before = 'rebuild'; }, head);
  await browser.evaluate(() => (window as unknown as WithMock).__yuliusMock.touchTask('sb_studio', 14, { background_shells: 0 }));
  await expect(screen.getByRole('region', 'Done').getByRole('link', /Start the preview server/)).toBeVisible();
  // The list was rebuilt: the toggle is a new element.
  expect(await browser.evaluate((css) => (document.querySelector(css) as HTMLElement).dataset.before ?? null, head)).toBeNull();
  await expect(stopped).toHaveAttribute('aria-expanded', 'false');
  await expect(stopped).toBeFocused();
  await browser.keyboard.press('Enter');
  await expect(stopped).toHaveAttribute('aria-expanded', 'true');
  await expect(stopped).toBeFocused();
});

test('the keyboard stays on its control when the sidebar list is rebuilt', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/t/12'));
  // The task's terminal takes the keyboard when it shows: the keyboard moves on after that.
  await terminalShown(browser);
  // The Stopped toggle keeps the keyboard: Enter folds it, Enter again unfolds it, no task opens.
  const stopped = screen.getByRole('region', 'Stopped').getByRole('button', /Stopped/);
  await stopped.focus();
  await browser.keyboard.press('Enter');
  await expect(stopped).toHaveAttribute('aria-expanded', 'false');
  await expect(stopped).toBeFocused();
  await browser.keyboard.press('Enter');
  await expect(stopped).toHaveAttribute('aria-expanded', 'true');
  await expect(stopped).toBeFocused();
  await expect(screen.getByRole('heading', { level: 1 })).toHaveText('#12 Checkout API');

  // A row keeps it while another task changes group.
  const link = screen.getByRole('region', 'Working').getByRole('link', /Checkout API/);
  await link.focus();
  await browser.evaluate(() => (window as unknown as WithMock).__yuliusMock.touchTask('sb_studio', 14, { background_shells: 0 }));
  await expect(screen.getByRole('region', 'Done').getByRole('link', /Start the preview server/)).toBeVisible();
  await expect(link).toBeFocused();

  // After a quick Archive the keyboard moves to the next row of the group: a task's link (#331).
  await browser.locator('.task-row:has-text("Start the preview server")').hover();
  const quick = screen.getByRole('button', 'Archive task #14');
  await quick.focus();
  await browser.keyboard.press('Enter');
  await expect(screen.getByText('Task #14 archived.')).toBeVisible();
  await expect.poll(() => browser.evaluate(() => {
    const el = document.activeElement;
    return el?.matches('.task-link') ? el.closest('.task-group')?.getAttribute('aria-label') ?? null : null;
  })).toBe('Done');
});

// #331: the keyboard on a stopped row's quick Archive, and the task is continued (from another
// tab, an agent): the button hides as its row moves to Working, and the keyboard goes to the row's
// link instead of falling to the page.
test('the keyboard on a quick Archive that hides moves to its row’s link', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/t/12'));
  await terminalShown(browser);
  const stopped = screen.getByRole('region', 'Stopped');
  await expect(stopped.getByRole('link', /Payment webhook/)).toBeVisible();
  await browser.locator('.task-row:has-text("Payment webhook")').hover();
  const quick = screen.getByRole('button', 'Archive task #4');
  await quick.focus();
  await expect(quick).toBeFocused();
  await browser.evaluate(() => (window as unknown as WithMock).__yuliusMock.touchTask('sb_studio', 4, { status: 'running', activity: 'working' }));
  const link = screen.getByRole('region', 'Working').getByRole('link', /Payment webhook/);
  await expect(link).toBeVisible();
  await expect(link).toBeFocused();
});
