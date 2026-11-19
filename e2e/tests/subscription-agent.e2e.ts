import { type Browser, test } from '@e2e-dev/web';
import { expect } from 'e2e';
import { axeViolations, failOnPageErrors, firstShown, mock } from './support.ts';

failOnPageErrors();

// Each subscription card names its agent (SPEC-513): a badge under the name, before the email,
// says Claude, Codex or Antigravity, and its title the vendor, wherever the card is shown.

type Mocked = { __yuliusMock: { connectCodex: () => void; connectAntigravity: () => void } };

/** Connects a Codex and an Antigravity subscription, as an administrator elsewhere would. */
const connectOthers = (browser: Browser) => browser.evaluate(() => {
  const db = (window as unknown as Mocked).__yuliusMock;
  db.connectCodex();
  db.connectAntigravity();
});

/** Each card's name, its agent badge's text and the badge's title, by name, within `cards`. */
const badges = (browser: Browser, cards: string) => browser.evaluate((selector) => Object.fromEntries(
  [...document.querySelectorAll(selector)].map((card) => {
    const badge = card.querySelector('.account-subline .agent-badge');
    return [card.querySelector('.account-name strong')?.textContent ?? '', badge ? [badge.textContent, badge.getAttribute('title')] : null];
  })), cards);

test('each card on the Subscriptions page names its agent and the vendor', async ({ app, browser }) => {
  await app.open(mock('/admin/accounts', '&ambient=0'));
  await firstShown(browser.locator('main .account-card').first());
  await connectOthers(browser);
  await expect(browser.locator('main .account-card:has-text("Antigravity")')).toBeVisible();
  await expect(browser.locator('main .account-card:has-text("Codex")')).toBeVisible();

  const shown = await badges(browser, 'main .account-card');
  expect(shown['Claude #1']).toEqual(['Claude', 'Claude Code subscription · Anthropic']);
  expect(shown['Claude spare']).toEqual(['Claude', 'Claude Code subscription · Anthropic']);
  expect(shown.Codex).toEqual(['Codex', 'Codex subscription · OpenAI']);
  expect(shown.Antigravity).toEqual(['Antigravity', 'Antigravity subscription · Google Gemini']);
  // The name stays whole, and the email beside the badge is unchanged.
  await expect(browser.locator('main .account-card:has-text("Claude #1") .account-name strong')).toHaveText('Claude #1');
  await expect(browser.locator('main .account-card:has-text("Claude #1") .account-sub')).toHaveText('mkarp@example.com');
  expect(await axeViolations(browser)).toEqual([]);
});

test('the Dashboard\'s folded subscriptions name their agent too', async ({ app, browser }) => {
  await app.open(mock('/s/studio', '&ambient=0'));
  await connectOthers(browser);
  await browser.evaluate(() => { location.hash = '#/s/studio/dashboard'; });
  await expect(browser.locator('.db-card .account-fold:has-text("Antigravity")')).toBeVisible();

  const shown = await badges(browser, '.db-card .account-fold');
  expect(shown['Claude #2']).toEqual(['Claude', 'Claude Code subscription · Anthropic']);
  expect(shown.Codex).toEqual(['Codex', 'Codex subscription · OpenAI']);
  expect(shown.Antigravity).toEqual(['Antigravity', 'Antigravity subscription · Google Gemini']);
});
