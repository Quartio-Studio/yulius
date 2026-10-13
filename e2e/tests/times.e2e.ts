import { test } from '@e2e-dev/web';
import { expect } from 'e2e';
import { axeViolations, failOnPageErrors, mock } from './support.ts';

failOnPageErrors();

// SPEC-087 (2026-10-13): "time shown next to the tasks + data in dashboards doesn't seem real
// often". The sidebar showed a bare "1m" — the time since the task's last change of anything —
// next to tasks that had worked for hours. Every time now says what it measures.

test('each group says what its time measures, in words (unit, in the page)', async ({ app, browser }) => {
  await app.open(mock('/s/studio'));
  const read = await browser.evaluate(async () => {
    const timeUrl = '/assets/tasktime.js';
    const statusUrl = '/assets/status.js';
    const { taskTime, span, ago } = await import(timeUrl);
    const { taskInfo } = await import(statusUrl);
    const now = Date.parse('2026-10-13T15:30:00Z');
    const before = (ms: number) => new Date(now - ms).toISOString();
    const MIN = 60_000;
    const HOUR = 60 * MIN;
    const base = { created_at: before(9 * HOUR), updated_at: before(MIN), steps: { done: 0, total: 0 } };
    const time = (task: Record<string, unknown>, pr: unknown = null) => taskTime({ ...base, ...task }, taskInfo({ ...base, ...task }, null, pr), now).text;
    return {
      // Woken by its own background work a minute ago, but working since 13:16.
      working: time({ status: 'running', activity: 'working', state_since: before(MIN), working_since: before(2 * HOUR + 14 * MIN) }),
      // Idle with a background shell: still Working, the same stretch.
      shell: time({ status: 'running', activity: 'waiting', state_since: before(5 * MIN), working_since: before(3 * HOUR), background_shells: 1, background_shells_since: before(5 * MIN) }),
      waiting: time({ status: 'running', activity: 'waiting', state_since: before(12 * MIN) }),
      asks: time({ status: 'running', activity: 'needs_attention', state_since: before(90 * 1000) }),
      done: time({ status: 'running', activity: 'done', state_since: before(3 * HOUR + 40 * MIN) }),
      queued: time({ status: 'queued', state_since: before(7 * HOUR) }),
      chained: time({ status: 'chained', state_since: before(2 * HOUR), after: { number: 4 } }),
      paused: time({ status: 'waiting_capacity', state_since: before(5 * MIN) }),
      review: time({ status: 'running', activity: 'waiting', state_since: before(3 * MIN) }, { number: 9, state: 'open', checks: 'pending' }),
      stopped: time({ status: 'stopped_cancelled', state_since: before(50 * HOUR) }),
      closed: time({ status: 'closed', state_since: before(5 * HOUR) }),
      failed: time({ status: 'failed_spawn_error', state_since: before(20 * MIN) }),
      // An older server without the state's time: its last change.
      older: time({ status: 'running', activity: 'waiting' }),
      spans: [0, 59_999, MIN, 59 * MIN, HOUR, 80 * MIN, 24 * HOUR, 27 * HOUR, 15 * 24 * HOUR].map(span),
      agos: [10_000, 5 * MIN, 3 * HOUR + 59 * MIN, 50 * HOUR].map(ago),
    };
  });
  expect(read).toEqual({
    working: 'working 2h 14m',
    shell: 'working 3h',
    waiting: 'waiting 12m',
    asks: 'waiting 1m',
    done: 'done 3h ago',
    queued: 'queued 7h',
    chained: 'queued 2h',
    paused: 'paused 5m',
    review: 'in review 3m',
    stopped: 'stopped 2d ago',
    closed: 'closed 5h ago',
    failed: 'failed 20m ago',
    older: 'waiting 1m',
    spans: ['<1m', '<1m', '1m', '59m', '1h', '1h 20m', '1d', '1d 3h', '2w'],
    agos: ['just now', '5m ago', '3h ago', '2d ago'],
  });
});

test('the sidebar and the Tasks page say how long a task has worked, not when it last changed', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/t/13'));
  // #12 last changed 20 s ago (a turn its background work woke) and has worked for over two hours.
  const age = (group: string, title: string) => browser.locator(`.task-group.group-${group} .task-row:has-text("${title}") .task-age`);
  await expect(age('working', 'Checkout API')).toHaveText(/^working 2h 1[45]m$/);
  await expect(age('working', 'Checkout API')).toHaveAttribute('title', /^Working without needing you since .+ \(2 h 1[45] min\) · created /);
  await expect(age('needs', 'AWS login for deploy')).toHaveText(/^waiting \d/);
  await expect(age('done', 'Improve the code')).toHaveText(/^done \d+m ago$/);
  await expect(age('ended', 'Payment webhook')).toHaveText('stopped 9h ago');

  await app.open(mock('/s/studio/tasks'));
  await expect(browser.locator('.tl-columns')).toHaveText(/Created\s*For/i);
  const row = '.tl-row[data-task-id="tsk_studio_12"]';
  await expect(browser.locator(`${row} .tl-time`)).toHaveText(/^working 2h 1[45]m$/);
  await expect(browser.locator(`${row} .tl-started`)).toHaveAttribute('title', /^Created /);
  expect(await axeViolations(browser)).toEqual([]);
});

test('a task overview and the dashboard name what each figure measures', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/tasks/12'));
  const facts = browser.locator('.ti-facts');
  await expect(facts).toContainText('Created');
  await expect(facts).toContainText(/Now\s*working 2h 1[45]m/);
  await expect(facts).toContainText(/Agent work\s*41 min\s*17 turns · sessions open/);

  await app.open(mock('/s/studio/dashboard'));
  const kpis = screen.getByRole('region', 'Key figures');
  await expect(kpis).toContainText(/Agent work per task\s*26 min/);
  await expect(kpis).toContainText(/Agent working time/);
  await expect(kpis).toContainText(/sessions open .+, idle included/);
  await expect(kpis).not.toContainText('Median duration');
  // A running task whose goal is achieved is finished: Done, not Running, in the outcomes.
  await expect(browser.locator('.db-legend')).toContainText(/Done\s*2/);
  await expect(browser.locator('.db-card.db-main')).toContainText('UTC days');
  expect(await axeViolations(browser)).toEqual([]);
});
