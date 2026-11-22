import { test } from '@e2e-dev/web';
import { expect } from 'e2e';
import { axeViolations, failOnPageErrors, mock, promptText } from './support.ts';

failOnPageErrors();

// Prepared tasks (SPEC-105): written now, started later — when the person presses Start now
// ("When I decide") or by itself at a picked time. The mock answers like the server
// (`status: "prepared"`, `start_at`).

type MockTask = { sandbox_id: string; number: number; status: string; start_at?: string | null };
const mockTask = (n: number) => (window as unknown as { __yuliusMock: { tasks: MockTask[] } }).__yuliusMock.tasks.find((t) => t.sandbox_id === 'sb_studio' && t.number === n);
const openedNumber = () => Number(location.hash.match(/\/t\/(\d+)$/)?.[1]);

test('When I decide prepares a task, and Start now in its view starts it (AC-PRP-6)', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio'));
  await expect(screen.getByRole('link', /Checkout API/).first()).toBeVisible();
  await screen.getByRole('button', /Run after: Start now/).click();
  await expect(screen.getByRole('menuitemradio', /^At a time…/)).toBeVisible();
  const item = screen.getByRole('menuitemradio', /^When I decide/);
  await expect(item).toHaveAttribute('aria-checked', 'false');
  await item.click();
  await expect(screen.getByRole('button', /Run after: When I decide/)).toBeVisible();
  expect(await axeViolations(browser)).toEqual([]);

  // A long prompt is still one task: a prepared task is one request by intent.
  await screen.getByRole('textbox', 'Task prompt').fill(`Tidy the docs: ${'fix every broken link and heading. '.repeat(8)}`);
  await screen.getByRole('button', 'Prepare task').click();
  await expect(browser).toHaveURL(/#\/s\/studio\/t\/\d+$/);
  const number = await browser.evaluate(openedNumber);
  await expect(browser.locator('.strip-status')).toHaveText('Prepared');
  await expect(browser.locator('.attention-bar')).toHaveText(/Prepared\. It starts when you press Start now\./);
  expect(await browser.evaluate(mockTask, number)).toMatchObject({ status: 'prepared' });
  expect(await axeViolations(browser)).toEqual([]);

  // The composer is back to Start now.
  await browser.evaluate(() => { location.hash = '#/s/studio'; });
  await expect(screen.getByRole('button', /Run after: Start now/)).toBeVisible();
  await browser.evaluate((n) => { location.hash = `#/s/studio/t/${n}`; }, number);
  await screen.getByRole('button', 'Start now').click();
  await expect(browser.locator('.strip-status')).not.toHaveText('Prepared');
  expect(await browser.evaluate(mockTask, number)).not.toMatchObject({ status: 'prepared' });
});

test('At a time… schedules a task; its time can change or give way to When I decide (AC-PRP-6)', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio'));
  await expect(screen.getByRole('link', /Checkout API/).first()).toBeVisible();
  await screen.getByRole('button', /Run after: Start now/).click();
  await screen.getByRole('menuitemradio', /^At a time…/).click();
  const dialog = screen.getByRole('dialog', 'Start at a time');
  await expect(dialog).toBeVisible();
  expect(await axeViolations(browser)).toEqual([]);
  const input = dialog.getByLabel('Date and time');

  // A time in the past is refused in the dialog.
  await input.fill('2020-01-01T09:00');
  await dialog.getByRole('button', 'Use this time').click();
  await expect(dialog.getByRole('alert')).toHaveText(/passed/);

  // Tomorrow at 09:00, local time.
  const local = await browser.evaluate(() => {
    const d = new Date();
    d.setDate(d.getDate() + 1);
    const pad = (n: number) => String(n).padStart(2, '0');
    return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}T09:00`;
  });
  const iso = await browser.evaluate((v) => new Date(v).toISOString().replace(/\.\d{3}Z$/, 'Z'), local);
  await input.fill(local);
  await dialog.getByRole('button', 'Use this time').click();
  await expect(screen.getByRole('button', /Run after: At tomorrow/)).toBeVisible();

  await screen.getByRole('textbox', 'Task prompt').fill('Run the nightly cleanup');
  await screen.getByRole('button', 'Schedule task').click();
  await expect(browser).toHaveURL(/#\/s\/studio\/t\/\d+$/);
  const number = await browser.evaluate(openedNumber);
  await expect(browser.locator('.strip-status')).toHaveText(/^Scheduled · tomorrow/);
  await expect(browser.locator('.attention-bar')).toHaveText(/Scheduled\. It starts by itself tomorrow/);
  expect(await browser.evaluate(mockTask, number)).toMatchObject({ status: 'prepared', start_at: iso });

  // Change time… → When I decide clears the time.
  await screen.getByRole('button', 'Change time…').click();
  const again = screen.getByRole('dialog', 'Start at a time');
  await expect(again.getByLabel('Date and time')).toHaveValue(local);
  await again.getByRole('button', 'When I decide').click();
  await expect(browser.locator('.strip-status')).toHaveText('Prepared');
  expect(await browser.evaluate(mockTask, number)).toMatchObject({ status: 'prepared', start_at: null });

  // Cancel ends it, once confirmed.
  await browser.locator('.attention-bar').getByRole('button', 'Cancel').click();
  await screen.getByRole('dialog', `Cancel task #${number}?`).getByRole('button', 'Cancel task').click();
  await expect(browser.locator('.strip-status')).toHaveText('Stopped');
});

test('a kept draft whose time has passed waits for you instead; the send button says what it does', async ({ app, browser, screen }) => {
  // Stored before the page opens (#269): an open composer would replace a draft written under it.
  await browser.addInitScript(() => {
    localStorage.setItem('yulius.draft.usr_1.sb_studio', JSON.stringify({
      text: 'Kept from yesterday', prepare: { at: new Date(Date.now() - 24 * 3600e3).toISOString() },
    }));
  });
  await app.open(mock('/s/studio'));
  await expect.poll(() => promptText(browser)).toBe('Kept from yesterday');
  await expect(screen.getByRole('button', /Run after: When I decide/)).toBeVisible();
  await expect(screen.getByRole('button', 'Prepare task')).toHaveAttribute('title', /^Prepare task \(/);
});
