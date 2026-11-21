import { test } from '@e2e-dev/web';
import { expect } from 'e2e';
import { axeViolations, failOnPageErrors, mock, terminalShown, touchTask } from './support.ts';

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
      unfinishedDone: info({ ...asked, activity: 'done' }),
      yourTurn: info(asked),
      noPullOverview: info({ ...parked, parked_needs_you: true }),
      plainParkedStatus: statusInfo({ ...parked, parked_needs_you: true }).group,
      staleGreenOverview: info({ ...parked, parked_needs_you: true }, open('success')),
      staleRedOverview: info({ ...merged, parked_needs_you: false, parked_pr: { number: 7, open: true } }, open('failure')),
      discoveredRed: info({ ...merged, parked_needs_you: false }, open('failure')),
      request: info({ ...withRequest, parked_needs_you: false }, open('success')),
      stale: info(stale),
      merged: info(merged, { number: 7, state: 'merged', checks: 'success' }),
      checksRunning: info(merged, open('pending')),
      conflict: info(merged, open('success', true)),
      ciError: info(merged, open('error')),
      achievedRunning: info({ ...merged, activity: 'done' }, open('pending')),
      neverWorking: groups.every((group) => group === 'needs' || group === 'done'),
      // Its idle run's facts, as the server sends them for a parked task (review round 3, W1).
      specLeft: info({ ...merged, spec_unfinished: true }, { number: 7, state: 'open', checks: 'success' }),
      goalLeft: info({ ...merged, goal_unfinished: true }, open('success')),
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
    unfinishedDone: ['Your turn · parked', 'needs'],
    yourTurn: ['Your turn · parked', 'needs'],
    noPullOverview: ['Your turn · parked', 'needs'],
    plainParkedStatus: 'needs',
    staleGreenOverview: ['Your turn · parked', 'needs'],
    staleRedOverview: ['PR #7 · checks failing · parked', 'needs'],
    discoveredRed: ['PR #7 · checks failing · parked', 'needs'],
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
  await expect(browser.locator('.term')).toHaveAttribute('data-cursor-blink', 'false');

  // Waiting attention bar uses calm `.waiting` info styling (never warning), offers Resume + Reply.
  const bar = browser.locator('.attention-bar');
  await expect(bar).toBeVisible();
  await expect(bar).toHaveAttribute('class', /\bwaiting\b/);
  await expect(bar).toHaveText(/This idle session was parked \(your turn\)/);
  await expect(bar.getByRole('button', { name: 'Resume' })).toBeVisible();
  await expect(bar.getByRole('button', { name: 'Reply' })).toBeVisible();

  // Compare against a crash: crash shows a warning banner with Continue, not the parked info banner,
  // and typing into a crashed terminal MUST NOT wake it.
  await touchTask(browser, 'sb_studio', 13, { status: 'stopped_crashed', activity: null });
  await expect(browser.locator('.term-banner-warn')).toBeVisible();
  await expect(browser.locator('.term-banner-warn')).toHaveText(/The agent stopped/);
  await expect(browser.locator('.term-banner-warn').getByRole('button', { name: 'Continue' })).toBeVisible();
  await browser.locator('.xterm-helper-textarea').focus();
  await browser.keyboard.type('do not wake crash');
  await expect(browser.locator('.term-banner-warn')).toBeVisible();

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
  await expect(browser.locator('.term')).toHaveAttribute('data-cursor-blink', 'true');

  // The next idle park is announced asynchronously. Exercise a park that arrives
  // after the ordinary five-second button assertion budget (#1501).
  await browser.evaluate(() => {
    type Mock = { touchTask(sandbox: string, number: number, changes: Record<string, string>): void };
    const db = (window as unknown as { __yuliusMock: Mock }).__yuliusMock;
    setTimeout(() => db.touchTask('sb_studio', 13, { status: 'parked', activity: 'waiting' }), 6_500);
  });
  await expect(bar).toHaveText(/This idle session was parked \(your turn\)/, { timeout: 15_000 });
  await expect(bar.getByRole('button', { name: 'Reply' })).toBeVisible();
  await bar.getByRole('button', { name: 'Reply' }).click();
  await bar.getByRole('textbox', { name: 'Reply to the agent' }).fill('Check CI status');
  await bar.getByRole('button', { name: 'Send' }).click();
  await expect(browser.locator('.strip-status')).not.toHaveText(/Starting|· parked/);

  await expect.poll(() => browser.evaluate(() => {
    type Session = { lines: unknown[] };
    const db = (window as unknown as { __yuliusMock: { sessions: Map<string, Session> } }).__yuliusMock;
    return db.sessions.get('tsk_studio_13')?.lines.some(line => typeof line === 'string' && line.replace(/\x1b\[[\d;]*m/g, '') === '> Check CI status');
  })).toBe(true);

  // Park again and type across BOTH `parked` and `starting` before `running`: all typed characters
  // stay buffered with zero terminal error notices and are delivered once the session runs.
  await touchTask(browser, 'sb_studio', 13, { status: 'parked', activity: 'waiting' });
  await expect(infoBanner).toBeVisible();
  await browser.locator('.xterm-helper-textarea').focus();
  await browser.keyboard.type('hel');
  await expect(browser.locator('.strip-status')).toHaveText(/Starting/);
  await browser.keyboard.type('lo');
  await browser.keyboard.press('Enter');
  await expect(browser.locator('.strip-status')).not.toHaveText(/Starting|· parked/);
  await expect(browser.locator('.term-notice')).not.toBeVisible();
  await expect(browser.locator('.xterm-rows')).toHaveText(/hello/);

  // Non-owner ownership boundary: another user's parked task never exposes Resume or Reply.
  await touchTask(browser, 'sb_studio', 13, { status: 'parked', activity: 'waiting', owner: { login: 'anna-q' } });
  await expect(bar.getByRole('button', { name: 'Resume' })).toHaveCount(0);
  await expect(bar.getByRole('button', { name: 'Reply' })).toHaveCount(0);

  // Initial-load parked view: navigating directly to an already-parked task renders the calm
  // parked banner, visible Resume action, and disabled cursor blink on first paint.
  await touchTask(browser, 'sb_studio', 13, { status: 'parked', activity: 'waiting', owner: { login: 'mkarp' } });
  await browser.evaluate(() => { window.location.hash = '#/s/studio'; });
  await expect(screen.getByRole('link', /Improve the code/)).toBeVisible();
  await screen.getByRole('link', /Improve the code/).click();
  await expect(browser.locator('.term-banner-info')).toBeVisible();
  await expect(browser.locator('.term-banner-info').getByRole('button', { name: 'Resume' })).toBeVisible();
  await expect(browser.locator('.term')).toHaveAttribute('data-cursor-blink', 'false');
});

type TerminalViewer = { push(message: string): void };
type ParkedSession = {
  cols: number;
  rows: number;
  task: { status: string };
  parkedScreen: { cols: number; rows: number; text: string } | null;
  viewers: Set<TerminalViewer>;
  receive(viewer: TerminalViewer, data: string): void;
  sendStates(): void;
  sendSnapshot(viewer: TerminalViewer): void;
  state(): string;
  snapshot(): string;
};
type TerminalMock = { __yuliusMock: { sessions: Map<string, ParkedSession> } };

