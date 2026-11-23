import { test, type Browser } from '@e2e-dev/web';
import { expect } from 'e2e';
import { axeViolations, failOnPageErrors, firstShown, mock, touchTask } from './support.ts';

failOnPageErrors();

// SPEC-628 (task #612): Fork task. A new task continues a copy of a task's conversation from its
// branch's head, and both pages link the other.

type WithMock = { __yuliusMock: { lastFork?: { task: string; message?: string; model?: string; account_id?: string } } };
const lastFork = (browser: Browser) => browser.evaluate(() => (window as unknown as WithMock).__yuliusMock.lastFork ?? null);

test('Fork… in the task menu forks the task with a message and opens the fork; each page links the other', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/t/12', '&ambient=0'));
  await firstShown(screen.getByRole('heading', { level: 1 }), '#12 Checkout API');
  await browser.locator('.stepstrip [aria-label="Task actions"]').click();
  await screen.getByRole('menuitem', /^Fork…/).click();
  const dialog = screen.getByRole('dialog', 'Fork #12');
  await expect(dialog).toBeVisible();
  await expect(dialog.getByText(/goes on untouched\. Uncommitted changes stay behind\./)).toBeVisible();
  expect(await axeViolations(browser)).toEqual([]);
  await dialog.getByRole('textbox', /^What should it do differently\?/).fill('Try Stripe Checkout instead of payment intents.');
  await dialog.getByRole('button', 'Fork').click();

  await expect(screen.getByText(/continues #12's conversation\.$/)).toBeVisible();
  await firstShown(screen.getByRole('heading', { level: 1 }), /^#\d+ Fork of #12: Checkout API$/);
  expect(await lastFork(browser)).toEqual({ task: 'tsk_studio_12', message: 'Try Stripe Checkout instead of payment intents.' });
  const origin = browser.locator('.stepstrip a.chain-chip:has-text("Fork of #12")');
  await expect(origin).toBeVisible();
  await expect(origin).toHaveAttribute('title', /^Continues #12 Checkout API from commit [0-9a-f]{7}$/);

  // The source names its fork, a link back.
  await origin.click();
  await firstShown(screen.getByRole('heading', { level: 1 }), '#12 Checkout API');
  await expect(browser.locator('.stepstrip a.chain-chip:has-text("Forked: #")')).toBeVisible();
});

test('the task details page forks too and lists the fork', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/tasks/12', '&ambient=0'));
  await firstShown(screen.getByRole('heading', { level: 1 }), /#12/);
  const fork = screen.getByRole('button', 'Fork…');
  await expect(fork).toBeEnabled();
  await fork.click();
  const dialog = screen.getByRole('dialog', 'Fork #12');
  await dialog.getByRole('button', 'Fork').click();
  await firstShown(screen.getByRole('heading', { level: 1 }), /^#\d+ Fork of #12: Checkout API$/);
  expect(await lastFork(browser)).toEqual({ task: 'tsk_studio_12' });
  await browser.evaluate(() => { location.hash = '#/s/studio/tasks/12'; });
  await expect(browser.locator('.ti-kv dt:has-text("Forks")')).toBeVisible();
  await expect(browser.locator('.ti-kv dd a:has-text("Fork of #12: Checkout API")')).toBeVisible();
});

test('Fork… is off, with the reason, for an Antigravity task and a task that has not started', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/t/13', '&ambient=0'));
  await firstShown(screen.getByRole('heading', { level: 1 }), '#13 Improve the code');
  await touchTask(browser, 'sb_studio', 13, { harness: 'antigravity' });
  await browser.locator('.stepstrip [aria-label="Task actions"]').click();
  // A disabled item's reason is part of its name, as for every item of the menu.
  await expect(screen.getByRole('menuitem', 'Fork… Antigravity cannot fork a conversation')).toBeDisabled();
  await browser.keyboard.press('Escape');

  // A task that rests without ever having run (`not_started`).
  await app.open(mock('/s/studio/t/5', '&ambient=0'));
  await firstShown(screen.getByRole('heading', { level: 1 }), '#5 Docs restructure');
  await touchTask(browser, 'sb_studio', 5, { neverRan: true });
  await browser.locator('.stepstrip [aria-label="Task actions"]').click();
  await expect(screen.getByRole('menuitem', 'Fork… Not started yet: no conversation to fork')).toBeDisabled();
  await browser.keyboard.press('Escape');

  // The details page says why beside its button, in words keyboard and touch users reach.
  await app.open(mock('/s/studio/tasks/8', '&ambient=0'));
  await firstShown(screen.getByRole('heading', { level: 1 }), /#8/);
  const off = screen.getByRole('button', 'Fork…');
  await expect(off).toHaveAttribute('aria-disabled', 'true');
  await expect(browser.locator('.fork-why')).toHaveText('Not started yet: no conversation to fork');
  await off.focus();
  await browser.keyboard.press('Enter');
  await expect(screen.getByRole('dialog', 'Fork #8')).toBeHidden();
  expect(await axeViolations(browser)).toEqual([]);

  await app.open(mock('/s/studio/t/8', '&ambient=0'));
  await firstShown(screen.getByRole('heading', { level: 1 }), '#8 Nightly feedback digest');
  await browser.locator('.stepstrip [aria-label="Task actions"]').click();
  await expect(screen.getByRole('menuitem', 'Fork… Not started yet: no conversation to fork')).toBeDisabled();
});

test('a refused fork says why in the dialog, and a deleted source leaves its fork a plain origin', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/t/4', '&ambient=0'));
  await firstShown(screen.getByRole('heading', { level: 1 }), '#4 Payment webhook');
  // The server refuses it for a reason the page could not know: the reason shows in the dialog.
  await browser.evaluate(() => { (window as unknown as { __yuliusMock: { refuseFork?: string } }).__yuliusMock.refuseFork = '#4 has no conversation to fork yet: start it first'; });
  await browser.locator('.stepstrip [aria-label="Task actions"]').click();
  await screen.getByRole('menuitem', /^Fork…/).click();
  const dialog = screen.getByRole('dialog', 'Fork #4');
  await dialog.getByRole('button', 'Fork').click();
  await expect(dialog.getByText('#4 has no conversation to fork yet: start it first')).toBeVisible();
  await dialog.getByRole('button', 'Cancel').click();

  await browser.evaluate(() => { delete (window as unknown as { __yuliusMock: { refuseFork?: string } }).__yuliusMock.refuseFork; });
  await browser.locator('.stepstrip [aria-label="Task actions"]').click();
  await screen.getByRole('menuitem', /^Fork…/).click();
  await screen.getByRole('dialog', 'Fork #4').getByRole('button', 'Fork').click();
  await firstShown(screen.getByRole('heading', { level: 1 }), /^#\d+ Fork of #4: Payment webhook$/);
  await expect(browser.locator('.stepstrip a.chain-chip:has-text("Fork of #4")')).toBeVisible();

  // #4 is deleted: the fork's chip stays, plain, and says so.
  const forkPage = await browser.evaluate(() => location.hash);
  await browser.locator('.stepstrip a.chain-chip:has-text("Fork of #4")').click();
  await firstShown(screen.getByRole('heading', { level: 1 }), '#4 Payment webhook');
  await browser.locator('.stepstrip [aria-label="Task actions"]').click();
  await screen.getByRole('menuitem', /^Delete permanently/).click();
  await screen.getByRole('dialog', /permanently\?$/).getByRole('button', 'Delete permanently').click();
  await expect(screen.getByText('Task #4 deleted.')).toBeVisible();
  await browser.evaluate((hash) => { location.hash = hash; }, forkPage);
  await firstShown(screen.getByRole('heading', { level: 1 }), /^#\d+ Fork of #4: Payment webhook$/);
  const plain = browser.locator('.stepstrip span.chain-chip:has-text("Fork of #4")');
  await expect(plain).toBeVisible();
  await expect(plain).toHaveAttribute('title', /\(deleted since\)$/);
  expect(await axeViolations(browser)).toEqual([]);
});
