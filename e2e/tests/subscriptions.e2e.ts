import { type Browser, type Screen, test } from '@e2e-dev/web';
import { expect } from 'e2e';
import { axeViolations, failOnPageErrors, mock } from './support.ts';

failOnPageErrors();

// Claude subscriptions as one pool (SPEC-001 ROT-009, ROT-010, ROT-013; F 2026-10-10).

test('the subscriptions page shows the pool capacity and nothing to manage but add and remove', async ({ app, browser, screen }) => {
  await app.open(mock('/admin/accounts'));
  // One name for one thing (#147): the page, the menu and Settings say "Subscriptions".
  await expect(screen.getByRole('heading', { level: 1 })).toHaveText('Subscriptions');
  const capacity = screen.getByRole('region', 'Capacity');
  await expect(capacity.getByText(/Add about 1 subscription to stop pausing/)).toBeVisible();
  for (const figure of ['Available now', 'Left this week', 'Burn rate', 'Runs out', 'Tokens (7 days)', 'API cost (7 days)', 'Paused (7 days)']) {
    await expect(capacity.getByText(figure)).toBeVisible();
  }
  // Token use and its cost at API list prices, each with a 7-day chart (SPEC-110 TUS-006, TUS-010).
  await expect(capacity.getByText(/^≈ \$[\d,.]+$/)).toBeVisible();
  await expect(capacity.getByText(/at Claude API list prices/)).toBeVisible();
  for (const name of ['Tokens', 'API cost', 'Burn rate', 'Paused']) {
    await expect(capacity.getByRole('img', new RegExp(`^${name}, last 7 days: first .+, highest .+, last .+$`))).toBeVisible();
  }
  // Each subscription's observed tokens say what they would cost on the API (TUS-004).
  await expect(screen.getByText(/^[\d.,]+[KMB]? tokens · 9 runs · 8 tasks · ≈ \$[\d,.]+ at API prices$/).first()).toBeVisible();
  await expect(screen.getByRole('button', 'Add subscription')).toBeVisible();
  await expect(screen.getByRole('button', 'Sign in again')).toBeVisible();
  // No priority, ordering or on/off controls remain (only the account list's Remove buttons).
  expect(await browser.evaluate(() => document.querySelectorAll('main input[type=number], main [role=switch]').length)).toBe(0);
  await expect(screen.getByText('Refresh usage')).toBeHidden();
  // A usage reading that failed for now is one quiet sentence, no HTTP codes (F 2026-10-10).
  await expect(screen.getByText('Usage updates again in about 12 min.')).toBeVisible();
  expect(await browser.evaluate(() => (document.querySelector('main') || document.body).innerText.includes('HTTP'))).toBe(false);
  await expect(screen.getByText(/Tasks don.t belong to a subscription/)).toBeVisible();

  expect(await axeViolations(browser)).toEqual([]);
});

test('tasks are subscription-agnostic: no account choice or account in the task view', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/t/12'));
  await browser.locator('.taskview .stepstrip-main [aria-label="Task actions"]').click();
  await expect(screen.getByRole('menuitem', /^Stop/)).toBeVisible();
  await expect(screen.getByRole('menuitem', /Switch account/)).toBeHidden();
  await browser.keyboard.press('Escape');
  // The subscription the task runs on shows only in Task details.
  await expect(browser.locator('.taskview .stepstrip-main')).not.toHaveText(/Claude #2/);
});

test('a task paused because no subscription has capacity left says so with the reset time, outside Needs you', async ({ app, screen }) => {
  await app.open(mock('/s/studio/t/12'));
  await expect(screen.getByRole('link', /Search indexing\s*, Paused: no subscription has capacity left \(resets ~[^)]+\)/)).toBeVisible();
});

test('the dashboard shows the subscriptions of the whole instance', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/dashboard'));
  await expect(screen.getByRole('heading', { level: 1 })).toHaveText('Dashboard');
  await expect(screen.getByRole('heading', 'Subscriptions')).toBeVisible();
  await expect(screen.getByText(/Add about 1 subscription to stop pausing/)).toBeVisible();

  expect(await axeViolations(browser)).toEqual([]);
});