for (const status of ['parked', 'queued', 'starting', 'waiting_capacity']) {
  test(`the owner sees every row of a parked screen while the task is ${status}`, async ({ app, browser }) => {
    await browser.setViewport({ width: 390, height: 844 });
    await app.open(mock('/s/studio/t/12', '&still=1'));
    await terminalShown(browser);
    await browser.evaluate((status) => {
      const session = (window as unknown as TerminalMock).__yuliusMock.sessions.get('tsk_studio_12')!;
      session.task.status = status;
      session.parkedScreen = {
        cols: 220, rows: 90,
        text: '\x1b[2J\x1b[HLast answer begins\x1b[89;1HLast answer ends\x1b[90;1HYulius parked this session. Write to resume.',
      };
      session.sendStates();
      for (const viewer of session.viewers) session.sendSnapshot(viewer);
    }, status);
    const rows = browser.locator('.taskview .xterm-rows > div');
    await expect(rows).toHaveCount(90);
    await expect(rows.first()).toHaveText(/Last answer begins/);
    // A phone's resize can arrive after the first rows of the saved screen are visible.
    const afterResize = await browser.evaluate(() => {
      const session = (window as unknown as TerminalMock).__yuliusMock.sessions.get('tsk_studio_12')!;
      for (const viewer of session.viewers) session.receive(viewer, JSON.stringify({ type: 'resize', cols: 44, rows: 28 }));
      return { state: JSON.parse(session.state()), snapshot: session.snapshot() };
    });
    await expect(rows.nth(88)).toHaveText(/Last answer ends/);
    expect(afterResize.snapshot).toContain('Last answer ends');
    expect(afterResize.state).toMatchObject({ parked_screen: true, attempt_seq: null, rows: 90, cols: 220 });
    await expect(rows.last()).toHaveText(/Write to resume/);
    // Opening a parked page receives state and snapshot. Staying connected also receives exit.
    if (status === 'parked') {
      await browser.evaluate(() => {
        const session = (window as unknown as TerminalMock).__yuliusMock.sessions.get('tsk_studio_12')!;
        for (const viewer of session.viewers) viewer.push(JSON.stringify({ type: 'exit', status: 'parked', code: 0 }));
      });
      await expect(rows).toHaveCount(90);
      await expect(rows.last()).toHaveText(/Write to resume/);
    }
    const fits = await browser.evaluate(() => {
      const host = document.querySelector('.taskview .term-host')!.getBoundingClientRect();
      const grid = document.querySelector('.taskview .xterm-rows')!.getBoundingClientRect();
      const notice = document.querySelector('.taskview .xterm-rows > div:last-child')!.getBoundingClientRect();
      return grid.left >= host.left - 1 && grid.right <= host.right + 1 && grid.top >= host.top - 1 && notice.bottom <= host.bottom + 1;
    });
    expect(fits).toBe(true);
    if (status === 'waiting_capacity') {
      // Offer the same desktop grid on resume. A phone that still mirrors it would show 90 rows.
      await browser.evaluate(() => {
        const session = (window as unknown as TerminalMock).__yuliusMock.sessions.get('tsk_studio_12')!;
        session.parkedScreen = null;
        session.cols = 220;
        session.rows = 90;
        session.sendStates();
      });
      await expect(browser.locator('.taskview .term-host.term-mirror')).toHaveCount(0);
      await expect(rows).not.toHaveCount(90);
    }
  });
}

test('a failed overlapping cold read does not discard the successful task list', async ({ app, browser }) => {
  await app.open(mock('/s/studio'));
  const results = await browser.evaluate(async () => {
    const url = '/assets/store.js';
    const { createStore } = await import(url);
    const sid = 'cold-list';
    const seen = [];
    for (const lateSuccess of [false, true]) {
      let release!: () => void, fail!: (e: Error) => void;
      const good = new Promise<void>(resolve => { release = resolve; });
      const bad = new Promise<void>((_, reject) => { fail = reject; });
      let calls = 0;
      const store = createStore({ get: async () => {
        if (++calls === 1) await good; else await bad;
        return [{ id: 'p', sandbox_id: sid, status: 'parked', parked_needs_you: false }];
      } });
      const first = store.loadTasks(sid);
      const second = store.loadTasks(sid);
      const finished = Promise.allSettled([first, second]);
      if (lateSuccess) { fail(new Error('unavailable')); await second.catch(() => {}); release(); }
      else { release(); await first; fail(new Error('unavailable')); }
      seen.push((await finished).map(r => r.status), store.findTask('p')?.status ?? null, Boolean(store.tasksError(sid)));
      store.stopEvents();
    }
    return seen;
  });
  expect(results).toEqual([['fulfilled', 'rejected'], 'parked', false, ['fulfilled', 'rejected'], 'parked', false]);
});

test('the latest successful full list wins when the older parked snapshot arrives last', async ({ app, browser }) => {
  await app.open(mock('/s/studio'));
  const result = await browser.evaluate(async () => {
    const url = '/assets/store.js';
    const { createStore } = await import(url);
    let release!: () => void;
    const held = new Promise<void>(resolve => { release = resolve; });
    let calls = 0;
    const store = createStore({ get: async () => {
      const first = ++calls === 1;
      if (first) await held;
      return [{ id: 'p', sandbox_id: 'cold', status: first ? 'parked' : 'running' }];
    } });
    const older = store.loadTasks('cold');
    await store.loadTasks('cold');
    release();
    const rows = await older;
    store.stopEvents();
    return [rows.get('p').status, store.findTask('p').status];
  });
  expect(result).toEqual(['running', 'running']);
});

test('a superseded list failure keeps the newer result while an isolated refresh reports its error', async ({ app, browser }) => {
  await app.open(mock('/s/studio'));
  const seen = await browser.evaluate(async () => {
    const url = '/assets/store.js';
    const { createStore } = await import(url);
    let reject!: (error: Error) => void;
    const held = new Promise<void>((_, no) => { reject = no; });
    let calls = 0;
    const store = createStore({ get: async () => {
      if (++calls === 1) await held;
      if (calls > 2) throw new Error('refresh unavailable');
      return [{ id: 'p', sandbox_id: 's', status: 'parked' }];
    } });
    const older = store.loadTasks('s');
    await store.loadTasks('s');
    reject(new Error('older read failed'));
    const rows = await older;
    const superseded = [rows.get('p')?.status, Boolean(store.tasksError('s'))];
    let failed = false;
    try { await store.loadTasks('s'); } catch { failed = true; }
    const isolated = [failed, store.tasksError('s')?.message, store.findTask('p').status];
    store.stopEvents();
    return [superseded, isolated];
  });
  expect(seen).toEqual([['parked', false], [true, 'refresh unavailable', 'parked']]);
});

test('a cold task page recovers when its other list request succeeds', async ({ app, browser, screen }) => {
  await browser.addInitScript(() => {
    const w = window as unknown as { __yuliusTaskListReply: (url: string) => Promise<Response | undefined>; releaseList: () => void };
    let reads = 0;
    w.__yuliusTaskListReply = async (url) => {
      if (!String(url).includes('/sb_studio/tasks?archived=false')) return;
      const request = ++reads;
      if (request === 1) await new Promise<void>(resolve => { w.releaseList = resolve; });
      if (request === 2) return new Response(JSON.stringify({ error: { code: 'unavailable', message: 'fixture list unavailable' } }), { status: 503 });
    };
  });
  await app.open(mock('/s/studio/t/13', '&ambient=0'));
  await expect(screen.getByText(/Couldn’t load the tasks of studio/i).first()).toBeVisible();
  await browser.evaluate(() => (window as unknown as { releaseList: () => void }).releaseList());
  await expect(browser.locator('.task-row').first()).toBeVisible();
  await expect(browser.locator('.taskview .strip-status')).toBeVisible();
  await expect(screen.getByText(/Couldn’t load the tasks of studio/i)).toHaveCount(0);
});

