import { test, type Browser } from '@e2e-dev/web';
import { expect } from 'e2e';
import { axeViolations, failOnPageErrors, mock } from './support.ts';

failOnPageErrors();

type Mocked = Window & { __release?: () => void; __yuliusMock: {
  manualOnboarding?: boolean;
  holdApprove?: Promise<void>;
  failPostPath?: string | null;
  failApprove?: boolean;
  startOnboarding(slug: string): void;
  finishOnboarding(slug: string): void;
  touchTask(sandboxId: string, number: number, changes: Record<string, unknown>): void;
  request(method: string, path: string, body?: unknown): Promise<number>;
  sandboxes: { id: string; slug: string }[];
} };

/**
 * Imports Quartio-Studio/`name` from the home screen. The mock leaves it `pending`, as `yulius
 * onboard` does; `mobile-app` already follows the format, so its onboarding is the environment
 * kind. The mock's onboarding then waits for the test to finish it (`finishOnboarding`).
 */
async function importRepository({ app, browser, screen }: { app: { open(url: string): Promise<unknown> }; browser: Browser; screen: any }, name: string) {
  await app.open(mock('/'));
  await browser.evaluate(() => { (window as unknown as Mocked).__yuliusMock.manualOnboarding = true; });
  await screen.getByRole('button', 'Import repository').click();
  const dialog = screen.getByRole('dialog', 'Add sandbox');
  await screen.getByRole('radio', new RegExp(`Quartio-Studio/${name}`)).check();
  await dialog.getByRole('button', 'Import').click();
  await expect(dialog).toBeHidden();
}

/** Changes the onboarding task (#1) of the `website` sandbox as the server would announce it. */
const touchOnboarding = (browser: Browser, changes: Record<string, unknown>) => browser.evaluate((c: Record<string, unknown>) => {
  const db = (window as unknown as Mocked).__yuliusMock;
  db.touchTask(db.sandboxes.find((s) => s.slug === 'website')!.id, 1, c);
}, changes);

/**
 * How far the onboarding region sits from the centre of the free page area (pixels, x and y). The
 * free area is the composer's box, below its top bar when that bar is in the flow (phones).
 */
const offCentre = (browser: Browser) => browser.evaluate(() => {
  const page = document.querySelector('.composer')!.getBoundingClientRect();
  const bar = document.querySelector('.composer-top')!;
  const top = getComputedStyle(bar).position === 'absolute' ? page.top : bar.getBoundingClientRect().bottom;
  const gate = document.querySelector('.onboarding-gate')!.getBoundingClientRect();
  return {
    x: Math.round(Math.abs((gate.left + gate.right) / 2 - (page.left + page.right) / 2)),
    y: Math.round(Math.abs((gate.top + gate.bottom) / 2 - (top + page.bottom) / 2)),
  };
});

