import { test } from '@e2e-dev/web';
import { expect } from 'e2e';
import { axeViolations, failOnPageErrors, mock, touchTask } from './support.ts';

failOnPageErrors();

// SPEC-512 part 1 (#1412): a task whose turn ended and that waits for a person is parked after a
// grace period: its session ends, the task stays where the list showed it and resumes when someone
// writes to it.

test('a parked task reads as it did, marked parked, and resumes from its menu (unit, in the page)', async ({ app, browser }) => {
  await app.open(mock('/s/studio'));
  const read = await browser.evaluate(async () => {
    const statusUrl = '/assets/status.js';
    const { taskInfo, activityLine, taskActions, isLive, endedMessage, statusInfo } = await import(statusUrl);
    const parked = { status: 'parked', activity: 'done', steps: { done: 3, total: 3 }, owner: { login: 'me' } };
    const asked = { status: 'parked', activity: 'waiting', steps: { done: 1, total: 3 }, owner: { login: 'me' } };
    const withRequest = { ...asked, needs_you: { action: 'Choose the `colour`', problem: 'Two options' } };
    // Shells and a `yulius wait` cannot run without a session: a stale count does not make it Working.
    const stale = { ...parked, background_shells: 2, yulius_wait: true };
    const merged = { status: 'parked', activity: 'waiting', steps: { done: 3, total: 3 } };
    const info = (task: Record<string, unknown>, pr: unknown = null) => {
      const shown = taskInfo(task, null, pr);
      return [shown.label, shown.group];
    };
    // Nothing of a parked task goes on by itself: what keeps a running agent busy is a person's
    // (review round 1, M2), and never Working.
    const open = (checks: string, conflicts = false) => ({ number: 7, state: 'open', checks, conflicts });
    const prs = [open('pending'), open('error'), open('success', true), open('failure'), open('success'), { number: 7, state: 'merged', checks: 'success' }];
    const groups = [merged, { ...merged, activity: 'done' }, asked].flatMap((task) => prs.map((pr) => taskInfo(task, null, pr).group));
    const acts = taskActions(parked, 'write', null, 'me');
    return {
      done: info(parked),
      yourTurn: info(asked),
      request: info(withRequest),
      stale: info(stale),
      merged: info(merged, { number: 7, state: 'merged', checks: 'success' }),
      checksRunning: info(merged, open('pending')),
      conflict: info(merged, open('success', true)),
      ciError: info(merged, open('error')),
      achievedRunning: info({ ...merged, activity: 'done' }, open('pending')),
      neverWorking: groups.every((group) => group === 'needs' || group === 'done'),
      // Its idle run's facts, as the server sends them for a parked task (review round 3, W1).
      specLeft: info({ ...merged, spec_unfinished: true }, { number: 7, state: 'open', checks: 'success' }),
      goalLeft: info({ ...merged, goal_unfinished: true }),
      line: activityLine(parked),
      live: isLive('parked'),
      actions: { continue: acts.continue, stop: acts.stop, archive: acts.archive, delete: acts.delete, move: acts.move },
      someoneElses: taskActions(parked, 'write', null, 'other').continue,
      message: endedMessage('parked'),
      // A status with no activity (the Dashboard's Outcomes bucket) reads Parked, not Running.
      bucket: [statusInfo({ status: 'parked', activity: null }).label, statusInfo({ status: 'parked', activity: null }).group],
    };
  });
  expect(read).toEqual({
    done: ['Done · parked', 'done'],
    yourTurn: ['Your turn · parked', 'needs'],
    request: ['Needs you: Choose the colour · parked', 'needs'],
    stale: ['Done · parked', 'done'],
    merged: ['Done · PR #7 merged · parked', 'done'],
    checksRunning: ['In review · PR #7 · checks running · parked', 'needs'],
    conflict: ['In review · PR #7 · conflicts with its base · parked', 'needs'],
    ciError: ['In review · PR #7 · CI could not run (not the code) · parked', 'needs'],
    achievedRunning: ['Done · PR #7 · parked', 'done'],
    neverWorking: true,
    specLeft: ['Your turn · parked', 'needs'],
    goalLeft: ['Your turn · parked', 'needs'],
    line: 'Parked to free its session · resumes when you write to it',
    live: false,
    // It resumes where it ran (a move would be refused), and a stop stops it.
    actions: { continue: true, stop: true, archive: true, delete: true, move: false },
    someoneElses: false,
    message: {
      tone: 'info',
      text: 'Yulius parked this idle session to free its place and memory. Resume it, or write to it, and it goes on in the same session with its history.',
      resume: true,
    },
    bucket: ['Parked', 'needs'],
  });
});

