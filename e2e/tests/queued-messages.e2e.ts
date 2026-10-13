import { test, type Browser } from '@e2e-dev/web';
import { expect } from 'e2e';
import { axeViolations, failOnPageErrors, mock } from './support.ts';

failOnPageErrors();

// Queued messages (SPEC-052): a task that waits and has never started takes messages into its
// request, so a person can correct it before the agent sees it. The tests that queue use the
// chained product #7: the mock's ambient activity starts the queued studio #8 after ~12 s, which a
// slow run would race.

type MockTasks = { __yuliusMock: { tasks: { id: string; prompt?: string; started?: boolean }[] } };

const storedRequest = (browser: Browser, id: string) =>
  browser.evaluate((tid) => (window as unknown as MockTasks).__yuliusMock.tasks.find((t) => t.id === tid)?.prompt ?? '', id);

test('a waiting task takes a message into its request', async ({ app, browser, screen }) => {
  await app.open(mock('/s/product/t/7'));

  const card = screen.getByRole('region', 'Queued messages');
  await expect(card).toBeVisible();
  const request = card.getByRole('region', 'Request');
  await expect(request).toContainText('Invoice emails');
  expect(await axeViolations(browser)).toEqual([]);

  const box = card.getByRole('textbox', 'Message for the agent');
  await box.click();
  await browser.keyboard.type('Only for paid invoices, please.');
  await card.getByRole('button', 'Queue').click();

  await expect(box).toHaveValue('');
  await expect(request).toContainText(/Added by mkarp before the task started \(.+ UTC\):/);
  await expect(request).toContainText('Only for paid invoices, please.');
  expect(await storedRequest(browser, 'tsk_product_7')).toMatch(/Invoice emails\n\n\*\*Added by mkarp before the task started \(.+ UTC\):\*\*\n\nOnly for paid invoices, please\.$/);

  // ⌘/Ctrl+Enter queues the next one.
  await box.click();
  await browser.keyboard.type('And skip weekends.');
  await browser.keyboard.press(process.platform === 'darwin' ? 'Meta+Enter' : 'Control+Enter');
  await expect(box).toHaveValue('');
  await expect(request).toContainText('And skip weekends.');
});

test('a chained task takes messages, and a refused one keeps the text with the reason', async ({ app, browser, screen }) => {
  await app.open(mock('/s/product/t/7'));

  const card = screen.getByRole('region', 'Queued messages');
  await expect(card).toBeVisible();
  // The task started meanwhile (its view has not heard yet): the server refuses.
  await browser.evaluate(() => {
    const task = (window as unknown as MockTasks).__yuliusMock.tasks.find((t) => t.id === 'tsk_product_7');
    if (task) task.started = true;
  });
  const box = card.getByRole('textbox', 'Message for the agent');
  await box.click();
  await browser.keyboard.type('Send them in Polish too.');
  await card.getByRole('button', 'Queue').click();

  await expect(card.getByRole('alert')).toContainText('type into its session instead');
  await expect(box).toHaveValue('Send them in Polish too.');
  expect(await storedRequest(browser, 'tsk_product_7')).not.toContain('Polish');
});

