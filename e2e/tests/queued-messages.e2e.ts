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

type Touching = { __yuliusMock: { touchTask: (sid: string, n: number, c: object) => void; detailReads?: Record<string, number>; holdDetail?: Promise<void> | null }; __releaseDetail?: () => void };
const touchProduct7 = (browser: Browser, changes: object) => browser.evaluate((c) => (window as unknown as Touching).__yuliusMock.touchTask('sb_product', 7, c), changes);

test('an older detail read that lands last does not bring back an older status (#316)', async ({ app, browser, screen }) => {
  await app.open(mock('/s/product/t/7'));
  const card = screen.getByRole('region', 'Queued messages');
  await expect(card).toBeVisible();
  const reads = () => browser.evaluate(() => (window as unknown as Touching).__yuliusMock.detailReads?.tsk_product_7 ?? 0);
  // The next detail read answers what the server had when it was asked, once released.
  await browser.evaluate(() => {
    const w = window as unknown as Touching;
    w.__yuliusMock.holdDetail = new Promise<void>((resolve) => { w.__releaseDetail = resolve; });
  });
  let before = await reads();
  await touchProduct7(browser, { status: 'starting', started: false });
  await expect.poll(reads).toBeGreaterThan(before);
  before = await reads();
  await touchProduct7(browser, { status: 'running', started: true });
  await expect.poll(reads).toBeGreaterThan(before);
  const strip = screen.getByRole('status').filter({ hasText: /Working|Running|Starting/ }).first();
  await expect(strip).not.toContainText('Starting');
  // The held read (taken while the task was starting) lands last: it is ignored.
  await browser.evaluate(() => (window as unknown as Touching).__releaseDetail?.());
  await expect.poll(() => browser.evaluate(() => (window as unknown as { __yuliusMock: { heldDetailAnswered?: boolean } }).__yuliusMock.heldDetailAnswered === true)).toBe(true);
  await browser.evaluate(() => new Promise((resolve) => requestAnimationFrame(() => setTimeout(resolve, 0))));
  await expect(strip).not.toContainText('Starting');
  await expect(card).toBeHidden();
});

test('text typed into the queue box stays, with Queue off, when the task starts (#324)', async ({ app, browser, screen }) => {
  await app.open(mock('/s/product/t/7'));
  const card = screen.getByRole('region', 'Queued messages');
  const box = card.getByRole('textbox', 'Message for the agent');
  await box.click();
  await browser.keyboard.type('Only for paid invoices.');
  await touchProduct7(browser, { status: 'starting', started: false });
  await expect(card).toContainText('The task has started');
  await expect(box).toBeVisible();
  await expect(box).toHaveValue('Only for paid invoices.');
  await expect(card.getByRole('button', 'Queue')).toBeDisabled();
  // Once dismissed, the card goes like any started task's.
  await card.getByRole('button', 'Dismiss').click();
  await expect(card).toBeHidden();
});

test('a refusal stays in place with its text when the task starts (#324)', async ({ app, browser, screen }) => {
  await app.open(mock('/s/product/t/7'));
  const card = screen.getByRole('region', 'Queued messages');
  await expect(card).toBeVisible();
  await browser.evaluate(() => {
    const task = (window as unknown as MockTasks).__yuliusMock.tasks.find((t) => t.id === 'tsk_product_7');
    if (task) task.started = true;
  });
  const box = card.getByRole('textbox', 'Message for the agent');
  await box.click();
  await browser.keyboard.type('Send them in Polish too.');
  await card.getByRole('button', 'Queue').click();
  await expect(card.getByRole('alert')).toContainText('type into its session instead');
  // The server says so a moment later; the reason and the text stay visible.
  await touchProduct7(browser, { status: 'running', started: true });
  await expect(card).toContainText('The task has started');
  await expect(card.getByRole('alert')).toContainText('type into its session instead');
  await expect(box).toHaveValue('Send them in Polish too.');
  await expect(card.getByRole('button', 'Queue')).toBeDisabled();
});