// SPEC-512 PRK-007: a queued task says what it waits for; a supertask's hold keeps its own line.
test('a queued task says what it waits for (unit, in the page)', async ({ app, browser }) => {
  await app.open(mock('/s/studio'));
  const read = await browser.evaluate(async () => {
    const statusUrl = '/assets/status.js';
    const { taskInfo, activityLine } = await import(statusUrl);
    const full = { status: 'queued', queue_reason: 'Waiting for a free session: 8 of 8 in use' };
    const held = { status: 'queued', held_by: [3], queue_reason: null };
    const plain = { status: 'queued' };
    return {
      full: [activityLine(full), taskInfo(full, null, null).detail],
      held: activityLine(held),
      plain: activityLine(plain),
    };
  });
  expect(read).toEqual({
    full: ['Waiting for a free session: 8 of 8 in use', 'Waiting for a free session: 8 of 8 in use'],
    held: 'It starts once #3 is done: a supertask runs alone in this project.',
    plain: 'Queued',
  });
});

// SPEC-512 PRK-008: the Capacity card shows working sessions of the limit, and parked, idle and
// queued ones apart, with when an idle session parks.
test('the Capacity card shows working and parked sessions apart (unit, in the page)', async ({ app, browser }) => {
  await app.open(mock('/s/studio'));
  const read = await browser.evaluate(async () => {
    const layoutUrl = '/assets/layout.js';
    const { capacityPanel } = await import(layoutUrl);
    const tile = (sessions: Record<string, number>) => {
      const panel = capacityPanel({ subscriptions: 1, sessions });
      const found = [...panel.querySelectorAll('.stat-tile')].find((t) => t.textContent.startsWith('Sessions'));
      return found ? [found.querySelector('.tile-value').textContent, [...found.querySelectorAll('.stat-detail')].map((d) => d.textContent)] : null;
    };
    return {
      busy: tile({ working: 8, limit: 8, idle: 2, parked: 13, queued: 3, park_after_ms: 1800000 }),
      quiet: tile({ working: 1, limit: 8, idle: 0, parked: 0, queued: 0, park_after_ms: 0 }),
      none: capacityPanel({ subscriptions: 1 }).textContent.includes('Sessions'),
    };
  });
  expect(read).toEqual({
    busy: ['8 / 8 working', ['13 parked · 2 idle · 3 queued', 'Idle sessions park after 30 min']],
    quiet: ['1 / 8 working', ['None parked', 'Parking is off']],
    none: false,
  });
});

// What a person sees: a parked task's row and strip, and a queued task's reason on its page.
test('a parked task shows where it was, marked parked; a queued task page says why it waits', async ({ app, browser, screen }) => {
  // No ambient activity: the mock's own changes would regroup the list under the test.
  await app.open(mock('/s/studio/t/13', '&ambient=0'));
  await expect(screen.getByRole('heading', { level: 1 })).toHaveText('#13 Improve the code');
  await touchTask(browser, 'sb_studio', 13, { status: 'parked', activity: 'done' });
  await expect(browser.locator('.strip-status')).toHaveText(/· parked/);
  await expect(screen.getByRole('region', 'Done').getByRole('link', /Improve the code/)).toBeVisible();
  expect(await axeViolations(browser)).toEqual([]);
  await touchTask(browser, 'sb_studio', 13, { status: 'queued', activity: null, queue_reason: 'Waiting for a free session: 8 of 8 in use' });
  await expect(browser.locator('.strip-status')).toHaveText(/Queued · Waiting for a free session: 8 of 8 in use/);
});