test('an aborted task-list fixture cannot return its held reply', async ({ app, browser }) => {
  await app.open(mock('/settings'));
  const result = await browser.evaluate(async () => {
    const url = '/assets/mock.js';
    const { createMockTransport } = await import(url);
    const mock = createMockTransport(new URLSearchParams('ambient=0'));
    let release!: (response: Response) => void;
    const held = new Promise<Response>(resolve => { release = resolve; });
    const w = window as unknown as { __yuliusTaskListReply?: () => Promise<Response> };
    w.__yuliusTaskListReply = () => held;
    const controller = new AbortController();
    const read = mock.fetch('/api/sandboxes/sb_studio/tasks?archived=false', { signal: controller.signal })
      .then((r: Response) => String(r.status), (e: Error) => e.name);
    controller.abort();
    release(new Response(null, { status: 200 }));
    delete w.__yuliusTaskListReply;
    return await read;
  });
  expect(result).toBe('AbortError');
});

test('retrying a cold archived task opens it with one archive lookup', async ({ app, browser }) => {
  type Fixture = { allowActiveList: boolean; archiveReads: number; __yuliusTaskListReply: (url: string) => Response | undefined };
  await browser.addInitScript(() => {
    const w = window as unknown as Fixture;
    w.archiveReads = 0;
    w.__yuliusTaskListReply = (url) => {
      if (!String(url).includes('/sb_studio/tasks?archived=')) return;
      const archived = String(url).includes('archived=true');
      if (archived ? ++w.archiveReads === 1 : w.allowActiveList) return;
      return new Response(JSON.stringify({ error: { code: 'unavailable', message: 'fixture list unavailable' } }), { status: 503 });
    };
  });
  await app.open(mock('/s/studio/t/1', '&ambient=0'));
  const retry = browser.locator('main .load-error button');
  await expect(retry).toBeVisible();
  await browser.evaluate(() => { (window as unknown as Fixture).allowActiveList = true; });
  await retry.click();
  await expect(browser.locator('.taskview .strip-status')).toBeVisible();
  expect(await browser.evaluate(() => (window as unknown as Fixture).archiveReads)).toBe(1);
});

for (const scenario of [
  { title: 'two successful active lists recover the same archived task only once', number: 1, failArchive: false },
  { title: 'an active task in the retry list wins over the held archive list', number: 13, failArchive: false },
  { title: 'an active task in the retry list wins over a failed archive lookup', number: 13, failArchive: true },
  { title: 'an active task in a later retry list reopens a cold missing task page', number: 13, failArchive: false, archiveFirst: true },
]) {
  test(scenario.title, async ({ app, browser }) => {
    type Fixture = {
      __yuliusTaskListReply: (url: string) => Promise<Response | undefined>;
      __yuliusMock: { tasks: { sandbox_id: string; number: number; title: string }[] };
      failArchive: boolean; releaseOriginal: () => void; releaseRetry?: () => void; releaseArchive?: () => void; archiveReads: number;
    };
    await browser.addInitScript(() => {
      const w = window as unknown as Fixture;
      let activeReads = 0;
      w.archiveReads = 0;
      w.__yuliusTaskListReply = async (url) => {
        if (!String(url).includes('/sb_studio/tasks?archived=')) return;
        if (String(url).includes('archived=true')) {
          if (++w.archiveReads === 1) {
            await new Promise<void>(resolve => { w.releaseArchive = resolve; });
            if (!w.failArchive) return;
          }
        } else {
          const read = ++activeReads;
          if (read === 1) {
            await new Promise<void>(resolve => { w.releaseOriginal = resolve; });
            return new Response('[]', { headers: { 'Content-Type': 'application/json' } });
          }
          if (read === 3) {
            await new Promise<void>(resolve => { w.releaseRetry = resolve; });
            w.__yuliusMock.tasks.find(t => t.sandbox_id === 'sb_studio' && t.number === 13)!.title = 'Retry list arrived';
            return;
          }
        }
        return new Response(JSON.stringify({ error: { code: 'unavailable', message: 'fixture list unavailable' } }), { status: 503 });
      };
    });
    await app.open(mock(`/s/studio/t/${scenario.number}`, '&ambient=0'));
    await browser.evaluate((failArchive) => { (window as unknown as Fixture).failArchive = failArchive; }, scenario.failArchive);
    await browser.locator('main .load-error button').click();
    await expect.poll(() => browser.evaluate(() => Boolean((window as unknown as Fixture).releaseRetry))).toBe(true);
    await browser.evaluate(() => (window as unknown as Fixture).releaseOriginal());
    await expect.poll(() => browser.evaluate(() => Boolean((window as unknown as Fixture).releaseArchive))).toBe(true);
    if (scenario.archiveFirst) {
      await browser.evaluate(() => (window as unknown as Fixture).releaseArchive!());
      await expect(browser.locator('main').getByText('Task not found')).toBeVisible();
    }
    await browser.evaluate(() => (window as unknown as Fixture).releaseRetry!());
    await expect(browser.locator('.sidebar .task-row:has-text("Retry list arrived")')).toBeVisible();
    if (!scenario.archiveFirst) await browser.evaluate(() => (window as unknown as Fixture).releaseArchive!());
    await expect(browser.locator('.taskview .strip-status')).toBeVisible();
    expect(await browser.evaluate(() => (window as unknown as Fixture).archiveReads)).toBe(1);
  });
}

test('the sidebar keeps verified unfinished parked work under Needs you with a healthy Git chip', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio', '&ambient=0&pullgap=0'));
  type Fixture = { __yuliusMock: { tasks: { id: string; stepsList: { status: string }[] }[];
    touchTask(s: string, n: number, fields: object): void; changePull(s: string, n: number, fields: object): void } };
  await browser.evaluate(() => {
    const db = (window as unknown as Fixture).__yuliusMock;
    const task = db.tasks.find(t => t.id === 'tsk_studio_13')!;
    db.changePull('sb_studio', 520, { checks: 'success' });
    db.touchTask('sb_studio', 13, {
      status: 'parked', activity: 'done', stepsList: task.stepsList.map(step => ({ ...step, status: 'done' })),
      parked_needs_you: false, parked_pr: { number: 520, open: true },
    });
  });
  const task = /PR #520\s*, pull request, open, checks passed.*Improve the code/;
  await expect(screen.getByRole('region', 'Done').getByRole('link', task)).toBeVisible();
  await touchTask(browser, 'sb_studio', 13, { parked_needs_you: true });
  await expect(screen.getByRole('region', 'Needs you').getByRole('link', task)).toBeVisible();
  await touchTask(browser, 'sb_studio', 13, { parked_needs_you: false });
  await expect(screen.getByRole('region', 'Done').getByRole('link', task)).toBeVisible();
  for (const flag of ['spec_unfinished', 'goal_unfinished']) {
    await touchTask(browser, 'sb_studio', 13, { [flag]: true });
    await expect(screen.getByRole('region', 'Needs you').getByRole('link', task)).toBeVisible();
    await touchTask(browser, 'sb_studio', 13, { [flag]: false });
    await expect(screen.getByRole('region', 'Done').getByRole('link', task)).toBeVisible();
  }
});