test('an older read is still applied when the newer one fails (#316)', async ({ app, browser, screen }) => {
  await app.open(mock('/s/product/t/7'));
  const card = screen.getByRole('region', 'Queued messages');
  const request = card.getByRole('region', 'Request');
  await expect(request).toContainText('Invoice emails');
  type Failing = Touching & { __yuliusMock: { failNextDetail?: boolean; tasks: { id: string; prompt?: string }[] } };
  const reads = () => browser.evaluate(() => (window as unknown as Touching).__yuliusMock.detailReads?.tsk_product_7 ?? 0);
  // Someone else queued a message; the read that sees it is held, and the next read fails.
  await browser.evaluate(() => {
    const w = window as unknown as Failing;
    const task = w.__yuliusMock.tasks.find((t) => t.id === 'tsk_product_7')!;
    task.prompt = `${task.prompt}\n\nFrom Anna: weekdays only.`;
    w.__yuliusMock.holdDetail = new Promise<void>((resolve) => { w.__releaseDetail = resolve; });
  });
  let before = await reads();
  await touchProduct7(browser, {});
  await expect.poll(reads).toBeGreaterThan(before);
  await browser.evaluate(() => { (window as unknown as Failing).__yuliusMock.failNextDetail = true; });
  before = await reads();
  await touchProduct7(browser, {});
  await expect.poll(reads).toBeGreaterThan(before);
  await expect(screen.getByText(/Couldn’t load task details/)).toBeVisible();
  await browser.evaluate(() => (window as unknown as Touching).__releaseDetail?.());
  await expect(request).toContainText('From Anna: weekdays only.');
});

test('a task cancelled while text sits in the queue box takes the card away (#324)', async ({ app, browser, screen }) => {
  await app.open(mock('/s/product/t/7'));
  const card = screen.getByRole('region', 'Queued messages');
  await card.getByRole('textbox', 'Message for the agent').click();
  await browser.keyboard.type('Never mind.');
  await touchProduct7(browser, { status: 'stopped_cancelled' });
  await expect(card).toBeHidden();
});

test('a card kept after a start is a plain queue box again when the task waits before running (#324)', async ({ app, browser, screen }) => {
  await app.open(mock('/s/product/t/7'));
  const card = screen.getByRole('region', 'Queued messages');
  await expect(card).toBeVisible();
  await browser.evaluate(() => {
    const task = (window as unknown as MockTasks).__yuliusMock.tasks.find((t) => t.id === 'tsk_product_7');
    if (task) task.started = true;
  });
  const box = card.getByRole('textbox', 'Message for the agent');
  await box.click();
  await browser.keyboard.type('Send them in Polish too.');
  await card.getByRole('button', 'Queue').click();
  await expect(card.getByRole('alert')).toContainText('type into its session instead');
  await touchProduct7(browser, { status: 'starting' });
  await expect(card).toContainText('The task has started');
  // The start found no capacity: it never ran, so the box takes messages again.
  await browser.evaluate(() => {
    const task = (window as unknown as MockTasks).__yuliusMock.tasks.find((t) => t.id === 'tsk_product_7');
    if (task) task.started = false;
  });
  await touchProduct7(browser, { status: 'waiting_capacity', started: false });
  await expect(card.getByRole('button', 'Queue')).toBeEnabled();
  await expect(card).not.toContainText('The task has started');
  await expect(card).not.toContainText('type into its session instead');
  await expect(box).toHaveValue('Send them in Polish too.');
});

