import { test, type Browser } from '@e2e-dev/web';
import { expect } from 'e2e';
import { axeViolations, failOnPageErrors, mock, touchTask } from './support.ts';

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

/** Changes a mock task of the studio sandbox as the server would announce it. */
const touchStudioTask = (browser: Browser, number: number, changes: Record<string, unknown>) => touchTask(browser, 'sb_studio', number, changes);

test('an idle agent with a background shell running is Working, not your turn', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/t/12'));
  await expect(screen.getByRole('heading', { level: 1 })).toHaveText('#12 Checkout API');

  // The turn ended while the agent's build runs in the background: it wakes by itself.
  await touchStudioTask(browser, 12, { activity: 'waiting', background_shells: 1 });
  await expect(browser.locator('.strip-status')).toHaveText(/Working · 1 background task still running/);
  await expect(screen.getByRole('region', 'Working').getByRole('link', /Checkout API/)).toBeVisible();
  await expect(browser.locator('.attention-bar')).toBeHidden();

  // The shell ended and nothing else runs: now the next move is the user's.
  await touchStudioTask(browser, 12, { background_shells: 0 });
  await expect(browser.locator('.strip-status')).toHaveText(/Your turn/);
  await expect(browser.locator('.attention-bar')).toHaveText(/waiting for your reply/);
});

// SPEC-085 STT-002 (F 2026-10-13: "needs you was too often"): an agent that waits in `yulius wait`
// for its checks or its armed merge goes on by itself when Yulius has news.
test('an agent waiting for Yulius is Working, not your turn', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/t/12'));
  await expect(screen.getByRole('heading', { level: 1 })).toHaveText('#12 Checkout API');

  // The turn ended while the agent waits for its checks (its plan's merge step is still open).
  await touchStudioTask(browser, 12, { activity: 'waiting', yulius_wait: true });
  await expect(browser.locator('.strip-status')).toHaveText(/Waiting for checks or merge/);
  await expect(screen.getByRole('region', 'Working').getByRole('link', /Checkout API/)).toBeVisible();
  await expect(screen.getByRole('region', 'Needs you').getByRole('link', /Checkout API/)).toBeHidden();
  await expect(browser.locator('.attention-bar')).toBeHidden();

  // The wait returned and the agent stopped: now the next move is the user's.
  await touchStudioTask(browser, 12, { yulius_wait: false });
  await expect(browser.locator('.strip-status')).toHaveText(/Your turn/);
  await expect(screen.getByRole('region', 'Needs you').getByRole('link', /Checkout API/)).toBeVisible();
});

// SPEC-130 DMD-003 (#474, task #109 on 2026-10-14: "this is done but pr isnt purple"): its plan was
// complete and its pull request's checks green, but its agent still waited in `yulius wait` for
// the merge it had armed. Not Done before that merge: it goes on by itself.
test('a task waiting for its armed merge is not Done before it', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/t/11'));
  await expect(screen.getByRole('heading', { level: 1 })).toHaveText('#11 3D model viewer');
  // Its plan is complete and its pull request tested: with the agent stopped, that is Done.
  await expect(browser.locator('.strip-status')).toHaveText(/^Done · PR #221/);

  await touchStudioTask(browser, 11, { yulius_wait: true });
  await expect(browser.locator('.strip-status')).toHaveText(/Waiting for checks or merge/);
  await expect(screen.getByRole('region', 'Working').getByRole('link', /3D model viewer/)).toBeVisible();
  await expect(screen.getByRole('region', 'Done').getByRole('link', /3D model viewer/)).toBeHidden();

  // An achieved goal is Done whatever the wait still does.
  await touchStudioTask(browser, 11, { activity: 'done' });
  await expect(browser.locator('.strip-status')).toHaveText(/^Done · PR #221/);
  await expect(screen.getByRole('region', 'Done').getByRole('link', /3D model viewer/)).toBeVisible();
});

// SPEC-100 WTI-001 (task #85, #393: "git icon started rotating and became green"): the branch of a
// task waiting for Yulius stands still in the muted tone of the other waits, unlike the Working ring.
test('an agent waiting for Yulius shows a still, muted branch', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/t/12'));
  await expect(screen.getByRole('heading', { level: 1 })).toHaveText('#12 Checkout API');

  await touchStudioTask(browser, 12, { activity: 'waiting', yulius_wait: true });
  await expect(screen.getByRole('region', 'Working').getByRole('link', /Checkout API/)).toBeVisible();
  const marks = () => browser.evaluate(() => {
    const working = document.querySelector('.task-row:has(.task-mark .glyph[aria-label="Working"]) .task-mark .glyph');
    const waits = document.querySelector('.task-row:has(.task-mark .glyph[aria-label^="Waiting for checks or merge"]) .task-mark .glyph');
    const read = (glyph: Element | null) => glyph && {
      animation: getComputedStyle(glyph.querySelector('.icon') as Element).animationName,
      color: getComputedStyle(glyph).color,
    };
    return { working: read(working), waits: read(waits) };
  });
  await expect.poll(async () => (await marks()).waits !== null).toBe(true);
  const { working, waits } = await marks();
  // Another studio task is Working: its ring turns, in Working's colour.
  expect(working?.animation).toBe('spin');
  expect(waits?.animation).toBe('none');
  expect(waits?.color).not.toBe(working?.color);
});