test('a running task has no queued messages card', async ({ app, screen }) => {
  await app.open(mock('/s/studio/t/12'));
  await expect(screen.getByRole('heading', { level: 1 })).toHaveText(/#12/);
  await expect(screen.getByRole('region', 'Queued messages')).toBeHidden();
});

test('once the task has started, a pause for capacity brings no card back', async ({ app, browser, screen }) => {
  await app.open(mock('/s/product/t/7'));
  const card = screen.getByRole('region', 'Queued messages');
  await expect(card).toBeVisible();

  // It starts — while `starting` the server has no run recorded yet, so the detail read then
  // still says it has not started — then a turn boundary pauses it for capacity, session alive.
  type Mocked = { __yuliusMock: { touchTask: (sid: string, n: number, c: object) => void; detailReads?: Record<string, number> } };
  const touch = (changes: object) => browser.evaluate((c) => (window as unknown as Mocked).__yuliusMock.touchTask('sb_product', 7, c), changes);
  const reads = () => browser.evaluate(() => (window as unknown as Mocked).__yuliusMock.detailReads?.tsk_product_7 ?? 0);
  const before = await reads();
  await touch({ status: 'starting', started: false });
  await expect(card).toBeHidden();
  // The view's read of the detail while `starting` has landed (it said: not started).
  await expect.poll(reads).toBeGreaterThan(before);
  await touch({ status: 'running', started: true });
  await touch({ status: 'waiting_capacity' });
  await expect(screen.getByRole('status').filter({ hasText: 'Paused' }).first()).toBeVisible();
  await expect(card).toBeHidden();
});

test('the terminal showing late does not take the keys typed into the queue box', async ({ app, browser, screen }) => {
  // The terminal's first screen waits until the test releases it.
  await browser.addInitScript(() => {
    const w = window as unknown as { __yuliusHoldTerminal?: Promise<void>; __releaseTerminal?: () => void };
    w.__yuliusHoldTerminal = new Promise<void>((resolve) => { w.__releaseTerminal = resolve; });
  });
  await app.open(mock('/s/product/t/7'));
  const box = screen.getByRole('region', 'Queued messages').getByRole('textbox', 'Message for the agent');
  await box.click();
  await browser.keyboard.type('Hello');
  await browser.evaluate(() => (window as unknown as { __releaseTerminal: () => void }).__releaseTerminal());
  await expect.poll(() => browser.evaluate(() => document.querySelector('.term-pending') === null && document.querySelector('.term-placeholder') === null)).toBe(true);
  await browser.keyboard.type(' world');
  await expect(box).toHaveValue('Hello world');
  await expect(box).toBeFocused();
});

test('a Shell opened while the task terminal loads gets the keyboard when it shows after it', async ({ app, browser, screen }) => {
  type Holds = { __yuliusHoldTerminal?: Promise<void>; __yuliusHoldShell?: Promise<void>; __releaseTerminal?: () => void; __releaseShell?: () => void };
  await browser.addInitScript(() => {
    const w = window as unknown as Holds;
    w.__yuliusHoldTerminal = new Promise<void>((resolve) => { w.__releaseTerminal = resolve; });
    w.__yuliusHoldShell = new Promise<void>((resolve) => { w.__releaseShell = resolve; });
  });
  await app.open(mock('/s/studio/t/12'));
  await screen.getByRole('button', /^Show tools/).click();
  await screen.getByRole('button', 'Shell').click();

  // The task terminal shows first (the keyboard is on the Shell button: it takes it), then the
  // Shell asked for afterwards shows: the terminal asked for last gets the keys.
  await browser.evaluate(() => (window as unknown as Holds).__releaseTerminal?.());
  await expect.poll(() => browser.evaluate(() => document.querySelector('.taskview .term-pending') === null)).toBe(true);
  await browser.evaluate(() => (window as unknown as Holds).__releaseShell?.());
  const shellInput = browser.locator('.term-views .term-slot:not([hidden]) .xterm-helper-textarea');
  await expect(shellInput).toBeFocused();
});

test('a Shell that took the keyboard keeps it when the task terminal asked earlier shows after it', async ({ app, browser, screen }) => {
  type Holds = { __yuliusHoldTerminal?: Promise<void>; __yuliusHoldShell?: Promise<void>; __releaseTerminal?: () => void; __releaseShell?: () => void };
  await browser.addInitScript(() => {
    const w = window as unknown as Holds;
    w.__yuliusHoldTerminal = new Promise<void>((resolve) => { w.__releaseTerminal = resolve; });
    w.__yuliusHoldShell = new Promise<void>((resolve) => { w.__releaseShell = resolve; });
  });
  await app.open(mock('/s/studio/t/12'));
  await screen.getByRole('button', /^Show tools/).click();
  await screen.getByRole('button', 'Shell').click();

  // The Shell, asked for after the task terminal, shows first and takes the keys (#323).
  await browser.evaluate(() => (window as unknown as Holds).__releaseShell?.());
  const shellInput = browser.locator('.term-views .term-slot:not([hidden]) .xterm-helper-textarea');
  await expect(shellInput).toBeFocused();
  // The task terminal's older request does not take them back when it shows.
  await browser.evaluate(() => (window as unknown as Holds).__releaseTerminal?.());
  await expect.poll(() => browser.evaluate(() => document.querySelector('.taskview .term-pending') === null)).toBe(true);
  await expect(shellInput).toBeFocused();
});