test('an older answer applied after a failed newer read keeps the newer status (#316)', async ({ app, browser, screen }) => {
  await app.open(mock('/s/product/t/7'));
  await expect(screen.getByRole('region', 'Queued messages')).toBeVisible();
  const reads = () => browser.evaluate(() => (window as unknown as Touching).__yuliusMock.detailReads?.tsk_product_7 ?? 0);
  await browser.evaluate(() => {
    const w = window as unknown as Touching;
    w.__yuliusMock.holdDetail = new Promise<void>((resolve) => { w.__releaseDetail = resolve; });
  });
  let before = await reads();
  await touchProduct7(browser, { status: 'starting', started: false });
  await expect.poll(reads).toBeGreaterThan(before);
  await browser.evaluate(() => { (window as unknown as { __yuliusMock: { failNextDetail: boolean } }).__yuliusMock.failNextDetail = true; });
  before = await reads();
  await touchProduct7(browser, { status: 'running', started: true });
  await expect.poll(reads).toBeGreaterThan(before);
  const strip = screen.getByRole('status').filter({ hasText: /Working|Running|Starting/ }).first();
  await expect(strip).not.toContainText('Starting');
  await browser.evaluate(() => (window as unknown as Touching).__releaseDetail?.());
  await expect.poll(() => browser.evaluate(() => (window as unknown as { __yuliusMock: { heldDetailAnswered?: boolean } }).__yuliusMock.heldDetailAnswered === true)).toBe(true);
  await browser.evaluate(() => new Promise((resolve) => requestAnimationFrame(() => setTimeout(resolve, 0))));
  await expect(strip).not.toContainText('Starting');
});

// SPEC-168 (task #158): "For images here we want movable but also clickable placeholders, not raw
// "@" - like in the task view. I also want to upload images to the queue."

type Touch = { __yuliusMock: { touchTask: (sid: string, n: number, c: object) => void } };
const PRODUCT_DRAFTS = '/srv/yulius/sandboxes/sb_product/attachments/drafts/usr_mkarp';

test('the request shows its files as placeholders that open them', async ({ app, browser, screen }) => {
  await app.open(mock('/s/product/t/7'));
  const card = screen.getByRole('region', 'Queued messages');
  const request = card.getByRole('region', 'Request');
  await expect(request).toContainText('Invoice emails');
  // The composer wrote two files inside the text, and an older prompt's file on a line of its own;
  // one of them is no longer on the server (another person's drafts are not this task's).
  const prompt = `Invoice emails: this part @'${PRODUCT_DRAFTS}/shot.png' looks wrong, the log is @'${PRODUCT_DRAFTS}/notes.txt'.\nAnd @'/srv/yulius/sandboxes/sb_product/attachments/drafts/usr_bob/gone.png'\n@${PRODUCT_DRAFTS}/second.png`;
  await browser.evaluate((p) => (window as unknown as Touch).__yuliusMock.touchTask('sb_product', 7, { prompt: p }), prompt);

  await expect(request.getByRole('button', 'Image 1, shot.png')).toBeVisible();
  await expect(request.getByRole('button', 'notes.txt')).toBeVisible();
  await expect(request.getByRole('button', 'Image 2, gone.png')).toBeVisible();
  await expect(request.getByRole('button', 'Image 3, second.png')).toBeVisible();
  await expect(request).not.toContainText("@'");
  await expect(request).not.toContainText('/srv/yulius');
  // The text as written, its lines kept.
  expect(await browser.evaluate(() => document.querySelector('.queue-request')?.textContent ?? null))
    .toBe('Invoice emails: this part Image 1 looks wrong, the log is notes.txt.\nAnd Image 2\nImage 3');
  expect(await axeViolations(browser)).toEqual([]);

  // A click opens the image in the preview dialog.
  await request.getByRole('button', 'Image 1, shot.png').click();
  const dialog = screen.getByRole('dialog', 'shot.png');
  await expect(dialog.getByRole('img', 'shot.png')).toBeVisible();
  await browser.keyboard.press('Escape');
  await expect(dialog).toBeHidden();

  // Enter on a placeholder opens it too; a file gone from the server says so.
  await request.getByRole('button', 'Image 2, gone.png').focus();
  await browser.keyboard.press('Enter');
  await expect(browser.locator('.toast-msg:has-text("gone.png is no longer on the server")')).toBeVisible();
  await expect(screen.getByRole('dialog', 'gone.png')).toBeHidden();
});