// SPEC-096 (task #81): onboarding is mandatory. Until it has finished, the new-task page shows no
// task input, only a centred message saying what onboarding will do (its task's own plan) and its
// button; no Skip. SPEC-051: `yulius onboard` only uploads, so a `pending` sandbox waits for Start
// onboarding here.
test('a pending sandbox shows only what onboarding will do and Start onboarding, centred', async ({ app, browser, screen }) => {
  await importRepository({ app, browser, screen }, 'website');
  await browser.evaluate(() => { location.hash = '#/'; });
  await expect(screen.getByText('Needs onboarding').first()).toBeVisible();

  // SPEC-097 (task #82): nothing has started, so there is no task, not even a Closed one.
  await browser.evaluate(() => { location.hash = '#/s/website/tasks'; });
  await expect(browser.locator('.tl .empty-state')).toBeVisible();
  await expect(screen.getByText('Prepare this repository for Yulius')).toHaveCount(0);

  await browser.evaluate(() => { location.hash = '#/s/website'; });
  const gate = screen.getByRole('region', 'Onboard website first');
  await expect(gate).toBeVisible();
  await expect(gate).toContainText('one task an agent runs before any other');
  // The task does not exist until Start onboarding (SPEC-097): its title, not a link to it.
  await expect(gate).toContainText('Prepare this repository for Yulius');
  await expect(gate.getByRole('link')).toHaveCount(0);
  // Its plan, step by step, as the server wrote it for this repository.
  const plan = gate.getByRole('list');
  await expect(plan.getByRole('listitem')).toHaveCount(6);
  await expect(plan).toContainText('Install the toolchains and run the repository\'s own checks');
  await expect(plan).toContainText('Move GitHub Actions into Yulius CI and delete them');
  await expect(gate).toContainText('never application code, and any change reaches dev through a pull request');
  await expect(gate).toContainText('passes the Yulius format check');
  await expect(gate.getByRole('button', 'Start onboarding')).toBeVisible();
  // No task input and no way around onboarding.
  await expect(screen.getByRole('textbox', 'Task prompt')).toBeHidden();
  await expect(screen.getByRole('button', 'Start task')).toBeHidden();
  await expect(browser.locator('.composer-tools')).toBeHidden();
  await expect(screen.getByRole('button', 'Skip for now')).toHaveCount(0);
  await expect(browser.locator('.composer-quote')).toBeHidden();
  await expect(browser.locator('.banner-info')).toHaveCount(0);
  const desktop = await offCentre(browser);
  expect(desktop.x).toBeLessThanOrEqual(2);
  expect(desktop.y).toBeLessThanOrEqual(2);
  expect(await axeViolations(browser)).toEqual([]);

  // The new-task shortcut puts the keyboard on Start onboarding: there is no prompt to type in.
  await browser.keyboard.press('ControlOrMeta+Shift+O');
  await expect(gate.getByRole('button', 'Start onboarding')).toBeFocused();

  await browser.setViewport({ width: 390, height: 844 });
  await expect(gate).toBeVisible();
  const phone = await offCentre(browser);
  expect(phone.x).toBeLessThanOrEqual(2);
  expect(phone.y).toBeLessThanOrEqual(2);
  expect(await browser.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(390);
  expect(await axeViolations(browser)).toEqual([]);
});

// The review of SPEC-096: a repository that already follows the format gets the environment-only
// onboarding, which neither moves GitHub Actions nor adds documents; the page must not say it does.
test('a repository already in the Yulius format is told only what its onboarding will do', async ({ app, browser, screen }) => {
  await importRepository({ app, browser, screen }, 'mobile-app');
  await browser.evaluate(() => { location.hash = '#/s/mobile-app'; });
  const gate = screen.getByRole('region', 'Onboard mobile-app first');
  await expect(gate).toContainText('Set up this sandbox\'s environment');
  const plan = gate.getByRole('list');
  await expect(plan.getByRole('listitem')).toHaveCount(3);
  await expect(plan).toContainText('Record the working commands in yulius.toml [commands]');
  await expect(gate).not.toContainText('GitHub Actions');
  await expect(gate).not.toContainText('missing documents');
});

// SPEC-111 (task #96): the press loads at once, with nothing left to press, and the page then opens
// the onboarding task, where its session runs, instead of stopping on Follow onboarding.
test('Start onboarding loads at once and opens the onboarding session', async ({ app, browser, screen }) => {
  await importRepository({ app, browser, screen }, 'website');
  await browser.evaluate(() => { location.hash = '#/s/website'; });
  // The server creates the task and starts its run before it answers; the mock holds in between.
  await browser.evaluate(() => {
    const w = window as unknown as Mocked;
    w.__yuliusMock.holdApprove = new Promise<void>((resolve) => { w.__release = resolve; });
  });
  await screen.getByRole('region', 'Onboard website first').getByRole('button', 'Start onboarding').click();

  const starting = screen.getByRole('region', 'Starting onboarding website');
  await expect(starting).toBeVisible();
  await expect(starting).toHaveAttribute('aria-busy', 'true');
  await expect(starting).toContainText('starting the agent');
  await expect(starting.getByRole('button')).toHaveCount(0);
  await expect(starting.getByRole('link')).toHaveCount(0);
  // The new task's events have arrived; the page still loads instead of offering another press.
  await expect(browser.locator('.sidebar')).toContainText('Prepare this repository for Yulius');
  await touchOnboarding(browser, { status: 'running', activity: 'working' });
  await expect(screen.getByRole('button', 'Start onboarding')).toHaveCount(0);
  await expect(screen.getByRole('link', 'Follow onboarding')).toHaveCount(0);
  await expect(starting).toBeVisible();
  expect(await axeViolations(browser)).toEqual([]);

  await browser.evaluate(() => (window as unknown as Mocked).__release!());
  await expect(browser).toHaveURL(/#\/s\/website\/t\/1$/);
  await expect(screen.getByRole('link', 'Follow onboarding')).toHaveCount(0);
});

test('a Start onboarding that fails brings the gate back with its button', async ({ app, browser, screen }) => {
  await importRepository({ app, browser, screen }, 'website');
  await browser.evaluate(() => {
    const db = (window as unknown as Mocked).__yuliusMock;
    db.failPostPath = `/api/sandboxes/${db.sandboxes.find((s) => s.slug === 'website')!.id}/onboarding/approve`;
  });
  await browser.evaluate(() => { location.hash = '#/s/website'; });
  await screen.getByRole('region', 'Onboard website first').getByRole('button', 'Start onboarding').click();
  await expect(screen.getByText(/Couldn’t start onboarding: The mock server failed on purpose/)).toBeVisible();
  const gate = screen.getByRole('region', 'Onboard website first');
  await expect(gate).toHaveAttribute('aria-busy', 'false');
  await expect(gate.getByRole('button', 'Start onboarding')).toBeFocused();
  await expect(browser).toHaveURL(/#\/s\/website$/);
});

// The review of SPEC-111: the start takes seconds, so the person may be elsewhere when it fails;
// the keyboard stays where they put it.
test('a slow Start onboarding that fails leaves the keyboard where the person moved it', async ({ app, browser, screen }) => {
  await importRepository({ app, browser, screen }, 'website');
  await browser.evaluate(() => { location.hash = '#/s/website'; });
  await browser.evaluate(() => {
    const w = window as unknown as Mocked;
    w.__yuliusMock.failApprove = true;
    w.__yuliusMock.holdApprove = new Promise<void>((resolve) => { w.__release = resolve; });
  });
  await screen.getByRole('region', 'Onboard website first').getByRole('button', 'Start onboarding').click();
  await expect(screen.getByRole('region', 'Starting onboarding website')).toBeVisible();
  const elsewhere = browser.locator('.sidebar a[href]').first();
  await elsewhere.focus();
  await browser.evaluate(() => (window as unknown as Mocked).__release!());
  await expect(screen.getByText(/Couldn’t start onboarding: the onboarding task could not start/)).toBeVisible();
  await expect(screen.getByRole('region', 'Onboard website first').getByRole('button', 'Start onboarding')).toBeVisible();
  await expect(elsewhere).toBeFocused();
});

// The person's screenshot of task #96: "Onboarding studio … Follow onboarding" should not exist.
// Whoever opens a sandbox while its onboarding runs lands on its task, whatever the task's state
// (running, waiting for capacity, stopped: the task page says each), and Back does not bounce.
test('opening a sandbox while onboarding runs opens its task, never a Follow onboarding screen', async ({ app, browser, screen }) => {
  await importRepository({ app, browser, screen }, 'website');
  await browser.evaluate(() => { location.hash = '#/s/website'; });
  await screen.getByRole('region', 'Onboard website first').getByRole('button', 'Start onboarding').click();
  await expect(browser).toHaveURL(/#\/s\/website\/t\/1$/);
  for (const changes of [{ status: 'running', activity: 'working' }, { status: 'waiting_capacity', activity: null }, { status: 'closed', activity: null }]) {
    await touchOnboarding(browser, changes);
    await browser.evaluate(() => { location.hash = '#/s/website/tasks'; });
    await expect(browser).toHaveURL(/#\/s\/website\/tasks$/);
    await browser.evaluate(() => { location.hash = '#/s/website'; });
    await expect(browser).toHaveURL(/#\/s\/website\/t\/1$/);
    await expect(screen.getByRole('link', 'Follow onboarding')).toHaveCount(0);
    await expect(screen.getByRole('link', 'Open onboarding')).toHaveCount(0);
    await expect(screen.getByRole('textbox', 'Task prompt')).toHaveCount(0);
  }
  // The sandbox page was replaced in the history: Back returns to the task list.
  await browser.evaluate(() => history.back());
  await expect(browser).toHaveURL(/#\/s\/website\/tasks$/);
});

// Review round 2 of SPEC-111: someone else (or another tab) starts it while the gate is open here.
test('onboarding started elsewhere takes the open gate to its task and says so', async ({ app, browser, screen }) => {
  await importRepository({ app, browser, screen }, 'website');
  await browser.evaluate(() => { location.hash = '#/s/website'; });
  await expect(screen.getByRole('region', 'Onboard website first')).toBeVisible();
  await browser.evaluate(() => (window as unknown as Mocked).__yuliusMock.startOnboarding('website'));
  await expect(browser).toHaveURL(/#\/s\/website\/t\/1$/);
  await expect(screen.getByText('Onboarding started: opened #1.')).toBeVisible();
});

test('leaving and coming back while Start onboarding runs still shows it loading', async ({ app, browser, screen }) => {
  await importRepository({ app, browser, screen }, 'website');
  await browser.evaluate(() => { location.hash = '#/s/website'; });
  // Every toast is recorded as it shows: one lives 5 s, and a loaded runner can take longer than
  // that from the start's ending to the last check (#525).
  await browser.evaluate(() => {
    const seen: string[] = [];
    (window as unknown as { __toasts: string[] }).__toasts = seen;
    new MutationObserver(() => {
      for (const el of document.querySelectorAll<HTMLElement>('.toast .toast-msg')) {
        if (!el.dataset.recorded) { el.dataset.recorded = '1'; seen.push(el.textContent ?? ''); }
      }
    }).observe(document.body, { childList: true, subtree: true });
  });
  await browser.evaluate(() => {
    const w = window as unknown as Mocked;
    w.__yuliusMock.holdApprove = new Promise<void>((resolve) => { w.__release = resolve; });
  });
  await screen.getByRole('region', 'Onboard website first').getByRole('button', 'Start onboarding').click();
  await expect(screen.getByRole('region', 'Starting onboarding website')).toBeVisible();
  await browser.evaluate(() => { location.hash = '#/s/website/tasks'; });
  await expect(browser).toHaveURL(/#\/s\/website\/tasks$/);
  await browser.evaluate(() => { location.hash = '#/s/website'; });
  const starting = screen.getByRole('region', 'Starting onboarding website');
  await expect(starting).toBeVisible();
  await expect(starting.getByRole('button')).toHaveCount(0);
  await expect(screen.getByRole('button', 'Start onboarding')).toHaveCount(0);
  // The start's own ending waits for its read of the tasks (held), which brings a title only that
  // read has: once it shows, the start has said all it says. The mock makes task #1 as the start
  // reaches it.
  await expect.poll(() => browser.evaluate(() => {
    const db = (window as unknown as { __yuliusMock: { tasks: { sandbox_id: string; number: number }[]; sandboxes: { id: string; slug: string }[] } }).__yuliusMock;
    const id = db.sandboxes.find((s) => s.slug === 'website')!.id;
    return db.tasks.some((t) => t.sandbox_id === id && t.number === 1);
  })).toBe(true);
  await browser.evaluate(() => {
    const w = window as unknown as Mocked & { __releaseTasks?: () => void };
    const db = w.__yuliusMock as unknown as { holdPath: string; held: Promise<void>; tasks: { sandbox_id: string; number: number; title: string }[] };
    const id = w.__yuliusMock.sandboxes.find((s) => s.slug === 'website')!.id;
    db.holdPath = `/api/sandboxes/${id}/tasks`;
    db.held = new Promise<void>((resolve) => { w.__releaseTasks = resolve; });
    db.tasks.find((t) => t.sandbox_id === id && t.number === 1)!.title = 'Read again';
    w.__release!();
  });
  await expect(browser).toHaveURL(/#\/s\/website\/t\/1$/);
  await browser.evaluate(() => (window as unknown as { __releaseTasks: () => void }).__releaseTasks());
  await expect.poll(() => browser.evaluate(() => document.title)).toMatch(/Read again/);
  // One start, one toast: the page that opened the task says so, the composer left says nothing
  // (#436).
  await expect.poll(() => browser.evaluate(() => (window as unknown as { __toasts: string[] }).__toasts.filter((text) => text.startsWith('Onboarding')))).toEqual(['Onboarding started: opened #1.']);
});

test('New task on a sandbox being onboarded stays on its onboarding task', async ({ app, browser, screen }) => {
  await app.open(mock('/s/product', '&onboarding=preparing'));
  await expect(browser).toHaveURL(/#\/s\/product\/t\/1$/);
  const before = await browser.evaluate(() => history.length);
  await browser.keyboard.press('ControlOrMeta+Shift+O');
  await expect(browser).toHaveURL(/#\/s\/product\/t\/1$/);
  // No detour through the sandbox's page: no new history entry, the task view is not rebuilt.
  await expect(screen.getByRole('region', 'Opening onboarding product')).toHaveCount(0);
  expect(await browser.evaluate(() => history.length)).toBe(before);
});

test('a sandbox already preparing when the app opens goes to its onboarding task', async ({ app, browser, screen }) => {
  // Until its tasks are read the page loads, busy, with nothing to press (OBS-005).
  await app.open(mock('/s/product', '&onboarding=preparing&hold=/api/sandboxes/sb_product/tasks'));
  const opening = screen.getByRole('region', 'Opening onboarding product');
  await expect(opening).toBeVisible();
  await expect(opening).toHaveAttribute('aria-busy', 'true');
  await expect(opening.getByRole('button')).toHaveCount(0);
  await expect(opening).not.toContainText('missing');
  await browser.evaluate(() => (window as unknown as { __yuliusMock: { release(): void } }).__yuliusMock.release());
  await expect(browser).toHaveURL(/#\/s\/product\/t\/1$/);
  await expect(screen.getByRole('link', 'Follow onboarding')).toHaveCount(0);
});

// The review of SPEC-096: until the sandbox's task list is read (or when reading it failed), the
// page cannot know whether the onboarding task exists, so it must not call it missing. Mock
// `onboarding=preparing` puts `product` in preparation; `fail=` makes its task list fail. When
// onboarding lands while the page is open, the composer shows by itself, takes the keyboard, and a
// screen reader hears so.
test('a preparing sandbox whose tasks cannot be read says onboarding runs, and the task box comes back once it lands', async ({ app, browser, screen }) => {
  await app.open(mock('/s/product', '&onboarding=preparing&fail=/api/sandboxes/sb_product/tasks'));
  const gate = screen.getByRole('region', 'Onboarding product');
  await expect(gate).toContainText('An agent is setting up this sandbox');
  await expect(gate).not.toContainText('missing');
  await expect(gate).toHaveAttribute('aria-busy', 'false');
  await expect(screen.getByRole('textbox', 'Task prompt')).toBeHidden();
  await expect(browser).toHaveURL(/#\/s\/product$/);
  const centred = await offCentre(browser);
  expect(centred.x).toBeLessThanOrEqual(2);
  expect(centred.y).toBeLessThanOrEqual(2);
  expect(await axeViolations(browser)).toEqual([]);

  await browser.evaluate(() => (window as unknown as Mocked).__yuliusMock.finishOnboarding('product'));
  const prompt = screen.getByRole('textbox', 'Task prompt');
  await expect(prompt).toBeVisible();
  await expect(prompt).toBeFocused();
  await expect(browser.locator('.composer > [aria-live="polite"]')).toHaveText('Onboarding finished: the task box is ready.');
  await expect(browser.locator('.onboarding-gate')).toBeHidden();
  await expect(browser.locator('.composer-quote')).toBeVisible();
});

test('a preparing sandbox without its onboarding task tells an administrator the way out', async ({ app, screen }) => {
  await app.open(mock('/s/product', '&onboarding=preparing&tasks=0'));
  const gate = screen.getByRole('region', 'Onboarding product');
  await expect(gate).toContainText('The onboarding task is missing');
  await expect(gate).toContainText('POST /api/sandboxes/sb_product/onboarding/skip');
});

// SPEC-153 OLC-006 (#399): a task that never ran is not Closed. A task created elsewhere (the API,
// an agent's tool, an automation) in a `pending` sandbox waits after the onboarding task, created
// then and resting: it reads "Waits for Start onboarding" under Working, and after an admin's Skip
// "Not started" with the stopped tasks. The Closed tab counts neither.
test('the onboarding task a new task waits for reads Waits for Start onboarding, not Closed', async ({ app, browser, screen }) => {
  await importRepository({ app, browser, screen }, 'website');
  const request = (method: string, path: string, body?: unknown) => browser.evaluate(async ([m, p, b]) => {
    const db = (window as unknown as Mocked).__yuliusMock;
    const id = db.sandboxes.find((s) => s.slug === 'website')!.id;
    return db.request(m as string, (p as string).replace('{sid}', id), b);
  }, [method, path, body ?? null]);
  expect(await request('POST', '/api/sandboxes/{sid}/tasks', { prompt: 'add a cart', harness: 'claude' })).toBe(201);
  await browser.evaluate(() => { location.hash = '#/s/website/tasks'; });
  const row = browser.locator('#main .tl-row:has-text("Prepare this repository for Yulius")');
  await expect(row).toContainText('Waits for Start onboarding');
  // Its time says how long it has waited, not when it was "closed".
  await expect(browser.locator('#main .tl-row:has-text("Prepare this repository for Yulius") .tl-time')).toHaveText(/^waiting /);
  await expect(browser.locator('#main .tl-group.group-working .tl-row:has-text("Prepare this repository for Yulius")')).toHaveCount(1);
  await expect(browser.locator('#main .tl-row:has-text("add a cart")')).toContainText('Starts by itself when #1 is done');
  const closedCount = browser.locator('#main .tl-tab:has-text("Closed") .tl-tab-count');
  await expect(closedCount).toHaveText('0');
  await expect(browser.locator('#main .tl-row:has-text("Closed")')).toHaveCount(0);
  expect(await axeViolations(browser)).toEqual([]);

  expect(await request('POST', '/api/sandboxes/{sid}/onboarding/skip')).toBe(200);
  await expect(row).toContainText('Not started');
  await expect(browser.locator('#main .tl-row:has-text("Prepare this repository for Yulius") .tl-time')).toHaveText(/^created /);
  await expect(browser.locator('#main .tl-group.group-stopped .tl-row:has-text("Prepare this repository for Yulius")')).toHaveCount(1);
  await expect(closedCount).toHaveText('0');
  await expect(row).not.toContainText('Closed');
  expect(await axeViolations(browser)).toEqual([]);
});
