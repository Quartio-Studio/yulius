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

test('a full list read cannot reverse a newer resume or removal', async ({ app, browser }) => {
  await app.open(mock('/s/studio'));
  const result = await browser.evaluate(async () => {
    const url = '/assets/store.js';
    const { createStore } = await import(url);
    const sid = 'late-list';
    const row = { id: 'p', sandbox_id: sid, status: 'parked', parked_needs_you: false };
    const removed = { ...row, id: 'removed' };
    const archived = { ...row, id: 'archived' };
    let release!: () => void;
    const held = new Promise<void>(resolve => { release = resolve; });
    let hold = false;
    const store = createStore({ get: async () => { if (hold) await held; return [row, removed, archived]; } });
    await store.loadTasks(sid);
    hold = true;
    const read = store.loadTasks(sid);
    const events: string[] = [];
    const off = store.on('task', (t: { id: string; status: string }) => { if (t.id === row.id) events.push(t.status); });
    store.upsertTask({ ...row, status: 'running' });
    store.removeTask(removed.id);
    store.upsertTask({ ...archived, archived: true });
    release();
    await read;
    off();
    store.stopEvents();
    return [events, store.findTask(row.id).status, Boolean(store.findTask(removed.id)), Boolean(store.findTask(archived.id))];
  });
  expect(result).toEqual([['running'], 'running', false, false]);
});

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