// SPEC-553 PRK-101, PRK-102: a parked Codex/Claude task renders the calm info-tone parked banner
// with a visible Resume button (distinct from a crash or operator stop), offers Resume and Reply
// in the waiting bar for its owner, and wakes via Resume or Reply while keeping stopped tasks inert.
test('a parked task terminal shows its parked banner and Resume action, distinct from a crash, and wakes on Resume or Reply', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/t/13', '&ambient=0'));
  await expect(screen.getByRole('heading', { level: 1 })).toHaveText('#13 Improve the code');

  // Verify termsocket.js treats parked as a non-live session state so the banner renders on load.
  const ended = await browser.evaluate(async () => {
    const { isEndedStatus } = await import('/assets/termsocket.js');
    return {
      parked: isEndedStatus('parked'),
      running: isEndedStatus('running'),
      stopped: isEndedStatus('stopped_cancelled'),
      crashed: isEndedStatus('stopped_crashed'),
    };
  });
  expect(ended).toEqual({ parked: true, running: false, stopped: true, crashed: true });

  // Connect a Codex account in the mock DB so Resume/Reply work on a Codex task too.
  await browser.evaluate(() => {
    (window as unknown as { __yuliusMock: { connectCodex: () => void } }).__yuliusMock.connectCodex();
  });

  // Park the task in "Your turn · parked" (the quartio-v2 #323 / #324 state).
  await touchTask(browser, 'sb_studio', 13, { status: 'parked', activity: 'waiting', harness: 'codex' });
  const infoBanner = browser.locator('.term-banner-info');
  await expect(infoBanner).toBeVisible();
  await expect(infoBanner).toHaveText(/parked this idle session/);
  await expect(infoBanner.getByRole('button', { name: 'Resume' })).toBeVisible();
  await expect(browser.locator('.term-banner-warn')).toHaveCount(0);

  // Waiting attention bar also offers Resume and Reply to the owner, below the banner (no overlap).
  const bar = browser.locator('.attention-bar');
  await expect(bar).toBeVisible();
  await expect(bar).toHaveText(/This idle session was parked \(your turn\)/);
  await expect(bar.getByRole('button', { name: 'Resume' })).toBeVisible();
  await expect(bar.getByRole('button', { name: 'Reply' })).toBeVisible();

  // Compare against a crash: crash shows a warning banner with Continue, not the parked info banner.
  await touchTask(browser, 'sb_studio', 13, { status: 'stopped_crashed', activity: null });
  await expect(browser.locator('.term-banner-warn')).toBeVisible();
  await expect(browser.locator('.term-banner-warn')).toHaveText(/The agent stopped/);
  await expect(browser.locator('.term-banner-warn').getByRole('button', { name: 'Continue' })).toBeVisible();

  // Compare against an operator stop: shows stopped banner and typing MUST NOT wake it.
  await touchTask(browser, 'sb_studio', 13, { status: 'stopped_cancelled', activity: null });
  await expect(browser.locator('.term-banner-info')).toHaveText(/You stopped this task/);
  await browser.locator('.xterm-helper-textarea').focus();
  await browser.keyboard.type('do not wake');
  await expect(browser.locator('.term-banner-info')).toHaveText(/You stopped this task/);

  // Back to parked: clicking Resume in the banner wakes the task into running through POST /continue.
  await touchTask(browser, 'sb_studio', 13, { status: 'parked', activity: 'waiting' });
  await expect(infoBanner.getByRole('button', { name: 'Resume' })).toBeVisible();
  await infoBanner.getByRole('button', { name: 'Resume' }).click();
  await expect(browser.locator('.strip-status')).not.toHaveText(/Starting|· parked/);
  await expect(browser.locator('.term-banner-info')).toHaveCount(0);

  // Park again and wake via Reply with a prompt.
  await touchTask(browser, 'sb_studio', 13, { status: 'parked', activity: 'waiting' });
  await expect(bar.getByRole('button', { name: 'Reply' })).toBeVisible();
  await bar.getByRole('button', { name: 'Reply' }).click();
  await bar.getByRole('textbox', { name: 'Reply to the agent' }).fill('Check CI status');
  await bar.getByRole('button', { name: 'Send' }).click();
  await expect(browser.locator('.strip-status')).not.toHaveText(/Starting|· parked/);

  // Park again and wake via ordinary owner typing directly in the parked terminal.
  await touchTask(browser, 'sb_studio', 13, { status: 'parked', activity: 'waiting' });
  await expect(infoBanner).toBeVisible();
  await browser.locator('.xterm-helper-textarea').focus();
  await browser.keyboard.type('hello');
  await expect(browser.locator('.strip-status')).not.toHaveText(/Starting|· parked/);

  // Non-owner ownership boundary: another user's parked task never exposes Resume or Reply.
  await touchTask(browser, 'sb_studio', 13, { status: 'parked', activity: 'waiting', owner: { login: 'anna-q' } });
  await expect(bar.getByRole('button', { name: 'Resume' })).toHaveCount(0);
  await expect(bar.getByRole('button', { name: 'Reply' })).toHaveCount(0);
});
