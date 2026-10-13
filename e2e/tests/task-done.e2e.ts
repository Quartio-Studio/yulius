import { test, type Browser } from '@e2e-dev/web';
import { expect } from 'e2e';
import { axeViolations, failOnPageErrors, mock } from './support.ts';

failOnPageErrors();

// F 2026-10-10 (TSK-014): "when the task's goal is achieved and none left shell is running, the
// task should be marked as finished (with green 'v' icon). Atm it keeps running."

test('a task whose goal is achieved is Done with a green check', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/t/13'));
  await expect(screen.getByRole('heading', { level: 1 })).toHaveText('#13 Improve the code');

  // The header says Done with the check, although the pull request's checks still run.
  const state = browser.locator('.strip-status');
  await expect(state).toHaveText(/^Done/);
  await expect(state).toHaveAttribute('data-tone', 'done');
  await expect(browser.locator('.strip-status .glyph.tone-done')).toBeVisible();

  // The sidebar lists it under Done, not Working.
  await expect(screen.getByRole('region', 'Done').getByRole('link', /Improve the code/)).toBeVisible();
  await expect(screen.getByRole('region', 'Working').getByRole('link', /Improve the code/)).toBeHidden();

  expect(await axeViolations(browser)).toEqual([]);
});

test('a finished goal with a background shell still running stays Working and says why', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/t/14'));
  await expect(screen.getByRole('heading', { level: 1 })).toHaveText('#14 Start the preview server');

  await expect(browser.locator('.strip-status')).toHaveText(/Working · 1 background task still running/);
  await expect(screen.getByRole('region', 'Working').getByRole('link', /Start the preview server/)).toBeVisible();
  await expect(screen.getByRole('region', 'Done').getByRole('link', /Start the preview server/)).toBeHidden();
});

test('the Tasks page groups a task with an achieved goal under Done', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/tasks'));
  await expect(screen.getByRole('heading', { level: 1 })).toHaveText('Tasks');

  await expect(browser.locator('.tl-group.group-done')).toHaveText(/Improve the code/);
  await expect(browser.locator('.tl-group.group-working')).toHaveText(/Start the preview server/);
});

test('a task being stopped moves straight to Stopped, never into Working', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/t/12'));
  await expect(screen.getByRole('region', 'Working').getByRole('link', /Checkout API/)).toBeVisible();

  await browser.locator('.taskview .stepstrip-main [aria-label="Task actions"]').click();
  await screen.getByRole('menuitem', /^Stop/).click();
  await screen.getByRole('button', 'Stop task').click();

  // Whether it still reads "Stopping" (the process going away) or already "Stopped", the sidebar
  // lists it with the ended tasks, never under Working.
  await expect(screen.getByRole('region', 'Stopped').getByRole('link', /Checkout API/)).toBeVisible();
  await expect(screen.getByRole('region', 'Working').getByRole('link', /Checkout API/)).toBeHidden();
});

/** Changes a mock task as the server would announce it. */
const touchTask = (browser: Browser, number: number, changes: Record<string, unknown>) =>
  browser.evaluate(([n, c]) => (window as unknown as WithMock).__yuliusMock.touchTask('sb_studio', n, c), [number, changes] as const);
type WithMock = { __yuliusMock: { touchTask: (sandbox: string, number: number, changes: unknown) => void } };

test('an idle agent with a background shell running is Working, not your turn', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/t/12'));
  await expect(screen.getByRole('heading', { level: 1 })).toHaveText('#12 Checkout API');

  // The turn ended while the agent's build runs in the background: it wakes by itself.
  await touchTask(browser, 12, { activity: 'waiting', background_shells: 1 });
  await expect(browser.locator('.strip-status')).toHaveText(/Working · 1 background task still running/);
  await expect(screen.getByRole('region', 'Working').getByRole('link', /Checkout API/)).toBeVisible();
  await expect(browser.locator('.attention-bar')).toBeHidden();

  // The shell ended and nothing else runs: now the next move is the user's.
  await touchTask(browser, 12, { background_shells: 0 });
  await expect(browser.locator('.strip-status')).toHaveText(/Your turn/);
  await expect(browser.locator('.attention-bar')).toHaveText(/waiting for your reply/);
});