test('an unknown cost reads as unknown, never as $0 (unit, in the page)', async ({ app, browser }) => {
  await app.open(mock('/s/studio'));
  const text = await browser.evaluate(async () => {
    const url = '/assets/layout.js';
    const { capacityPanel } = await import(url);
    const spend = (cost) => ({ input: 10, output: 5, cache_read: 0, cache_write: 0, cost_usd: cost });
    // The week had a priced run; the last 24 hours only a run of a model without a price.
    const panel = capacityPanel({ subscriptions: 1, tokens: { last_24h: spend(null), last_7d: spend(1.5), per_day: [spend(1.5), spend(null)], unknown_usage_runs: 0, unpriced_runs: 1 } });
    return panel.textContent;
  });
  expect(text).toContain('≈ $1.50');
  expect(text).toContain('last 24 h: —');
  expect(text).not.toContain('$0.00');
});

/**
 * Each folded subscription under `root`: its name and how much of it shows, its state badge's text,
 * the glance's lines and whether its bars show.
 */
const glances = (browser: Browser, root: string) => browser.evaluate((css) =>
  [...document.querySelectorAll(`${css} .account-fold`)].map((fold) => ({
    label: fold.querySelector('.account-name strong')?.textContent ?? null,
    nameShown: fold.querySelector('.account-name strong')!.getBoundingClientRect().width,
    nameFull: fold.querySelector('.account-name strong')!.scrollWidth,
    nameTitle: fold.querySelector('.account-name strong')!.getAttribute('title'),
    state: fold.querySelector('.account-state')?.textContent ?? null,
    open: (fold as HTMLDetailsElement).open,
    lines: [...fold.querySelectorAll('summary .account-glance .glance-line')].map((line) => {
      const meter = line.querySelector('[role="meter"]') as HTMLElement | null;
      return {
        name: line.querySelector('.glance-name')?.textContent ?? null,
        pct: line.querySelector('.usage-pct')?.textContent ?? null,
        meter: meter?.getAttribute('aria-label') ?? null,
        bar: Boolean(meter && meter.getClientRects().length),
      };
    }),
    overflows: fold.scrollWidth > fold.clientWidth,
  })), root);

test('a folded subscription shows its 5-hour and weekly usage without opening (SPEC-172)', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/dashboard'));
  await expect(screen.getByRole('heading', 'Subscriptions')).toBeVisible();
  await expect(browser.locator('.db-card .account-fold').first()).toBeVisible();
  const rows = await glances(browser, '.db-card');
  expect(rows.length).toBeGreaterThan(3);
  // GLN-001, GLN-002: both windows, named, with the percent and a named meter, every row.
  for (const row of rows) {
    expect(row.lines.map((l) => [l.name, l.meter, l.bar])).toEqual([['5 h', '5-hour window', true], ['Week', 'This week', true]]);
  }
  expect(rows.find((r) => r.label === 'Claude #1')!.lines.map((l) => l.pct)).toEqual(['42%', '19%']);
  // A subscription without a weekly reading says so, never 0 %.
  expect(rows.find((r) => r.label === 'Claude spare')!.lines.map((l) => l.pct)).toEqual(['1%', '—']);
  // GLN-003: opened, the rows below say it with reset times and the glance hides.
  await browser.locator('.db-card .account-fold:has-text("Claude #1") > summary').click();
  await expect(browser.locator('.db-card .account-fold:has-text("Claude #1") .account-glance')).toBeHidden();
  await expect(browser.locator('.db-card .account-fold:has-text("Claude #1") .usage-row:has-text("This week")')).toBeVisible();

  expect(await axeViolations(browser)).toEqual([]);
});

/** GLN-004: the name keeps at least 64 px (or all of itself), with its full text on hover. */
const expectNameRoom = (row: { label: string | null; nameShown: number; nameFull: number; nameTitle: string | null }) => {
  expect(row.nameShown, `${row.label} shows ${row.nameShown} of ${row.nameFull} px`).toBeGreaterThanOrEqual(Math.min(row.nameFull, 64) - 0.5);
  expect(row.nameTitle).toBe(row.label);
};

test('the desktop Status panel shows both windows with their bars, and names keep room beside a state (SPEC-172)', async ({ app, browser }) => {
  await app.open(mock('/s/studio/git'));
  await browser.locator('.btn-status').click();
  await expect(browser.locator('.status-pop .account-fold').first()).toBeVisible();
  const rows = await glances(browser, '.status-pop');
  expect(rows.length).toBeGreaterThan(3);
  // A subscription that needs attention opens by itself, its rows instead of the glance (GLN-003).
  for (const row of rows) {
    expect(row.lines.map((l) => [l.name, l.meter, l.bar])).toEqual([['5 h', '5-hour window', !row.open], ['Week', 'This week', !row.open]]);
    expectNameRoom(row);
  }
  // The state still reads in full, also where only its icon shows.
  expect(rows.find((r) => r.label === 'Claude #5')!.state).toBe('Limit reached');

  expect(await axeViolations(browser)).toEqual([]);
});