test('the PR watch refreshes the parked group in both directions', async ({ app, browser }) => {
  await app.open(mock('/s/studio'));
  const groups = await browser.evaluate(async () => {
    const storeModule = '/assets/store.js';
    const statusModule = '/assets/status.js';
    const { createStore } = await import(storeModule);
    const { taskInfo } = await import(statusModule);
    const sid = 'fixture';
    const id = 't-parked';
    const task = { id, sandbox_id: sid, activity: 'waiting', status: 'parked', steps: { total: 2, done: 2 } };
    let fails = false;
    let unavailable = false;
    let overviewUnavailable = false;
    let newerEvent = false;
    let tick = () => {};
    const store = createStore({
      get: async (path: string) => {
        if (path.endsWith('/git')) {
          if (overviewUnavailable) throw new Error('overview temporarily unavailable');
          return { tasks: [{ task_id: id, pr: { number: 7, state: 'open', checks: fails ? 'failure' : 'success' } }] };
        }
        if (path.includes('/tasks?')) {
          if (unavailable) throw new Error('summary temporarily unavailable');
          if (newerEvent) store.upsertTask({ ...task, parked_needs_you: true });
          return [{ ...task, parked_pr: { number: 7, open: true }, parked_needs_you: fails }, { ...task, id: 'answered', parked_needs_you: false }];
        }
        return [];
      },
    }, { clock: { now: () => 50_000, setInterval: (fn: () => void) => { tick = fn; return 1; }, clearInterval() {}, clearTimeout() {} } });
    // The two sources notify independently. Sample after both synchronous fake
    // replies have drained, including the release of each in-flight request.
    const settled = () => new Promise<void>(resolve => {
      const channel = new MessageChannel();
      channel.port1.onmessage = () => { channel.port1.close(); channel.port2.close(); resolve(); };
      channel.port2.postMessage(null);
    });
    const group = (taskId = id) => taskInfo(store.state.tasks.get(sid).get(taskId), null, store.pullsOf(sid).get(taskId)).group;
    await store.loadTasks(sid);
    const stop = store.watchPulls(sid);
    await settled();
    const seen = [[group(), group('answered')]];
    for (const [red, failure, event] of [
      [true, false, false], [false, false, false],
      [true, true, false], [false, false, false], [false, false, true],
    ]) {
      fails = red;
      unavailable = failure;
      newerEvent = event;
      tick();
      await settled();
      seen.push([group(), group('answered')]);
    }
    // The overview may fail independently of the task summaries. Wait for the
    // mock fetch promises to settle before sampling the next browser turn.
    newerEvent = false;
    overviewUnavailable = true;
    for (const red of [false, true]) {
      fails = red;
      tick();
      await settled();
      seen.push([group(), group('answered')]);
    }
    stop();
    store.stopEvents();
    return seen;
  });
  expect(groups).toEqual([
    ['done', 'done'], ['needs', 'done'], ['done', 'done'],
    ['needs', 'done'], ['done', 'done'], ['needs', 'done'],
    ['done', 'done'], ['needs', 'done'],
  ]);
});

test('parked status refreshes while the Git overview is held or incomplete', async ({ app, browser }) => {
  await app.open(mock('/s/studio'));
  const groups = await browser.evaluate(async () => {
    const module = '/assets/store.js';
    const { createStore } = await import(module);
    const sid = 'cold-overview';
    const task = { sandbox_id: sid, status: 'parked', activity: 'done' };
    let needs = false;
    let unavailable = false;
    let release!: () => void;
    const held = new Promise<void>((resolve) => { release = resolve; });
    let tick = () => {};
    const store = createStore({
      get: async (path: string) => {
        if (path.endsWith('/git')) { await held; return { tasks: [] }; }
        if (!path.includes('/tasks?')) return [];
        if (unavailable) throw new Error('task summary unavailable');
        return [{ ...task, id: 'pr', parked_pr: { number: 42, open: true }, parked_needs_you: needs },
          { ...task, id: 'answer', parked_needs_you: false }];
      },
    }, { pullGapMs: 0, clock: { now: () => 50_000, setInterval: (fn: () => void) => { tick = fn; return 1; }, clearInterval() {}, clearTimeout() {} } });
    const settled = () => new Promise<void>((resolve) => {
      const channel = new MessageChannel();
      channel.port1.onmessage = () => { channel.port1.close(); channel.port2.close(); resolve(); };
      channel.port2.postMessage(null);
    });
    const state = () => ['pr', 'answer'].map(id => store.state.tasks.get(sid).get(id).parked_needs_you);
    await store.loadTasks(sid);
    needs = true;
    const stop = store.watchPulls(sid);
    await settled();
    const seen = [state()];
    // A later poll and a fresh event each recheck status before the same
    // held overview finishes. The snapshot can change more than once.
    needs = false;
    tick();
    await settled();
    seen.push(state());
    needs = true;
    store.handleEvent({ type: 'pulls.updated', sandbox_id: sid });
    await settled();
    seen.push(state());
    release();
    await settled();
    needs = false;
    for (const fails of [false, true, false]) {
      unavailable = fails;
      tick();
      await settled();
      seen.push(state());
    }
    stop();
    store.stopEvents();
    return seen;
  });
  expect(groups).toEqual([[true, false], [false, false], [true, false], [false, false], [true, false], [false, false]]);
});

test('the sidebar follows parked PR reruns while the Git overview is held', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio', '&ambient=0&pullgap=0'));
  await touchTask(browser, 'sb_studio', 13, { status: 'parked', activity: 'done', parked_needs_you: false, parked_pr: { number: 520, open: true } });
  await expect(screen.getByRole('region', 'Done').getByRole('link', /Improve the code/)).toBeVisible();
  await browser.evaluate(() => {
    (window as unknown as { __yuliusMock: { holdGitReads: boolean } }).__yuliusMock.holdGitReads = true;
  });
  for (const fails of [true, false]) {
    await browser.evaluate((red) => {
      type Mock = { tasks: { id: string; parked_needs_you?: boolean }[]; changePull(sandbox: string, number: number, changes: object): void };
      const db = (window as unknown as { __yuliusMock: Mock }).__yuliusMock;
      // The summary changes on the server. The only event is the PR watch.
      db.tasks.find((t) => t.id === 'tsk_studio_13')!.parked_needs_you = red;
      db.changePull('sb_studio', 520, { checks: red ? 'failure' : 'success' });
    }, fails);
    await expect(screen.getByRole('region', fails ? 'Needs you' : 'Done').getByRole('link', /Improve the code/)).toBeVisible();
  }
});