test('a finished task whose pull request is tested can be archived and is closed', async ({ app, browser, screen }) => {
  // #11 finished its turn with a complete plan and green checks: Done (#35), although it idles.
  await app.open(mock('/s/studio/t/11'));
  await expect(browser.locator('.strip-status')).toHaveText(/^Done · PR #221/);

  await browser.locator('.taskview .stepstrip-main [aria-label="Task actions"]').click();
  await screen.getByRole('menuitem', 'Archive').click();
  await expect(screen.getByText('Task #11 archived.')).toBeVisible();
  await expect(screen.getByRole('region', 'Done').getByRole('link', /3D model viewer/)).toBeHidden();
  const status = await browser.evaluate(() => (window as unknown as { __yuliusMock: { tasks: { number: number; sandbox_id: string; status: string }[] } }).__yuliusMock.tasks.find((t) => t.sandbox_id === 'sb_studio' && t.number === 11)?.status);
  expect(status).toBe('closed');
});

// #294: the pull requests are read on their own; when that read lands after the task view drew its
// strip (and after the task's detail), the strip follows it, as the sidebar does.
test('a task view reads Done once its pull request is read late', async ({ app, browser, screen }) => {
  type HoldingGit = Window & { __yuliusHoldGit?: Promise<void>; __releaseGit?: () => void };
  await browser.addInitScript(() => {
    const w = window as unknown as HoldingGit;
    w.__yuliusHoldGit = new Promise<void>((resolve) => { w.__releaseGit = resolve; });
  });
  await app.open(mock('/s/studio/t/11'));
  // The task's detail has been read and drawn: the strip's step list shows its steps.
  await browser.locator('.taskview .stepstrip-toggle').click();
  await expect(browser.locator('.taskview .step-item').first()).toBeVisible();
  // The mock's one browser request (task #10, about 5 s after the start) redraws the strip too;
  // it came before the release, so only the pull request read can make the strip Done.
  await expect(screen.getByRole('link', /^Needs you in the browser AWS login for deploy/)).toBeVisible({ timeout: 15_000 });
  const strip = browser.locator('.strip-status');
  await expect(strip).toHaveText('Your turn');
  await browser.evaluate(() => (window as unknown as HoldingGit).__releaseGit!());
  await expect(strip).toHaveText(/^Done · PR #221/);
});

// F 2026-10-12 (task #37, SPEC-039): "This task is done but I still can't delete it."
test('a Done task can be deleted from its menu; a working one says it has to be stopped first', async ({ app, browser, screen }) => {
  // #12 is still at work: Delete stays off and says why in words.
  await app.open(mock('/s/studio/t/12'));
  await browser.locator('.taskview .stepstrip-main [aria-label="Task actions"]').click();
  const blocked = screen.getByRole('menuitem', /^Delete permanently/);
  await expect(blocked).toHaveAttribute('aria-disabled', 'true');
  await expect(blocked).toHaveText(/Still running: stop the task before deleting it/);
  await browser.keyboard.press('Escape');

  // #13's goal is achieved: Done, although its session idles. Delete closes it and deletes it.
  await app.open(mock('/s/studio/t/13'));
  await expect(browser.locator('.strip-status')).toHaveText(/^Done/);
  await browser.locator('.taskview .stepstrip-main [aria-label="Task actions"]').click();
  await screen.getByRole('menuitem', /^Delete permanently/).click();
  await expect(screen.getByRole('dialog')).toHaveText(/finished session is closed first/);
  await screen.getByRole('button', 'Delete permanently').click();
  await expect(screen.getByText('Task #13 deleted.')).toBeVisible();
  await expect(screen.getByRole('region', 'Done').getByRole('link', /Improve the code/)).toBeHidden();
});