test('at phone width the Status panel and the Dashboard keep both percents and drop the glance bars (SPEC-172)', async ({ app, browser }) => {
  await browser.setViewport({ width: 360, height: 780 });
  await app.open(mock('/s/studio/dashboard'));
  await expect(browser.locator('.db-card .account-fold').first()).toBeVisible();
  await browser.locator('.btn-status').click();
  await expect(browser.locator('.status-pop .account-fold').first()).toBeVisible();
  for (const root of ['.status-pop', '.db-card']) {
    const rows = await glances(browser, root);
    expect(rows.length).toBeGreaterThan(3);
    // GLN-004: names and percents stay, bars go, nothing wider than its row.
    for (const row of rows) {
      expect(row.lines.map((l) => [l.name, l.bar])).toEqual([['5 h', false], ['Week', false]]);
      expect(row.lines.every((l) => l.pct === '—' || /^\d+%$/.test(l.pct ?? ''))).toBe(true);
      expect(row.overflows).toBe(false);
      expectNameRoom(row);
    }
  }

  expect(await axeViolations(browser)).toEqual([]);
});

/**
 * Each Status section head: its widths, its summary's text and its lines. `narrow` first squeezes
 * the summary of that section to 60 px, in the same script, so no refresh of the panel comes between.
 */
const sectionHeads = (browser: Browser, narrow: string | null = null) => browser.evaluate((narrow) => {
  if (narrow) {
    const squeezed = [...document.querySelectorAll<HTMLElement>('.status-pop .status-section')]
      .find((section) => section.querySelector('h2')?.textContent === narrow)!.querySelector<HTMLElement>('.status-summary-text')!;
    squeezed.style.maxWidth = '60px';
  }
  const panel = document.querySelector('.status-pop')!.getBoundingClientRect();
  return [...document.querySelectorAll('.status-pop .status-section')].map((section) => {
    const head = section.querySelector(':scope > summary');
    const text = head?.querySelector('.status-summary-text');
    const parts = [...(text?.querySelectorAll('.status-summary-part') ?? [])];
    return {
      title: section.querySelector('h2')?.textContent ?? null,
      wider: [section, head, text].filter((el): el is Element => Boolean(el))
        .filter((el) => el.scrollWidth > el.clientWidth).map((el) => el.className || el.localName),
      textRight: text ? text.getBoundingClientRect().right : null,
      panelRight: panel.right,
      summary: text?.textContent ?? null,
      // A part split over two lines has two rects; the lines are the distinct tops of all rects.
      splitParts: parts.filter((p) => p.getClientRects().length > 1).map((p) => p.textContent),
      lines: new Set(parts.flatMap((p) => [...p.getClientRects()].map((r) => Math.round(r.top)))).size,
      // How far each line ends short of the summary's right edge (its last part's right end).
      lineGaps: [...Map.groupBy(parts, (p) => Math.round(p.getBoundingClientRect().top)).values()]
        .map((line) => text!.getBoundingClientRect().right - Math.max(...line.map((p) => p.getBoundingClientRect().right))),
    };
  });
}, narrow);

test('at phone width every Status section head fits the panel and shows its whole summary (#549)', async ({ app, browser }) => {
  await browser.setViewport({ width: 360, height: 780 });
  await app.open(mock('/s/studio/git'));
  await browser.locator('.btn-status').click();
  await expect(browser.locator('.status-pop .account-fold').first()).toBeVisible();
  const heads = await sectionHeads(browser);
  expect(heads.map((h) => h.title)).toEqual(['Machine', 'Sandboxes', 'Subscriptions']);
  // SSF-001: nothing in a head is wider than it is shown, and no summary ends past the panel.
  for (const head of heads) {
    expect(head.wider, `${head.title} head`).toEqual([]);
    if (head.textRight !== null) expect(head.textRight, `${head.title} summary's right edge`).toBeLessThanOrEqual(head.panelRight);
  }
  // SSF-002: the longest summary wraps between its parts, every word shown.
  const subscriptions = heads.find((h) => h.title === 'Subscriptions')!;
  expect(subscriptions.summary).toBe('4 ready · 1 at the limit · 1 need attention');
  expect(subscriptions.lines).toBeGreaterThan(1);
  expect(subscriptions.splitParts).toEqual([]);
  // Each line ends at the summary's right edge, under the one before (right-aligned).
  for (const gap of subscriptions.lineGaps) expect(Math.abs(gap)).toBeLessThanOrEqual(1);

  // SSF-003
  expect(await axeViolations(browser)).toEqual([]);

  // SSF-002: a summary of one text, squeezed, wraps between its words instead of spilling.
  const sandboxes = (await sectionHeads(browser, 'Sandboxes')).find((h) => h.title === 'Sandboxes')!;
  expect(sandboxes.lines).toBeGreaterThan(1);
  expect(sandboxes.wider).toEqual([]);
});