test('a held parked summary leaves Git rows readable and refreshes a newly parked task', async ({ app, browser }) => {
  await app.open(mock('/s/studio'));
  const result = await browser.evaluate(async () => {
    const module = '/assets/store.js';
    const { createStore } = await import(module);
    const sid = 'held-summary';
    const task = { id: 'pr', sandbox_id: sid, status: 'parked', activity: 'waiting' };
    let release!: () => void;
    const held = new Promise<void>(resolve => { release = resolve; });
    const turn = () => new Promise<void>(resolve => {
      const channel = new MessageChannel();
      channel.port1.onmessage = () => { channel.port1.close(); channel.port2.close(); resolve(); };
      channel.port2.postMessage(null);
    });
    let summaryReads = 0;
    let gitUpdates = 0;
    const store = createStore({ get: async (path: string) => {
      if (path.endsWith('/git')) return { tasks: [] };
      if (!path.includes('/tasks?')) return [];
      summaryReads++;
      if (summaryReads === 1) return [{ ...task, status: 'running' }];
      await held;
      return [{ ...task, steps: { total: 7, done: 5 }, parked_pr: { number: 73, open: true }, parked_needs_you: true }];
    } }, { pullGapMs: 0 });
    await store.loadTasks(sid);
    store.upsertTask(task); // A lifecycle event can arrive without the list's verified facts.
    const off = store.on('pulls', () => { gitUpdates++; });
    const stop = store.watchPulls(sid);
    await turn();
    store.handleEvent({ type: 'pulls.updated', sandbox_id: sid });
    store.handleEvent({ type: 'pulls.updated', sandbox_id: sid });
    await turn();
    const whileHeld = { summaryReads, gitVisible: gitUpdates > 0 };
    release();
    await turn();
    off();
    stop();
    store.stopEvents();
    const refreshed = store.state.tasks.get(sid).get(task.id);
    return { whileHeld, summaryReads, needs: refreshed.parked_needs_you ?? null, steps: refreshed.steps ?? null };
  });
  expect(result).toEqual({ whileHeld: { summaryReads: 2, gitVisible: true }, summaryReads: 3, needs: true, steps: { total: 7, done: 5 } });
});

test('failed parked verification accounts for open replacement PRs in either response order', async ({ app, browser }) => {
  await app.open(mock('/s/studio'));
  const groups = await browser.evaluate(async () => {
    const storeUrl = '/assets/store.js', statusUrl = '/assets/status.js';
    const { createStore } = await import(storeUrl);
    const { taskInfo } = await import(statusUrl);
    const seen = [];
    const settled = () => new Promise<void>(resolve => {
      const channel = new MessageChannel();
      channel.port1.onmessage = () => { channel.port1.close(); channel.port2.close(); resolve(); };
      channel.port2.postMessage(null);
    });
    for (const gitFirst of [true, false]) {
      const sid = 'replacement';
      const row = { id: 'pr', sandbox_id: sid, status: 'parked', activity: 'done', parked_needs_you: false, parked_pr: { number: 7, open: false } };
      let release!: () => void;
      const held = new Promise<void>(resolve => { release = resolve; });
      let unavailable = false;
      let tick = () => {};
      const store = createStore({ get: async (path: string) => {
        if (path.endsWith('/git')) {
          if (!gitFirst) await held;
          return { tasks: [{ task_id: row.id, pr: { number: 8, state: 'open', checks: 'success' } }] };
        }
        if (!path.includes('/tasks?')) return [];
        if (unavailable) { if (gitFirst) await held; throw new Error('summary unavailable'); }
        return [row];
      } }, { pullGapMs: 0, clock: { now: () => 50_000, setInterval: (fn: () => void) => { tick = fn; return 1; }, clearInterval() {}, clearTimeout() {} } });
      await store.loadTasks(sid);
      unavailable = true;
      const stop = store.watchPulls(sid);
      await settled();
      const rename = () => store.upsertTask({ id: row.id, sandbox_id: sid, status: 'parked', activity: 'done', title: 'Renamed' });
      if (!gitFirst) rename(); // The list failed before Git discovered the replacement.
      release();
      await settled();
      const group = () => taskInfo(store.findTask(row.id), null, store.pullsOf(sid).get(row.id)).group;
      seen.push(group());
      if (gitFirst) rename();
      seen.push(group(), store.findTask(row.id).parked_pr?.number ?? null);
      await settled();
      unavailable = false;
      tick();
      await settled();
      seen.push(group());
      stop();
      store.stopEvents();
    }
    return seen;
  });
  expect(groups).toEqual(['needs', 'needs', 8, 'done', 'needs', 'needs', 8, 'done']);
});

test('parked reads keep their three-second gap while Git is held', async ({ app, browser }) => {
  await app.open(mock('/s/studio'));
  const starts = await browser.evaluate(async () => {
    const url = '/assets/store.js';
    const { createStore } = await import(url);
    const sid = 'busy';
    let now = 50_000;
    let release!: () => void;
    const held = new Promise<void>(resolve => { release = resolve; });
    const starts: number[] = [];
    const timers = new Map<number, { at: number; fn: () => void }>();
    let timer = 0;
    const store = createStore({ get: async (path: string) => {
      if (path.endsWith('/git')) { await held; return { tasks: [] }; }
      if (!path.includes('/tasks?')) return [];
      starts.push(now);
      return [{ id: 'pr', sandbox_id: sid, status: 'parked', parked_needs_you: true }];
    } }, { clock: { now: () => now, setInterval: () => 1, clearInterval() {},
      setTimeout(fn: () => void, ms: number) { timers.set(++timer, { fn, at: now + ms }); return timer; },
      clearTimeout(id: number) { timers.delete(id); },
    } });
    const settled = () => new Promise<void>(resolve => {
      const channel = new MessageChannel();
      channel.port1.onmessage = () => { channel.port1.close(); channel.port2.close(); resolve(); };
      channel.port2.postMessage(null);
    });
    await store.loadTasks(sid);
    starts.length = 0;
    const stop = store.watchPulls(sid);
    await settled();
    for (const at of [53_000, 53_100, 53_200, 53_300]) {
      now = at;
      store.handleEvent({ type: 'pulls.updated', sandbox_id: sid });
      await settled();
    }
    now = 56_000;
    for (const [id, job] of [...timers]) if (job.at <= now) { timers.delete(id); job.fn(); }
    await settled();
    stop();
    release();
    store.stopEvents();
    return starts;
  });
  expect(starts).toEqual([50_000, 53_000, 56_000]);
});

test('a replacement list tells parked task views about changed rows once', async ({ app, browser }) => {
  await app.open(mock('/s/studio'));
  const notifications = await browser.evaluate(async () => {
    const url = '/assets/store.js';
    const { createStore } = await import(url);
    let needs = false;
    const row = { id: 'p', sandbox_id: 's', status: 'parked' };
    const store = createStore({ get: async () => [{ ...row, parked_needs_you: needs }] });
    await store.loadTasks('s');
    const seen: boolean[] = [];
    const off = store.on('task', (t: { parked_needs_you: boolean }) => seen.push(t.parked_needs_you));
    needs = true;
    await store.loadTasks('s');
    await store.loadTasks('s');
    off();
    store.stopEvents();
    return seen;
  });
  expect(notifications).toEqual([true]);
});

test('verified recovery supersedes an interim fallback from a held Git read', async ({ app, browser }) => {
  await app.open(mock('/s/studio'));
  const result = await browser.evaluate(async () => {
    const url = '/assets/store.js';
    const { createStore } = await import(url);
    const sid = 'race';
    const row = { id: 'p', sandbox_id: sid, status: 'parked', activity: 'done', parked_needs_you: false };
    let gitReady!: () => void, summaryReady!: () => void;
    const git = new Promise<void>(resolve => { gitReady = resolve; });
    const summary = new Promise<void>(resolve => { summaryReady = resolve; });
    const settled = () => new Promise<void>(resolve => {
      const channel = new MessageChannel();
      channel.port1.onmessage = () => { channel.port1.close(); channel.port2.close(); resolve(); };
      channel.port2.postMessage(null);
    });
    let reads = 0;
    const store = createStore({ get: async (path: string) => {
      if (path.endsWith('/git')) {
        await git;
        return { tasks: [{ task_id: row.id, pr: { number: 8, state: 'open', checks: 'success' } }] };
      }
      if (!path.includes('/tasks?')) return [];
      reads++;
      if (reads === 1) return [{ ...row, parked_pr: { number: 7, open: false } }];
      if (reads === 2) throw new Error('verification unavailable');
      await summary;
      return [{ ...row, parked_pr: { number: 8, open: true } }];
    } }, { pullGapMs: 0 });
    await store.loadTasks(sid);
    const stop = store.watchPulls(sid);
    await settled();
    store.handleEvent({ type: 'pulls.updated', sandbox_id: sid });
    await settled();
    gitReady();
    await settled();
    const fallback = store.findTask(row.id).parked_needs_you;
    summaryReady();
    await settled();
    const verified = store.findTask(row.id);
    stop();
    store.stopEvents();
    return [fallback, verified.parked_needs_you, verified.parked_pr.number];
  });
  expect(result).toEqual([true, false, 8]);
});

