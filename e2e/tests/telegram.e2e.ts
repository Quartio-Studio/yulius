import { test, type Browser } from '@e2e-dev/web';
import { expect } from 'e2e';
import { axeViolations, failOnPageErrors, firstShown, mock } from './support.ts';

failOnPageErrors();

// The personal Telegram channel (SPEC-498): an instance administrator sets and removes the bot in
// Settings → Administration (TGC-001, TGC-002); a person links their chat with a single-use code,
// chooses what reaches it and unlinks in Settings → Notifications (TGC-010..TGC-020).

type MockTelegram = {
  choices?: { done?: boolean };
  holdRead?: Promise<void>;
  releaseRead?: () => void;
  readsHeld?: number;
  lateAnswers?: number;
};
type Mocked = Window & { __yuliusMock: { telegramStart: (text: string) => boolean; telegram: MockTelegram } };

function botSends(browser: Browser, text: string): Promise<boolean> {
  return browser.evaluate((t) => (window as unknown as Mocked).__yuliusMock.telegramStart(t), text);
}

test('an instance administrator sets and removes the Telegram bot', async ({ app, browser, screen }) => {
  await app.open(mock('/settings', '&role=admin'));
  const card = screen.getByRole('region', 'Telegram bot');
  await firstShown(card.getByText(/No bot: nobody can link a Telegram chat/));
  await expect(screen.getByText(/No Telegram bot is set up on this instance yet/)).toBeVisible();
  await browser.locator('#settings-telegram-token').fill('12345:refused-token');
  await card.getByRole('button', 'Save').click();
  await expect(screen.getByText(/Telegram refused this token/)).toBeVisible();
  await browser.locator('#settings-telegram-token').fill('12345:a-good-token');
  await card.getByRole('button', 'Save').click();
  // Until Telegram's first answer the card says it is starting, then reads the state again by
  // itself (review round 2: it said "starting" until a reload).
  await expect(card.getByText(/@yulius_mock_bot is starting: waiting for Telegram's first answer/)).toBeVisible();
  await expect(card.getByText(/@yulius_mock_bot is receiving\. Set by mkarp/)).toBeVisible();
  // The token is never shown again.
  expect(await browser.evaluate(() => document.body.innerText)).not.toMatch(/a-good-token/);
  expect(await axeViolations(browser)).toEqual([]);
  await card.getByRole('button', 'Remove').click();
  await screen.getByRole('dialog', 'Remove the Telegram bot?').getByRole('button', 'Remove').click();
  await expect(card.getByText(/No bot: nobody can link a Telegram chat/)).toBeVisible();
});

test('a state read before the bot was removed does not come back after it', async ({ app, browser, screen }) => {
  await app.open(mock('/settings', '&role=admin'));
  const card = screen.getByRole('region', 'Telegram bot');
  await firstShown(card.getByText(/No bot: nobody can link a Telegram chat/));
  await browser.locator('#settings-telegram-token').fill('12345:a-good-token');
  await card.getByRole('button', 'Save').click();
  await expect(card.getByText(/is starting: waiting for Telegram's first answer/)).toBeVisible();
  // The card's next read of the state ("receiving") is answered only after the bot is removed.
  await browser.evaluate(() => {
    const t = (window as unknown as Mocked).__yuliusMock.telegram;
    t.holdRead = new Promise((resolve) => { t.releaseRead = resolve; });
  });
  await expect.poll(() => browser.evaluate(() => (window as unknown as Mocked).__yuliusMock.telegram.readsHeld ?? 0), { timeout: 10000 }).toBe(1);
  await card.getByRole('button', 'Remove').click();
  await screen.getByRole('dialog', 'Remove the Telegram bot?').getByRole('button', 'Remove').click();
  await expect(card.getByText(/No bot: nobody can link a Telegram chat/)).toBeVisible();
  await browser.evaluate(() => {
    const t = (window as unknown as Mocked).__yuliusMock.telegram;
    delete t.holdRead;
    t.releaseRead?.();
  });
  await expect.poll(() => browser.evaluate(() => (window as unknown as Mocked).__yuliusMock.telegram.lateAnswers ?? 0)).toBe(1);
  // The late answer has reached the page once a frame passed after it (review round 3).
  await browser.evaluate(() => new Promise<void>((resolve) => requestAnimationFrame(() => requestAnimationFrame(() => resolve()))));
  await expect(card.getByText(/is receiving/)).toHaveCount(0);
  await expect(card.getByText(/No bot: nobody can link a Telegram chat/)).toBeVisible();
});

test('a member does not see the bot card', async ({ app, screen }) => {
  await app.open(mock('/settings', '&role=member&telegram=1'));
  await firstShown(screen.getByRole('button', 'Link Telegram'));
  await expect(screen.getByRole('region', 'Telegram bot')).toHaveCount(0);
});

test('a person links their chat with a code, chooses what reaches it and unlinks', async ({ app, browser, screen }) => {
  await app.open(mock('/settings', '&role=member&telegram=1'));
  await firstShown(screen.getByRole('button', 'Link Telegram'));
  await expect(screen.getByText(/not end-to-end encrypted/)).toBeVisible();
  await screen.getByRole('button', 'Link Telegram').click();
  const open = screen.getByRole('link', 'Open @yulius_mock_bot');
  await expect(open).toHaveAttribute('href', 'https://t.me/yulius_mock_bot?start=k7mq2xw9tz4hb8np');
  await expect(screen.getByText('/start k7mq2xw9tz4hb8np')).toBeVisible();
  await expect(screen.getByText(/works once, for 10 minutes/)).toBeVisible();
  expect(await axeViolations(browser)).toEqual([]);

  // A wrong code links nothing; the shown one links the chat, and the page notices by itself.
  expect(await botSends(browser, '/start wrongcode0000000')).toBe(false);
  expect(await botSends(browser, '/start k7mq2xw9tz4hb8np')).toBe(true);
  await expect(screen.getByText(/Linked to @yulius_mock_bot/)).toBeVisible({ timeout: 10000 });
  const needsYou = screen.getByRole('checkbox', /Needs you/);
  await expect(needsYou).toBeChecked();
  const done = screen.getByRole('checkbox', /Done/);
  await expect(done).not.toBeChecked();
  // A member is not offered subscription problems.
  await expect(screen.getByRole('checkbox', /Subscription problems/)).toHaveCount(0);
  await done.check();
  // The choice is the server's: it is there when the page is opened again, once saved (the mock
  // answers after a random delay: leaving at once could read the choices before the save; review
  // round 3).
  await expect.poll(() => browser.evaluate(() => (window as unknown as Mocked).__yuliusMock.telegram.choices?.done === true)).toBe(true);
  await browser.evaluate(() => { window.location.hash = '#/'; });
  await expect(screen.getByRole('heading', { level: 1 })).toHaveText('Sandboxes');
  await browser.evaluate(() => { window.location.hash = '#/settings'; });
  await expect(screen.getByRole('checkbox', /Done/)).toBeChecked();
  expect(await axeViolations(browser)).toEqual([]);

  await screen.getByRole('button', 'Unlink').click();
  await screen.getByRole('dialog', 'Unlink Telegram?').getByRole('button', 'Unlink').click();
  await expect(screen.getByRole('button', 'Link Telegram')).toBeVisible();
});

test('an instance administrator may also choose subscription problems', async ({ app, screen }) => {
  await app.open(mock('/settings', '&role=admin&telegram=linked'));
  await firstShown(screen.getByRole('checkbox', /Subscription problems/));
  await expect(screen.getByText(/Linked to @yulius_mock_bot/)).toBeVisible();
});

test('a person turns their chat\'s intake on with a project, off by default (SPEC-512 TGI-001)', async ({ app, browser, screen }) => {
  await app.open(mock('/settings', '&telegram=linked'));
  const intake = browser.locator('#telegram-intake');
  await firstShown(screen.getByText('Work from this chat'));
  expect(await intake.isChecked()).toBe(false);
  await intake.check();
  await expect(screen.getByText('Your Telegram chat now answers tasks and starts new ones.')).toBeVisible();
  const saved = await browser.evaluate(() => JSON.parse(JSON.stringify((window as unknown as { __yuliusMock: { telegram: { intake?: unknown } } }).__yuliusMock.telegram.intake)));
  expect(saved).toMatchObject({ enabled: true });
  expect((saved as { sandbox_id: string | null }).sandbox_id).toBeTruthy();
  expect(await axeViolations(browser)).toEqual([]);
});