test('at desktop width a Status section summary stays on one line beside its title (#549)', async ({ app, browser }) => {
  await app.open(mock('/s/studio/git'));
  await browser.locator('.btn-status').click();
  await expect(browser.locator('.status-pop .account-fold').first()).toBeVisible();
  // SSF-004
  for (const head of (await sectionHeads(browser)).filter((h) => h.summary !== null)) {
    expect(head.wider, `${head.title} head`).toEqual([]);
    expect(head.lines, `${head.title} summary lines`).toBe(1);
  }
});

// ---- Claude's sign-in confirms itself (SPEC-243)

type MockLogins = {
  __yuliusMock: {
    loginCancels: string[];
    startLogin: (accountId: string) => string;
    forgetLogins: (accountId: string) => void;
    logins: Map<string, { accountId: string; end: (status: string, code: number | null) => void }>;
    accounts: { id: string }[];
  };
};
type MockAccounts = { __yuliusMock: { accounts: { label: string; email?: string }[] } };

const loginInput = (browser: Browser) => browser.locator('.login-terminal .xterm-helper-textarea');

/** Opens "Add subscription", names it and continues to Claude's sign-in, ready for keys. */
async function addSubscription(browser: Browser, screen: Screen, label: string) {
  await screen.getByRole('button', 'Add subscription').first().click();
  const add = screen.getByRole('dialog', 'Add a Claude subscription');
  await add.getByRole('textbox', /^Name/).fill(label);
  await add.getByRole('button', 'Continue to sign-in').click();
  const dialog = screen.getByRole('dialog', `Sign in · ${label}`);
  await expect(dialog.getByText(/This window closes by itself once the subscription is connected/)).toBeVisible();
  await expect(loginInput(browser)).toBeFocused();
  return dialog;
}

/** Types the authorization code into the sign-in terminal and submits it. */
async function enterCode(browser: Browser, code: string) {
  await expect(loginInput(browser)).toBeFocused();
  await browser.keyboard.type(code);
  await browser.keyboard.press('Enter');
}

test('a finished Claude sign-in closes its dialog by itself and says the subscription is connected (SPEC-243 LGN-003)', async ({ app, browser, screen }) => {
  await app.open(mock('/admin/accounts', '&ambient=0'));
  const dialog = await addSubscription(browser, screen, 'Claude #9');
  await enterCode(browser, 'code-from-claude');
  await expect(dialog).toBeHidden();
  // The email of the Claude account arrives just after the sign-in (SPEC-244) and is named.
  await expect(browser.locator('.toast-ok .toast-msg:has-text("Claude #9 is connected as signed-in@example.com")')).toBeVisible();
  // The new subscription's card reads as signed in: no "Sign in again" on it.
  await expect(browser.locator('main .card:has-text("Claude #9")')).toBeVisible();
  await expect(browser.locator('main .card:has-text("Claude #9") button:has-text("Sign in again")')).toHaveCount(0);
});

test('a failed Claude sign-in says why and Try again signs in in the same dialog (SPEC-243 LGN-004, LGN-005)', async ({ app, browser, screen }) => {
  await app.open(mock('/admin/accounts', '&ambient=0'));
  const dialog = await addSubscription(browser, screen, 'Claude #10');
  await enterCode(browser, 'fail');
  const outcome = dialog.getByRole('alert');
  await expect(outcome).toContainText('Sign-in failed');
  await expect(outcome).toContainText('The sign-in ended without producing a token');
  await expect(dialog.getByRole('button', 'Try again')).toBeVisible();
  await expect(dialog.getByRole('button', 'Cancel sign-in')).toBeHidden();
  expect(await axeViolations(browser)).toEqual([]);

  await dialog.getByRole('button', 'Try again').click();
  await expect(outcome).toBeHidden();
  await expect(dialog.getByRole('button', 'Cancel sign-in')).toBeVisible();
  // The toast names the email the account view reports (UI-022, SPEC-244).
  await browser.evaluate(() => {
    const account = (window as unknown as MockAccounts).__yuliusMock.accounts.find((a) => a.label === 'Claude #10');
    if (account) account.email = 'pawel@example.com';
  });
  await enterCode(browser, 'second-code');
  await expect(dialog).toBeHidden();
  await expect(browser.locator('.toast-ok .toast-msg:has-text("Claude #10 is connected as pawel@example.com")')).toBeVisible();
});