test('a parked lifecycle event preserves a known open PR until its status is verified again', async ({ app, browser }) => {
  await app.open(mock('/s/studio'));
  const seen = await browser.evaluate(async () => {
    const url = '/assets/store.js';
    const { createStore } = await import(url);
    const sid = 'unverified-event';
    const task = { id: 'p', sandbox_id: sid, status: 'parked', activity: 'done' };
    const verified = { ...task, parked_needs_you: true, parked_pr: { number: 8, open: true } };
    const store = createStore({ get: async (path: string) => path.includes('/tasks?') ? [verified] : { tasks: [] } });
    await store.loadTasks(sid);
    store.upsertTask({ ...task, title: 'Renamed' });
    const interim = store.findTask(task.id);
    verified.parked_needs_you = false;
    await store.loadTasks(sid);
    const current = store.findTask(task.id);
    store.stopEvents();
    return [interim.parked_needs_you ?? null, interim.parked_pr?.number ?? null, current.parked_needs_you];
  });
  expect(seen).toEqual([true, 8, false]);
});


test('entering Parked from Running verifies an open PR even with a green Git chip', async ({ app, browser }) => {
  await app.open(mock('/s/studio'));
  const result = await browser.evaluate(async () => {
    const module = '/assets/store.js';
    const { createStore } = await import(module);
    const task = { id: 'p', sandbox_id: 's', status: 'running', activity: 'done' };
    let row = task;
    let reads = 0;
    let release!: () => void;
    const held = new Promise<void>(resolve => { release = resolve; });
    const store = createStore({ get: async (path: string) => {
      if (path.endsWith('/git')) return { tasks: [{ task_id: task.id, pr: { number: 8, state: 'open', checks: 'success' } }] };
      if (!path.includes('/tasks?')) return [];
      if (++reads > 1) await held;
      return [{ ...row, parked_needs_you: false, parked_pr: { number: 8, open: true } }];
    } }, { pullGapMs: 0 });
    await store.loadTasks('s');
    const git = new Promise<void>(resolve => { const off = store.on('pulls', () => { off(); resolve(); }); });
    const stop = store.watchPulls('s');
    await git;
    row = { ...task, status: 'parked' };
    store.upsertTask(row);
    const interim = store.findTask(task.id).parked_needs_you ?? null;
    release();
    await new Promise<void>(resolve => {
      const channel = new MessageChannel();
      channel.port1.onmessage = () => { channel.port1.close(); channel.port2.close(); resolve(); };
      channel.port2.postMessage(null);
    });
    const confirmed = store.findTask(task.id).parked_needs_you ?? null;
    stop();
    store.stopEvents();
    return [interim, confirmed, reads];
  });
  expect(result).toEqual([true, false, 2]);
});

test('a held list reconciles parked resumes, same-second Undo, and irreversible removal', async ({ app, browser }) => {
  await app.open(mock('/s/studio'));
  const observed = await browser.evaluate(async () => {
    const storeUrl = '/assets/store.js', actionsUrl = '/assets/actions.js';
    const { createStore } = await import(storeUrl);
    const { archiveTask, archiveAll } = await import(actionsUrl);
    const results = [];
    for (const action of ['resume', 'archive', 'undo', 'single-restored', 'bulk-restored', 'delete']) {
      const original = { id: 'p', number: 1, sandbox_id: 's', status: 'parked', activity: 'done', archived: false, updated_at: '2026-11-20T04:52:00Z' };
      let latest = original;
      let reads = 0;
      let release!: () => void;
      const held = new Promise<void>(resolve => { release = resolve; });
      let deleted = false;
      const api = { get: async () => {
        const snapshot = latest;
        if (++reads === 2) await held;
        return snapshot.archived || deleted && reads > 2 ? [] : [snapshot];
      }, post: async () => original };
      const store = createStore(api);
      const ctx = { api, store, route: () => null };
      await store.loadTasks('s');
      const read = store.loadTasks('s');
      if (action === 'resume') {
        latest = { ...original, status: 'running' };
        store.upsertTask(latest);
      } else if (action === 'archive' || action === 'undo') {
        latest = { ...original, archived: true };
        store.upsertTask(latest);
        if (action === 'undo') latest = original; // A restore commits before the browser receives its SSE.
      } else if (action === 'single-restored') {
        await archiveTask(ctx, original); // POST rereads an already restored task.
      } else if (action === 'bulk-restored') {
        await archiveAll(ctx, [original], 'task');
      } else {
        deleted = true;
        store.removeTask(original.id);
        store.upsertTask(original); // A delayed response from before permanent deletion.
      }
      const interim = store.findTask(original.id)?.status ?? null;
      release();
      await read;
      results.push([interim, store.findTask(original.id)?.status ?? null, reads <= 3]);
      store.stopEvents();
    }
    return results;
  });
  expect(observed).toEqual([
    ['running', 'running', true], [null, null, true], [null, 'parked', true],
    ['parked', 'parked', true], ['parked', 'parked', true], [null, null, true],
  ]);
});

test('a cold parked snapshot cannot undo a resume and a parked watch preserves an unrelated live update', async ({ app, browser }) => {
  await app.open(mock('/s/studio'));
  const seen = await browser.evaluate(async () => {
    const url = '/assets/store.js';
    const { createStore } = await import(url);
    const parked = { sandbox_id: 's', id: 'p', status: 'parked' };
    const live = { sandbox_id: 's', id: 'r', status: 'running', title: 'Before' };
    let rows = [parked, live];
    let release!: () => void;
    let held = new Promise<void>(resolve => { release = resolve; });
    const store = createStore({ get: async (path: string) => {
      if (path.endsWith('/git')) return { tasks: [] };
      const snapshot = rows;
      await held;
      return snapshot;
    } });
    const cold = store.loadTasks('s');
    rows = [{ ...parked, status: 'running' }, live];
    store.upsertTask(rows[0]);
    release();
    await cold;
    const resumed = store.findTask('p').status;
    rows = [parked, live];
    await store.loadTasks('s');
    held = new Promise<void>(resolve => { release = resolve; });
    const stop = store.watchPulls('s');
    store.upsertTask({ ...live, title: 'After' });
    const completed = new Promise<void>(resolve => { const off = store.on('tasks', () => { off(); resolve(); }); });
    release();
    await completed;
    stop();
    store.stopEvents();
    return [resumed, store.findTask('r').title];
  });
  expect(seen).toEqual(['running', 'After']);
});