// SPEC-085 STT-003 (task #63 on 2026-10-13: "Needs you" with failing checks while its agent had
// fixed them and waited for its review agent).
test('failing checks wait for the agent’s own work before they need you', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/t/4'));
  await expect(screen.getByRole('heading', { level: 1 })).toHaveText('#4 Payment webhook');

  // Its pull request's checks fail while a background agent of its idle session still runs.
  await touchStudioTask(browser, 4, { status: 'running', activity: 'waiting', background_shells: 1 });
  await expect(browser.locator('.strip-status')).toHaveText(/Working · PR #209 checks failing/);
  await expect(screen.getByRole('region', 'Working').getByRole('link', /Payment webhook/)).toBeVisible();
  await expect(screen.getByRole('region', 'Needs you').getByRole('link', /Payment webhook/)).toBeHidden();

  // Nothing runs any more and the agent stopped: the failing checks are the user's.
  await touchStudioTask(browser, 4, { background_shells: 0 });
  await expect(browser.locator('.strip-status')).toHaveText(/PR #209 · checks failing/);
  await expect(screen.getByRole('region', 'Needs you').getByRole('link', /Payment webhook/)).toBeVisible();
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
  await expect(screen.getByRole('link', /^Needs you in the browser Task #10 AWS login for deploy/)).toBeVisible({ timeout: 15_000 });
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

// #290 (SPEC-065 END-007): closing a Done task takes a few seconds; the UI says so and a second
// confirmation meanwhile sends nothing.
test('deleting a Done task says so while it runs and sends one request', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/t/13'));
  await expect(browser.locator('.strip-status')).toHaveText(/^Done/);
  await browser.evaluate(() => {
    const mocked = window as unknown as { __yuliusMock: { holdDelete?: Promise<void>; releaseDelete?: () => void } };
    mocked.__yuliusMock.holdDelete = new Promise<void>((resolve) => { mocked.__yuliusMock.releaseDelete = resolve; });
  });
  const confirmDelete = async () => {
    await browser.locator('.taskview .stepstrip-main [aria-label="Task actions"]').click();
    await screen.getByRole('menuitem', /^Delete permanently/).click();
    await screen.getByRole('button', 'Delete permanently').click();
  };
  await confirmDelete();
  await expect(screen.getByText('Deleting #13…')).toBeVisible();
  await confirmDelete();
  const requests = () => browser.evaluate(() => (window as unknown as { __yuliusMock: { deleteRequests?: number } }).__yuliusMock.deleteRequests || 0);
  expect(await requests()).toBe(1);

  await browser.evaluate(() => (window as unknown as { __yuliusMock: { releaseDelete: () => void } }).__yuliusMock.releaseDelete());
  await expect(screen.getByText('Task #13 deleted.')).toBeVisible();
  await expect(screen.getByText('Deleting #13…')).toBeHidden();
  expect(await requests()).toBe(1);
});

test('a task being stopped says Stopping… on its Delete item and can be stopped again', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/t/12'));
  await touchStudioTask(browser, 12, { status: 'stopping', activity: null });
  await browser.locator('.taskview .stepstrip-main [aria-label="Task actions"]').click();
  const item = screen.getByRole('menuitem', /^Delete permanently/);
  await expect(item).toHaveAttribute('aria-disabled', 'true');
  await expect(item).toHaveText(/Stopping…/);
  // A stop whose session could not be ended is retried from the same menu (SPEC-065).
  await expect(screen.getByRole('menuitem', /^Stop/)).not.toHaveAttribute('aria-disabled', 'true');
});

// SPEC-229 (task #215, 2026-10-19): after the host rebooted, idle and finished tasks read
// "Yulius restarted while the agent was working", which was untrue. A finished task now comes
// back Closed (server side); a stopped one says the restart ended its session.
test('a session a restart ended says so without claiming the agent was working (unit, in the page)', async ({ app, browser }) => {
  await app.open(mock('/s/studio'));
  const read = await browser.evaluate(async () => {
    const statusUrl = '/assets/status.js';
    const { endedMessage, statusInfo } = await import(statusUrl);
    const lost = endedMessage('stopped_server_lost');
    const closed = statusInfo({ status: 'closed', steps: { done: 8, total: 8 } });
    return { tone: lost.tone, text: lost.text, closed: [closed.label, closed.tone] };
  });
  expect(read).toEqual({
    tone: 'warn',
    text: 'Yulius restarted, which ended this session. Its branch and changes are safe.',
    closed: ['Closed', 'closed'],
  });
});

// SPEC-268 (task #254, 2026-10-22): a session that ended while its sandbox was at its process or
// memory limit says so instead of "The agent stopped unexpectedly"; the cause comes from the exit
// frame, or after a reload from the run's recorded exit reason.
test('a session that ended at a sandbox limit says which (unit, in the page)', async ({ app, browser }) => {
  await app.open(mock('/s/studio'));
  const read = await browser.evaluate(async () => {
    const statusUrl = '/assets/status.js';
    const { endedMessage, limitCause } = await import(statusUrl);
    return {
      pids: endedMessage('stopped_crashed', 'pids').text,
      memory: endedMessage('stopped_killed', 'memory').text,
      plain: endedMessage('stopped_crashed').text,
      cancelled: endedMessage('stopped_cancelled', 'pids').text,
      causes: [
        limitCause('exited code=None signal=Some(6) limit=pids'),
        limitCause('exited code=Some(137) signal=None limit=memory'),
        limitCause('exited code=Some(3) signal=None'),
        limitCause(null),
      ],
    };
  });
  expect(read).toEqual({
    pids: 'The agent stopped: the sandbox hit its process limit while it ran (too many processes and threads in this project’s tasks at once). Its branch and changes are safe.',
    memory: 'The agent stopped: the sandbox ran out of memory while it ran. Its branch and changes are safe.',
    plain: 'The agent stopped unexpectedly. Its branch and changes are safe.',
    cancelled: 'You stopped this task. Its branch and changes are safe.',
    causes: ['pids', 'memory', null, null],
  });
});

// SPEC-268 review round 1: the rendered views. #3 "Image pipeline" crashed while its sandbox was
// at its process limit (`limit=pids` in its run's exit reason): after a load, the task's banner
// and its overview both say so.
test('a task that crashed at the sandbox’s process limit says so in its banner and overview', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/t/3'));
  await expect(screen.getByRole('heading', { level: 1 })).toHaveText('#3 Image pipeline');
  await expect(browser.locator('.taskview .term-banner')).toContainText('the sandbox hit its process limit while it ran');
  await app.open(mock('/s/studio/tasks/3'));
  await expect(browser.locator('main')).toContainText('Ended · the sandbox hit its process limit');
});