test('closing a running Claude sign-in cancels it and says the subscription is not signed in (SPEC-243 LGN-006)', async ({ app, browser, screen }) => {
  await app.open(mock('/admin/accounts', '&ambient=0'));
  const dialog = await addSubscription(browser, screen, 'Claude #11');
  await dialog.getByRole('button', 'Cancel sign-in').click();
  await expect(dialog).toBeHidden();
  await expect(browser.locator('.toast-msg:has-text("Sign-in cancelled: Claude #11 is not signed in")')).toBeVisible();
  expect(await browser.evaluate(() => (window as unknown as MockLogins).__yuliusMock.loginCancels.length)).toBe(1);
  await expect(browser.locator('main .card:has-text("Claude #11") button:has-text("Sign in again")')).toBeVisible();
});

test('a Claude sign-in replaced elsewhere says it was cancelled, with Try again (SPEC-243 LGN-007)', async ({ app, browser, screen }) => {
  await app.open(mock('/admin/accounts', '&ambient=0'));
  await browser.locator('main .card:has-text("Claude #3") button:has-text("Sign in again")').click();
  const dialog = screen.getByRole('dialog', 'Sign in · Claude #3');
  await expect(loginInput(browser)).toBeFocused();
  await browser.evaluate(() => (window as unknown as MockLogins).__yuliusMock.startLogin('acc_c3'));
  await expect(dialog.getByRole('alert')).toContainText('This sign-in was cancelled');
  await expect(dialog.getByRole('button', 'Try again')).toBeVisible();
  // Closed after it ended, nothing is cancelled again.
  await dialog.getByRole('button', 'Close').last().click();
  await expect(dialog).toBeHidden();
  expect(await browser.evaluate(() => (window as unknown as MockLogins).__yuliusMock.loginCancels.length)).toBe(0);
});

test('a Claude sign-in whose subscription was removed says so, without Try again (SPEC-243 LGN-007)', async ({ app, browser, screen }) => {
  await app.open(mock('/admin/accounts', '&ambient=0'));
  await browser.locator('main .card:has-text("Claude #3") button:has-text("Sign in again")').click();
  const dialog = screen.getByRole('dialog', 'Sign in · Claude #3');
  await expect(loginInput(browser)).toBeFocused();
  // Another administrator removes the subscription: the server cancels its sign-in.
  await browser.evaluate(() => {
    const db = (window as unknown as MockLogins).__yuliusMock;
    db.accounts = db.accounts.filter((a) => a.id !== 'acc_c3');
    for (const session of db.logins.values()) if (session.accountId === 'acc_c3') session.end('cancelled', null);
  });
  await expect(dialog.getByRole('alert')).toContainText('The subscription was removed.');
  await expect(dialog.getByRole('button', 'Try again')).toBeHidden();
  await expect(dialog.getByRole('button', 'Close').last()).toBeVisible();
});

test('a Claude sign-in the server no longer has says so instead of reconnecting (SPEC-243 LGN-007)', async ({ app, browser, screen }) => {
  await app.open(mock('/admin/accounts', '&ambient=0'));
  await browser.locator('main .card:has-text("Claude #3") button:has-text("Sign in again")').click();
  const dialog = screen.getByRole('dialog', 'Sign in · Claude #3');
  await expect(loginInput(browser)).toBeFocused();
  await browser.evaluate(() => (window as unknown as MockLogins).__yuliusMock.forgetLogins('acc_c3'));
  await expect(dialog.getByRole('alert')).toContainText('This sign-in is no longer available');
  await expect(dialog.getByRole('button', 'Cancel sign-in')).toBeHidden();
  // Try again starts a new sign-in, which works.
  await dialog.getByRole('button', 'Try again').click();
  await enterCode(browser, 'code-again');
  await expect(dialog).toBeHidden();
  await expect(browser.locator('.toast-ok .toast-msg:has-text("Claude #3 is connected")')).toBeVisible();
});