test('a newer failed watch keeps an open PR unverified after an older list succeeds', async ({ app, browser }) => {
  await app.open(mock('/s/studio'));
  for (const failureFirst of [false, true]) {
    const result = await browser.evaluate(async (failureFirst) => {
      const url = '/assets/store.js';
      const { createStore } = await import(url);
      const parked = { sandbox_id: 's', id: 'p', status: 'parked', parked_needs_you: false, parked_pr: { number: 42, open: true } };
      let release!: () => void, fail!: (error: Error) => void;
      const older = new Promise<void>(resolve => { release = resolve; });
      const newer = new Promise<void>((_, reject) => { fail = reject; });
      let reads = 0;
      const store = createStore({ get: async (path: string) => {
        if (path.endsWith('/git')) return { tasks: [] };
        if (++reads === 2) await older;
        else if (reads === 3) await newer;
        return [{ ...parked }];
      } });
      await store.loadTasks('s');
      const explicit = store.loadTasks('s');
      const stop = store.watchPulls('s');
      if (!failureFirst) { release(); await explicit; }
      fail(new Error('verification unavailable'));
      await new Promise<void>(resolve => {
        const channel = new MessageChannel();
        channel.port1.onmessage = () => { channel.port1.close(); channel.port2.close(); resolve(); };
        channel.port2.postMessage(null);
      });
      if (failureFirst) { release(); await explicit; }
      stop();
      return store.findTask('p').parked_needs_you;
    }, failureFirst);
    expect(result).toBe(true);
  }
});

test('a task-list confirmation keeps a live sibling update received during its second read', async ({ app, browser }) => {
  await app.open(mock('/s/studio'));
  const result = await browser.evaluate(async () => {
    const url = '/assets/store.js';
    const { createStore } = await import(url);
    let parked = { sandbox_id: 's', id: 'p', status: 'parked' };
    const live = { sandbox_id: 's', id: 'r', status: 'running', title: 'Before' };
    let first!: () => void, second!: () => void, started!: () => void;
    const initial = new Promise<void>(resolve => { first = resolve; });
    const confirm = new Promise<void>(resolve => { second = resolve; });
    const confirming = new Promise<void>(resolve => { started = resolve; });
    let calls = 0;
    const store = createStore({ get: async () => {
      const rows = [parked, live];
      if (++calls === 2) await initial;
      else if (calls === 3) { started(); await confirm; }
      return rows;
    } });
    await store.loadTasks('s');
    const read = store.loadTasks('s');
    parked = { ...parked, status: 'running' };
    store.upsertTask(parked);
    first();
    await confirming;
    store.upsertTask({ ...live, title: 'After' });
    second();
    await read;
    return [store.findTask('p').status, store.findTask('r').title, calls];
  });
  expect(result).toEqual(['running', 'After', 3]);
});

test('a successful archive confirmation outranks a concurrent list that began before Undo', async ({ app, browser }) => {
  await app.open(mock('/s/studio'));
  const results = await browser.evaluate(async () => {
    const url = '/assets/store.js';
    const { createStore } = await import(url);
    const seen = [];
    for (const order of ['list-first', 'confirm-first', 'confirm-fails']) {
      const original = { sandbox_id: 's', id: 'p', status: 'parked', archived: false };
      const restored = { ...original, status: 'closed' };
      let row = original;
      const release: (() => void)[] = [];
      let started!: () => void;
      const confirmation = new Promise<void>(resolve => { started = resolve; });
      let calls = 0;
      const store = createStore({ get: async () => {
        const snapshot = row.archived ? [] : [row];
        const n = ++calls;
        if (n > 1) {
          if (n === 4) started();
          await new Promise<void>(resolve => { release[n] = resolve; });
        }
        if (n === 4 && order === 'confirm-fails') throw new Error('confirmation failed');
        return snapshot;
      } });
      await store.loadTasks('s');
      const first = store.loadTasks('s');
      row = { ...restored, archived: true };
      store.upsertTask(row);
      const concurrent = store.loadTasks('s');
      row = restored;
      release[2]();
      await confirmation;
      if (order === 'confirm-first') { release[4](); await first; release[3](); }
      else { release[3](); await concurrent; release[4](); }
      await Promise.all([first, concurrent]);
      seen.push(store.findTask('p')?.status ?? null, calls);
    }
    return seen;
  });
  expect(results).toEqual(['closed', 4, 'closed', 4, null, 4]);
});

test('a cold parked rename retains an open PR from the first list when confirmation fails', async ({ app, browser }) => {
  await app.open(mock('/s/studio'));
  const seen = await browser.evaluate(async () => {
    const url = '/assets/store.js';
    const { createStore } = await import(url);
    const task = { sandbox_id: 's', id: 'p', status: 'parked', activity: 'done' };
    let release!: () => void;
    const held = new Promise<void>(resolve => { release = resolve; });
    let calls = 0;
    const store = createStore({ get: async () => {
      if (++calls > 1) throw new Error('confirmation unavailable');
      await held;
      return [{ ...task, parked_needs_you: true, parked_pr: { number: 42, open: true } }];
    } });
    const read = store.loadTasks('s');
    store.upsertTask({ ...task, title: 'Renamed' });
    release();
    await read;
    const row = store.findTask('p');
    return [row.title, row.parked_needs_you ?? null, row.parked_pr?.number ?? null];
  });
  expect(seen).toEqual(['Renamed', true, 42]);
});

test('a cold list remembers a PR change until it learns whether parked rows need verification', async ({ app, browser }) => {
  await app.open(mock('/s/studio'));
  const result = await browser.evaluate(async () => {
    const url = '/assets/store.js';
    const { createStore } = await import(url);
    const seen = [];
    const turn = () => new Promise<void>(resolve => {
      const channel = new MessageChannel();
      channel.port1.onmessage = () => { channel.port1.close(); channel.port2.close(); resolve(); };
      channel.port2.postMessage(null);
    });
    for (const status of ['parked', 'running']) {
      let release!: () => void, verify!: () => void;
      const first = new Promise<void>(resolve => { release = resolve; });
      const second = new Promise<void>(resolve => { verify = resolve; });
      let calls = 0, red = false;
      const store = createStore({ get: async (path: string) => {
        if (path.endsWith('/git')) return { tasks: [{ task_id: 'p', pr: { number: 42, state: 'open', checks: red ? 'failure' : 'success' } }] };
        const row = { id: 'p', sandbox_id: 's', status, parked_needs_you: red, parked_pr: { number: 42, open: true } };
        await (++calls === 1 ? first : second);
        return [row];
      } }, { pullGapMs: 0 });
      const stop = store.watchPulls('s');
      const list = store.loadTasks('s');
      await turn();
      red = true;
      store.handleEvent({ type: 'pulls.updated', sandbox_id: 's' });
      await turn();
      release();
      await list;
      const waiting = store.findTask('p').parked_needs_you;
      verify();
      await turn();
      seen.push([waiting, store.findTask('p').parked_needs_you, calls]);
      stop();
      store.stopEvents();
    }
    return seen;
  });
  expect(result).toEqual([[true, true, 2], [false, false, 1]]);
});

