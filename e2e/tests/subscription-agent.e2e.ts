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

/**
 * Each card's name, its agent badge's text and the badge's title, by name, within `cards`; a badge
 * that is not the first thing of the line under the name, before the email, reads as missing.
 */
const badges = (browser: Browser, cards: string) => browser.evaluate((selector) => Object.fromEntries(
  [...document.querySelectorAll(selector)].map((card) => {
    const badge = card.querySelector('.account-subline > .agent-badge:first-child');
    return [card.querySelector('.account-name strong')?.textContent ?? '', badge ? [badge.textContent, badge.getAttribute('title')] : null];
  })), cards);

/**
 * The ancestors inside the card that clip what overflows them and come closer than the 4 px
 * focus ring to the focused element: each would cut its ring (review round 1).
 */
const ringClippers = (browser: Browser) => browser.evaluate(() => {
  const focused = document.activeElement!;
  const r = focused.getBoundingClientRect();
  const clippers: string[] = [];
  for (let a = focused.parentElement; a && !a.classList.contains('account-card'); a = a.parentElement) {
    const style = getComputedStyle(a);
    if (style.overflowX === 'visible' && style.overflowY === 'visible') continue;
    const b = a.getBoundingClientRect();
    if (b.top > r.top - 4 || b.bottom < r.bottom + 4 || b.left > r.left - 4 || b.right < r.right + 4) clippers.push(a.className);
  }
  return clippers;
});

/** An Antigravity subscription at its limit, read days ago, with an email: the widest line under a name. */
const connectWidest = (browser: Browser) => browser.evaluate(() => {
  type Account = { provider: string; email: string | null; usage: Record<string, unknown> };
  const db = (window as unknown as { __yuliusMock: Mocked['__yuliusMock'] & { accounts: Account[] } }).__yuliusMock;
  db.connectAntigravity();
  const account = db.accounts[db.accounts.length - 1];
  const soon = new Date(Date.now() + 3600e3).toISOString();
  account.email = 'gemini-team@example.com';
  account.usage = { primary: { used_percent: 100, resets_at: soon, window_minutes: 300 }, secondary: { used_percent: 40, resets_at: soon, window_minutes: 10080 }, observed_at: new Date(Date.now() - 3 * 86400e3).toISOString() };
});

/** Each folded row under `root`: how much of its name, badge and email shows, and whether it overflows. */
const narrowRows = (browser: Browser, root: string) => browser.evaluate((css) =>
  [...document.querySelectorAll(`${css} .account-fold`)].map((fold) => {
    const name = fold.querySelector('.account-name strong')!;
    const badge = fold.querySelector('.agent-badge')!;
    const email = fold.querySelector('.account-subline > .account-sub:not(.agent-badge)');
    return {
      label: name.textContent,
      nameShown: name.getBoundingClientRect().width,
      nameFull: name.scrollWidth,
      badgeWhole: badge.scrollWidth <= badge.clientWidth,
      badgeInRow: badge.getBoundingClientRect().right <= fold.querySelector('summary')!.getBoundingClientRect().right,
      email: email ? email.getBoundingClientRect().width : null,
      emailTitle: email ? email.getAttribute('title') : null,
      overflows: fold.scrollWidth > fold.clientWidth,
    };
  }), root);

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
  expect(await axeViolations(browser)).toEqual([]);
});

test('the Status panel names each subscription\'s agent, for administrators and for members', async ({ app, browser }) => {
  await app.open(mock('/s/studio/git', '&ambient=0'));
  await connectOthers(browser);
  await browser.locator('.btn-status').click();
  await expect(browser.locator('.status-pop .account-fold:has-text("Antigravity")')).toBeVisible();
  const admin = await badges(browser, '.status-pop .account-fold');
  expect(admin['Claude #1']).toEqual(['Claude', 'Claude Code subscription · Anthropic']);
  expect(admin.Codex).toEqual(['Codex', 'Codex subscription · OpenAI']);
  expect(admin.Antigravity).toEqual(['Antigravity', 'Antigravity subscription · Google Gemini']);
  expect(await axeViolations(browser)).toEqual([]);

  // A member sees the badge alone where the email would be: it names the agent as before.
  await app.open(mock('/s/studio/git', '&ambient=0&role=member'));
  await connectOthers(browser);
  await browser.locator('.btn-status').click();
  await expect(browser.locator('.status-pop .account-fold:has-text("Antigravity")')).toBeVisible();
  const member = await badges(browser, '.status-pop .account-fold');
  expect(member['Claude #2']).toEqual(['Claude', 'Claude Code subscription · Anthropic']);
  expect(member.Codex).toEqual(['Codex', 'Codex subscription · OpenAI']);
  expect(member.Antigravity).toEqual(['Antigravity', 'Antigravity subscription · Google Gemini']);
  await expect(browser.locator('.status-pop .account-fold:has-text("Antigravity") .account-subline')).toHaveText('Antigravity');
});

test('the line under the name leaves the email\'s pencil and field their whole focus ring', async ({ app, browser, screen }) => {
  await app.open(mock('/admin/accounts', '&ambient=0'));
  await firstShown(browser.locator('main .account-card').first());
  const pencil = screen.getByRole('button', 'Change the email of Claude #1');
  await pencil.focus();
  await expect(pencil).toBeFocused();
  expect(await ringClippers(browser)).toEqual([]);
  await pencil.click();
  await expect(screen.getByRole('textbox', 'Email of Claude #1')).toBeFocused();
  expect(await ringClippers(browser)).toEqual([]);
});

test('at phone width the widest badge stays whole beside a state, with the name\'s room and the email\'s start', async ({ app, browser }) => {
  await browser.setViewport({ width: 360, height: 780 });
  await app.open(mock('/s/studio', '&ambient=0'));
  await connectWidest(browser);
  await browser.evaluate(() => { location.hash = '#/s/studio/dashboard'; });
  await expect(browser.locator('.db-card .account-fold:has-text("Antigravity")')).toBeVisible();
  await browser.locator('.btn-status').click();
  await expect(browser.locator('.status-pop .account-fold:has-text("Antigravity")')).toBeVisible();
  for (const root of ['.db-card', '.status-pop']) {
    const rows = await narrowRows(browser, root);
    const widest = rows.find((row) => row.label === 'Antigravity')!;
    expect(widest.emailTitle, root).toBe('gemini-team@example.com · Antigravity account');
    for (const row of rows) {
      const where = `${root} ${row.label}`;
      expect(row.badgeWhole, where).toBe(true);
      expect(row.badgeInRow, where).toBe(true);
      expect(row.overflows, where).toBe(false);
      expect(row.nameShown, where).toBeGreaterThanOrEqual(Math.min(row.nameFull, 64) - 0.5);
      // At least the address's start and its ellipsis (SUBK-003).
      expect(row.email ?? 0, where).toBeGreaterThanOrEqual(24);
    }
  }
  expect(await axeViolations(browser)).toEqual([]);
});
