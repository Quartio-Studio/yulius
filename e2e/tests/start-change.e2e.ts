import { test, type Browser } from '@e2e-dev/web';
import { expect } from 'e2e';
import { axeViolations, failOnPageErrors, mock, mockReady } from './support.ts';

failOnPageErrors();

// Changing how a waiting task starts (SPEC-126): a prepared or chained task's view has a start
// chip that opens New task's start menu, without the task itself; a pick changes the task. The
// mock answers like the server (`PUT /api/tasks/{tid}/start`). It signs in as mkarp.

type MockTask = { id: string; sandbox_id: string; number: number; status: string; start_at?: string | null; supertask?: boolean; afterLink?: { number: number; rule: string | null } | null };
type Mocked = { __yuliusMock: { tasks: MockTask[]; touchTask: (sid: string, n: number, c: object) => void } };
const mockTask = (n: number) => (window as unknown as Mocked).__yuliusMock.tasks.find((t) => t.sandbox_id === 'sb_studio' && t.number === n);
const openedNumber = () => Number(location.hash.match(/\/t\/(\d+)$/)?.[1]);

/** Prepares a task "When I decide" from New task and opens its view; resolves to its number. */
async function preparedTask(app: { open: (url: string) => Promise<unknown> }, browser: Browser, screen: any): Promise<number> {
  await app.open(mock('/s/studio'));
  await expect(screen.getByRole('link', /Checkout API/).first()).toBeVisible();
  await screen.getByRole('button', /Run after: Start now/).click();
  await screen.getByRole('menuitemradio', /^When I decide/).click();
  await screen.getByRole('textbox', 'Task prompt').fill('Tidy the docs');
  await screen.getByRole('button', 'Prepare task').click();
  await expect(browser).toHaveURL(/#\/s\/studio\/t\/\d+$/);
  await expect(browser.locator('.strip-status')).toHaveText('Prepared');
  return browser.evaluate(openedNumber);
}

test('a prepared task changes to run after a task, back to When I decide, then to a time (AC-CHS-8)', async ({ app, browser, screen }) => {
  const number = await preparedTask(app, browser, screen);
  const bar = browser.locator('.attention-bar');
  const chip = bar.getByRole('button', /^Starts: When I decide/);
  await expect(chip).toBeVisible();
  expect(await axeViolations(browser)).toEqual([]);

  await chip.click();
  await expect(screen.getByRole('menuitemradio', /^When I decide/)).toHaveAttribute('aria-checked', 'true');
  // The task cannot wait for itself: it is not among the tasks to run after.
  await expect(screen.getByRole('menuitemradio', /^#12 Checkout API/)).toBeVisible();
  await expect(browser.locator(`[role="menuitemradio"]:has-text("#${number} Tidy the docs")`)).toHaveCount(0);
  expect(await axeViolations(browser)).toEqual([]);
  await screen.getByRole('menuitemradio', /^#12 Checkout API/).click();
  await expect(bar).toHaveText(/Queued after #12\. It starts by itself when #12 is done\./);
  await expect(bar.getByRole('button', /^Starts: After #12/)).toBeVisible();
  expect(await browser.evaluate(mockTask, number)).toMatchObject({ status: 'chained', afterLink: { number: 12, rule: null } });

  // Back to When I decide: prepared, no chain.
  await bar.getByRole('button', /^Starts: After #12/).click();
  await expect(screen.getByRole('menuitemradio', /^#12 Checkout API/)).toHaveAttribute('aria-checked', 'true');
  await screen.getByRole('menuitemradio', /^When I decide/).click();
  await expect(browser.locator('.strip-status')).toHaveText('Prepared');
  await expect(bar.getByRole('button', /^Starts: When I decide/)).toBeVisible();
  expect(await browser.evaluate(mockTask, number)).toMatchObject({ status: 'prepared', start_at: null, afterLink: null });

  // At a time…: tomorrow at 09:00, local time.
  await bar.getByRole('button', /^Starts: When I decide/).click();
  await screen.getByRole('menuitemradio', /^At a time…/).click();
  const dialog = screen.getByRole('dialog', 'Start at a time');
  const local = await browser.evaluate(() => {
    const d = new Date();
    d.setDate(d.getDate() + 1);
    const pad = (n: number) => String(n).padStart(2, '0');
    return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}T09:00`;
  });
  await dialog.getByLabel('Date and time').fill(local);
  await dialog.getByRole('button', 'Use this time').click();
  await expect(browser.locator('.strip-status')).toHaveText(/^Scheduled · tomorrow/);
  await expect(bar.getByRole('button', /^Starts: At tomorrow/)).toBeVisible();

  // Supertask starts it now, alone.
  await bar.getByRole('button', /^Starts: At tomorrow/).click();
  await screen.getByRole('menuitemradio', /^Supertask/).click();
  await expect(browser.locator('.strip-status')).not.toHaveText(/Prepared|Scheduled/);
  expect(await browser.evaluate(mockTask, number)).toMatchObject({ supertask: true });
});

test('a chained task changes to When I decide; someone else sees no start chip (AC-CHS-8)', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio'));
  await expect(screen.getByRole('link', /Checkout API/).first()).toBeVisible();
  await screen.getByRole('button', /Run after: Start now/).click();
  await screen.getByRole('menuitemradio', /^#12 Checkout API/).click();
  await screen.getByRole('textbox', 'Task prompt').fill('Write the checkout docs');
  await screen.getByRole('button', /^Start task/).click();
  await expect(browser).toHaveURL(/#\/s\/studio\/t\/\d+$/);
  const number = await browser.evaluate(openedNumber);
  const bar = browser.locator('.attention-bar');
  await bar.getByRole('button', /^Starts: After #12/).click();
  await screen.getByRole('menuitemradio', /^When I decide/).click();
  await expect(browser.locator('.strip-status')).toHaveText('Prepared');
  expect(await browser.evaluate(mockTask, number)).toMatchObject({ status: 'prepared', afterLink: null });

  // Handed to someone else: read-only, no chip.
  await mockReady(browser);
  await browser.evaluate((n) => (window as unknown as Mocked).__yuliusMock.touchTask('sb_studio', n, { owner: { login: 'anna-q' } }), number);
  await expect(screen.getByText('@anna-q owns this task:')).toBeVisible();
  await expect(bar.getByRole('button', /^Starts:/)).toHaveCount(0);
});