test('replaying a parked rename preserves a replacement PR discovered while the watch was held', async ({ app, browser }) => {
  await app.open(mock('/s/studio'));
  const result = await browser.evaluate(async () => {
    const url = '/assets/store.js';
    const { createStore } = await import(url);
    const base = { id: 'p', sandbox_id: 's', status: 'parked', activity: 'done' };
    let gitReady!: () => void, firstReady!: () => void, nextReady!: () => void;
    const git = new Promise<void>(resolve => { gitReady = resolve; });
    const first = new Promise<void>(resolve => { firstReady = resolve; });
    const next = new Promise<void>(resolve => { nextReady = resolve; });
    const turn = () => new Promise<void>(resolve => {
      const channel = new MessageChannel();
      channel.port1.onmessage = () => { channel.port1.close(); channel.port2.close(); resolve(); };
      channel.port2.postMessage(null);
    });
    let calls = 0;
    const store = createStore({ get: async (path: string) => {
      if (path.endsWith('/git')) {
        await git;
        return { tasks: [{ task_id: 'p', pr: { number: 8, state: 'open', checks: 'success' } }] };
      }
      if (++calls === 2) await first; else if (calls === 3) await next;
      return [{ ...base, parked_pr: { number: 7, open: false }, parked_needs_you: false }];
    } }, { pullGapMs: 0 });
    await store.loadTasks('s');
    const stop = store.watchPulls('s');
    store.upsertTask({ ...base, title: 'Renamed' });
    gitReady();
    await turn();
    const row = () => { const t = store.findTask('p'); return [t.title, t.parked_needs_you ?? null, t.parked_pr?.number ?? null]; };
    const before = row();
    firstReady();
    await turn();
    const during = row();
    nextReady();
    await turn();
    stop();
    return [before, during, calls];
  });
  expect(result).toEqual([['Renamed', true, 8], ['Renamed', true, 8], 3]);
});

test('an Undo received during archive confirmation survives a competing empty list in both orders', async ({ app, browser }) => {
  await app.open(mock('/s/studio'));
  const results = await browser.evaluate(async () => {
    const url = '/assets/store.js';
    const { createStore } = await import(url);
    const seen = [];
    for (const finish of [[3, 4], [4, 3]]) {
      let row = { sandbox_id: 's', id: 'p', status: 'parked', archived: false };
      const release: (() => void)[] = [];
      let started!: () => void;
      const confirm = new Promise<void>(resolve => { started = resolve; });
      let calls = 0;
      const store = createStore({ get: async () => {
        const snapshot = row.archived ? [] : [row];
        const n = ++calls;
        if (n === 3) started();
        if (n > 1 && n <= 4) await new Promise<void>(resolve => { release[n] = resolve; });
        return snapshot;
      } });
      await store.loadTasks('s');
      const first = store.loadTasks('s');
      row = { ...row, status: 'closed', archived: true };
      store.upsertTask(row);
      release[2]();
      await confirm;
      const competing = store.loadTasks('s');
      row = { ...row, archived: false };
      store.upsertTask(row);
      for (const n of finish) {
        release[n]();
        await (n === 3 ? first : competing);
      }
      seen.push(store.findTask('p')?.status ?? null);
    }
    return seen;
  });
  expect(results).toEqual(['closed', 'closed']);
});

test('a verified parked recovery displaces an earlier failing chip only for the same PR', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio', '&ambient=0&pullgap=0'));
  type Fixture = { __yuliusMock: {
    holdGitReads: boolean; tasks: { id: string; parked_needs_you?: boolean; parked_pr?: { number: number; open: boolean }; title: string }[];
    changePull(s: string, n: number, changes: object): void; touchTask(s: string, n: number, changes: object): void;
  } };
  await browser.evaluate(() => {
    const db = (window as unknown as Fixture).__yuliusMock;
    db.touchTask('sb_studio', 13, { status: 'parked', activity: 'done', parked_needs_you: true, parked_pr: { number: 520, open: true } });
    db.changePull('sb_studio', 520, { checks: 'failure' });
  });
  const chip = /PR #520\s*, pull request, open, checks fail.*Improve the code/;
  await expect(screen.getByRole('region', 'Needs you').getByRole('link', chip)).toBeVisible();
  for (const number of [521, 520]) {
    await browser.evaluate((number) => {
      const db = (window as unknown as Fixture).__yuliusMock;
      db.holdGitReads = true;
      const task = db.tasks.find(t => t.id === 'tsk_studio_13')!;
      task.parked_needs_you = false;
      task.parked_pr = { number, open: true };
      db.tasks.find(t => t.id === 'tsk_studio_12')!.title = 'Verified alongside PR ' + number;
      db.changePull('sb_studio', 520, { checks: 'success' });
    }, number);
    await expect(browser.locator('.task-row').getByText('Verified alongside PR ' + number)).toBeVisible();
    await expect(screen.getByRole('region', number === 520 ? 'Done' : 'Needs you').getByRole('link', chip)).toBeVisible();
  }
});

test('an open replacement discovered after a failed watch updates an already waiting parked row', async ({ app, browser }) => {
  await app.open(mock('/s/studio'));
  const result = await browser.evaluate(async () => {
    const url = '/assets/store.js';
    const { createStore } = await import(url);
    let reads = 0, release!: () => void;
    const held = new Promise<void>(resolve => { release = resolve; });
    const row = { id: 'p', sandbox_id: 's', status: 'parked', parked_needs_you: false, parked_pr: { number: 7, open: true } };
    const store = createStore({ get: async (path: string) => {
      if (path.endsWith('/git')) {
        await held;
        return { tasks: [{ task_id: 'p', pr: { number: 8, state: 'open', checks: 'success' } }] };
      }
      if (++reads === 1) return [row];
      throw new Error('verification unavailable');
    } });
    await store.loadTasks('s');
    const fallback = new Promise<void>(resolve => { const off = store.on('task', () => { off(); resolve(); }); });
    const git = new Promise<void>(resolve => { const off = store.on('pulls', () => { off(); resolve(); }); });
    const stop = store.watchPulls('s');
    await fallback;
    const first = store.findTask('p').parked_pr.number;
    release();
    await git;
    const latest = store.findTask('p');
    stop();
    return [first, latest.parked_needs_you, latest.parked_pr.number, reads];
  });
  expect(result).toEqual([7, true, 8, 2]);
});

test('a parked rename preserves a verified Needs you without a PR until a healthy list arrives', async ({ app, browser }) => {
  await app.open(mock('/s/studio'));
  const seen = await browser.evaluate(async () => {
    const url = '/assets/store.js', statusUrl = '/assets/status.js';
    const { createStore } = await import(url);
    const { taskInfo } = await import(statusUrl);
    const results = [];
    for (const cold of [true, false]) {
      let release!: () => void;
      const held = new Promise<void>(resolve => { release = resolve; });
      let calls = 0;
      const raw = { sandbox_id: 's', id: 'p', status: 'parked', activity: 'done', title: 'Renamed' };
      const store = createStore({ get: async (path: string) => {
        if (path.endsWith('/git')) return { tasks: [] };
        if (++calls === 1) {
          if (cold) await held;
          return [{ ...raw, title: 'Before', parked_needs_you: true }];
        }
        if (calls === 2) throw new Error('verifier unavailable');
        return [{ ...raw, parked_needs_you: false }];
      } });
      const first = store.loadTasks('s');
      if (!cold) await first;
      store.upsertTask(raw);
      release();
      await first;
      let stop;
      if (!cold) {
        stop = store.watchPulls('s');
        await new Promise<void>(resolve => {
          const channel = new MessageChannel();
          channel.port1.onmessage = () => { channel.port1.close(); channel.port2.close(); resolve(); };
          channel.port2.postMessage(null);
        });
      }
      const pending = store.findTask('p');
      await store.loadTasks('s');
      results.push([pending.title, taskInfo(pending).group, taskInfo(store.findTask('p')).group]);
      stop?.();
      store.stopEvents();
    }
    return results;
  });
  expect(seen).toEqual([['Renamed', 'needs', 'done'], ['Renamed', 'needs', 'done']]);
});
