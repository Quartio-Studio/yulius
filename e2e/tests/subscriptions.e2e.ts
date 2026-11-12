import { type Browser, type Screen, test } from '@e2e-dev/web';
import { expect } from 'e2e';
import { axeViolations, failOnPageErrors, firstShown, mock } from './support.ts';

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
  // Reading the usage again is offered for all and for each signed-in one (SPEC-473).
  await expect(screen.getByRole('button', 'Refresh all')).toBeVisible();
  await expect(screen.getByRole('button', 'Refresh usage of Claude #1')).toBeVisible();
  // A usage reading that failed for now is one quiet sentence, no HTTP codes (F 2026-10-10).
  await expect(screen.getByText('Usage updates again in about 12 min.')).toBeVisible();
  expect(await browser.evaluate(() => (document.querySelector('main') || document.body).innerText.includes('HTTP'))).toBe(false);
  await expect(screen.getByText(/Tasks don.t belong to a subscription/)).toBeVisible();

  expect(await axeViolations(browser)).toEqual([]);
});

test('a subscription out of its model\'s usage credits shows that model\'s week beside its own (SPEC-464)', async ({ app, browser, screen }) => {
  await app.open(mock('/admin/accounts'));
  // The limit is the model's: its weekly allowance is used up until it resets, while the 5-hour
  // and weekly windows keep their own readings.
  const card = '.card:has-text("studio@example.com")';
  await expect(browser.locator(`${card} .account-state`)).toHaveText('Limit reached');
  const fable = browser.locator(`${card} .usage-row:has-text("Fable this week")`);
  await expect(fable).toContainText('100% used');
  await expect(fable).toContainText(/Resets \S+ \d/);
  await expect(browser.locator(`${card} .usage-row:has-text("This week")`).first()).toContainText('71% used');
  await expect(screen.getByRole('meter', 'Fable this week')).toHaveAttribute('aria-valuetext', '100% used');
  // A subscription Claude reported no such limit for has no such row.
  await expect(browser.locator('.card:has-text("mkarp@example.com") .usage-row:has-text("Fable this week")')).toHaveCount(0);
  expect(await axeViolations(browser)).toEqual([]);
});

/**
 * What the card of the subscription `named` (a text on it) says, top down: its state, the
 * reading's age, the used-up windows and the rows. `cards` selects the cards to look among.
 */
const cardSays = (browser: Browser, cards: string, named: string) => browser.evaluate(([css, text]) => {
  const el = [...document.querySelectorAll(css)].find((card) => card.textContent!.includes(text))!;
  const reading = el.querySelector('.account-reading');
  return {
    state: el.querySelector('.account-state')?.textContent ?? null,
    // The body's first line: the first thing in the part the clock draws, which heads the body.
    first: el.querySelector('.account-body > :first-child > :first-child')?.className ?? null,
    age: reading?.querySelector('.account-age')?.textContent ?? null,
    stale: reading?.getAttribute('data-stale') ?? null,
    limits: [...el.querySelectorAll('.account-limit')].map((line) => line.textContent),
    rows: [...el.querySelectorAll('.usage-row')].map((row) => [row.querySelector('.usage-name > span')?.textContent, row.querySelector('.usage-reset')?.textContent, row.querySelector('.usage-pct')?.textContent]),
    foot: el.querySelector('.account-foot')?.textContent ?? null,
  };
}, [cards, named]);

// SPEC-473 RFR-009, RFR-010: a card starts with how old its reading is and marks one that may be
// out of date; under it, each used-up window is named with its reset. The foot no longer holds
// the age. Claude spare's own reading by the page is held, so its card stays as old as it is.
test('each card says how old its reading is and which window is used up (SPEC-473)', async ({ app, browser }) => {
  await app.open(mock('/admin/accounts', '&ambient=0&holdpost=/api/admin/accounts/acc_x3/usage'));
  await firstShown(browser.locator('main .account-card').first());
  const fresh = await cardSays(browser, 'main .account-card', 'mkarp@example.com');
  expect(fresh.first).toBe('account-reading');
  expect(fresh.age).toBe('Usage read 2 min ago');
  expect(fresh.stale).toBe('false');
  expect(fresh.limits).toEqual([]);
  expect(fresh.state).toBeNull();
  expect(fresh.foot).toBe('1 task on this subscription');
  const old = await cardSays(browser, 'main .account-card', 'Claude spare');
  expect(old.age).toBe('Usage read 3 d ago · may be out of date');
  expect(old.stale).toBe('true');
  expect(old.foot).toBe('No tasks right now');
  // Out of the default model's allowance: the line names that window and its reset.
  const out = await cardSays(browser, 'main .account-card', 'studio@example.com');
  expect(out.state).toBe('Limit reached');
  expect(out.limits.length).toBe(1);
  expect(out.limits[0]).toMatch(/^Fable this week used up · resets \S+ \d/);
  // One that needs a sign-in and was never read has no reading to date.
  const refused = await cardSays(browser, 'main .account-card', 'ops@example.com');
  expect(refused.age).toBeNull();
  expect(refused.state).toBe('Sign in again');
  expect(await axeViolations(browser)).toEqual([]);
});

// SPEC-473 RFR-009, RFR-011: folded (the Dashboard, the Status panel), a row whose reading may be
// out of date says so in its summary, with words for a screen reader; unfolded, its body starts
// with the age like a card. A window past its reset shows as not read, never as its old percent.
test('a folded subscription marks a reading that may be out of date, and a window past its reset reads as not read (SPEC-473)', async ({ app, browser }) => {
  await app.open(mock('/s/studio', '&ambient=0'));
  await browser.evaluate(() => {
    type Account = { label: string; usage: Record<string, unknown> };
    const db = (window as unknown as { __yuliusMock: { accounts: Account[] } }).__yuliusMock;
    const four = db.accounts.find((a) => a.label === 'Claude #4')!;
    four.usage = { ...four.usage, primary: { used_percent: 100, resets_at: new Date(Date.now() - 60e3).toISOString(), window_minutes: 300 } };
    location.hash = '#/s/studio/dashboard';
  });
  await expect(browser.locator('.db-card .account-fold').first()).toBeVisible();
  const marks = await browser.evaluate(() => Object.fromEntries([...document.querySelectorAll('.db-card .account-fold')].map((fold) => {
    const mark = fold.querySelector('summary .account-stale');
    return [fold.querySelector('.account-name strong')!.textContent, mark ? [mark.textContent, mark.getAttribute('title'), Boolean(mark.querySelector('svg'))] : null];
  })));
  expect(marks['Claude spare']).toEqual(['Usage read 3 d ago, may be out of date', 'Usage read 3 d ago, may be out of date', true]);
  expect(marks['Claude #1']).toBeNull();
  expect(marks['Claude #3']).toBeNull();
  const rows = await glances(browser, '.db-card');
  expect(rows.find((r) => r.label === 'Claude #4')!.lines.map((l) => l.pct)).toEqual(['—', '33%']);
  expect(rows.find((r) => r.label === 'Claude #4')!.state).toBeNull();
  await browser.locator('.db-card .account-fold:has-text("Claude spare") > summary').click();
  const open = await cardSays(browser, '.db-card .account-fold', 'Claude spare');
  expect(open.first).toBe('account-reading');
  expect(open.age).toBe('Usage read 3 d ago · may be out of date');
  await expect(browser.locator('.db-card .account-fold:has-text("Claude spare") summary .account-stale')).toBeHidden();
  await browser.locator('.db-card .account-fold:has-text("Claude #4") > summary').click();
  const reset = await cardSays(browser, '.db-card .account-fold', 'Claude #4');
  expect(reset.rows[0]).toEqual(['5-hour window', 'Reset since this reading', '—']);
  expect(reset.limits).toEqual([]);
  expect(await axeViolations(browser)).toEqual([]);
});

/**
 * Opens the Subscriptions page with Claude #4 stored `ok`, read just now, its 5-hour window at
 * 100 % and resetting in ten minutes: at its limit until then, by the clock alone.
 */
async function openWithAWindowThatResetsInTenMinutes(app: { open: (path: string) => Promise<void> }, browser: Browser) {
  await app.open(mock('/s/studio', '&ambient=0'));
  await browser.evaluate(() => {
    type Account = { label: string; status: string; usage: Record<string, unknown> };
    const db = (window as unknown as { __yuliusMock: { accounts: Account[] } }).__yuliusMock;
    const four = db.accounts.find((a) => a.label === 'Claude #4')!;
    Object.assign(four, { status: 'ok', usage: { ...four.usage, primary: { used_percent: 100, resets_at: new Date(Date.now() + 10 * 60e3).toISOString(), window_minutes: 300 }, observed_at: new Date().toISOString() } });
    location.hash = '#/admin/accounts';
  });
  await firstShown(browser.locator('main .account-card').first());
}

/**
 * Eleven minutes pass on the page's clock (`Date.now`, which the cards read each second): past
 * the reset above, with no test waiting on the wall clock.
 */
const elevenMinutesPass = (browser: Browser) => browser.evaluate(() => {
  const real = Date.now.bind(Date);
  Date.now = () => real() + 11 * 60e3;
});

const FOUR = 'main .card:has-text("pawel@example.com") .account-card';
type Sent = { __yuliusMock: { sent?: Record<string, number>; asked?: Record<string, number> } };

// SPEC-473 RFR-008, RFR-012: the card follows the clock. A subscription at 100 % of its 5-hour
// window reads "Limit reached" though its last reading stored `ok`; the moment the window resets
// the card stops saying so, with no reading and no list read; the row then says the window has
// reset since.
test('a card stops saying Limit reached the moment its window resets, with no new reading (SPEC-473)', async ({ app, browser }) => {
  await openWithAWindowThatResetsInTenMinutes(app, browser);
  // A card that has nothing new to say is not drawn again by the clock: its line stays the
  // very element it was (marked here, looked at once the clock has gone on). Claude #3 was
  // never read, so nothing on its card depends on the time.
  await browser.evaluate(() => {
    const card = [...document.querySelectorAll('main .account-card')].find((el) => el.textContent!.includes('ops@example.com'))!;
    (card.querySelector('.account-error') as HTMLElement & { marked?: boolean }).marked = true;
  });
  const before = await cardSays(browser, 'main .account-card', 'pawel@example.com');
  expect(before.state).toBe('Limit reached');
  expect(before.limits.length).toBe(1);
  expect(before.limits[0]).toMatch(/^5-hour window used up · resets at \d/);
  expect(before.rows[0].slice(0, 1).concat(before.rows[0].slice(2))).toEqual(['5-hour window', '100% used']);
  expect(before.age).toBe('Usage read just now');
  expect(await axeViolations(browser)).toEqual([]);
  await elevenMinutesPass(browser);
  await expect(browser.locator(`${FOUR} .account-state`)).toBeHidden();
  const after = await cardSays(browser, 'main .account-card', 'pawel@example.com');
  // The reading aged with the clock too.
  expect(after.age).toBe('Usage read 11 min ago · may be out of date');
  expect(after.limits).toEqual([]);
  expect(after.rows[0]).toEqual(['5-hour window', 'Reset since this reading', '—']);
  expect(after.rows[1][2]).toBe('33% used');
  // Nothing was read meanwhile: no usage request for it, and the list was read once.
  const requests = await browser.evaluate(() => {
    const db = (window as unknown as Sent).__yuliusMock;
    return { usage: (db.sent ?? {})['POST /api/admin/accounts/acc_x1/usage'] ?? 0, list: (db.asked ?? {})['/api/admin/accounts'] ?? 0 };
  });
  expect(requests).toEqual({ usage: 0, list: 1 });
  expect(await browser.evaluate(() => {
    const card = [...document.querySelectorAll('main .account-card')].find((el) => el.textContent!.includes('ops@example.com'))!;
    return (card.querySelector('.account-error') as HTMLElement & { marked?: boolean }).marked ?? false;
  })).toBe(true);
  expect(await axeViolations(browser)).toEqual([]);
});

// SPEC-473 RFR-012: the clock draws only what depends on it. A name being typed stays as it is,
// with the keyboard in its field, while the card's state changes beside it.
test('a name being edited survives the clock drawing its card again (SPEC-473)', async ({ app, browser, screen }) => {
  await openWithAWindowThatResetsInTenMinutes(app, browser);
  await screen.getByRole('button', 'Rename Claude #4').click();
  const name = screen.getByRole('textbox', 'New name for Claude #4');
  await name.fill('Half typed');
  await expect(browser.locator(`${FOUR} .account-state`)).toHaveText('Limit reached');
  await elevenMinutesPass(browser);
  await expect(browser.locator(`${FOUR} .account-state`)).toBeHidden();
  await expect(name).toBeFocused();
  await expect(name).toHaveValue('Half typed');
  await name.press('Escape');
});

// ---- Refresh, for one and for all (SPEC-473 RFR-013 to RFR-016)

type Usage = {
  __yuliusMock: {
    accounts: { id: string; label: string; status: string; usage: Record<string, unknown>; last_error: string | null }[];
    sent?: Record<string, number>;
    asked?: Record<string, number>;
    usageGate?: Promise<void> | null;
    usageAnswers?: Record<string, { fail?: string; note?: boolean; unread?: boolean; usage?: Record<string, unknown>; status?: string; at?: string }>;
    release?: () => void;
    usageInFlight?: number;
    usageMostInFlight?: number;
  };
  __openUsageGate?: () => void;
};

/** Every usage request from now on waits until `openUsageGate`. */
const holdUsage = (browser: Browser) => browser.evaluate(() => {
  const w = window as unknown as Usage;
  w.__yuliusMock.usageGate = new Promise<void>((resolve) => { w.__openUsageGate = resolve; });
});
const openUsageGate = (browser: Browser) => browser.evaluate(() => {
  const w = window as unknown as Usage;
  w.__openUsageGate?.();
  w.__yuliusMock.usageGate = null;
});
/** How often the page asked for the usage of each subscription, by its id. */
const usageRequests = (browser: Browser) => browser.evaluate(() => {
  const sent = (window as unknown as Usage).__yuliusMock.sent ?? {};
  return Object.fromEntries(Object.entries(sent).flatMap(([what, times]) => {
    const id = /^POST \/api\/admin\/accounts\/([^/]+)\/usage$/.exec(what)?.[1];
    return id ? [[id, times]] : [];
  }));
});
/** Opens the Subscriptions page and waits until its own reading of the one old card (Claude spare) is over. */
async function openSubscriptions(app: { open: (path: string) => Promise<void> }, browser: Browser) {
  await app.open(mock('/admin/accounts', '&ambient=0'));
  await firstShown(browser.locator('main .account-card').first());
  await expect.poll(async () => (await cardSays(browser, 'main .account-card', 'Claude spare')).age).toBe('Usage read just now');
}
/**
 * How many toasts are on the page at this moment. A check that none is there looks once: one
 * that retries for as long as a toast lives would pass over a toast that came and went.
 */
const toastsNow = (browser: Browser, css = '.toast') => browser.evaluate((css) => document.querySelectorAll(css).length, css);
const refreshOf = (screen: Screen, name: string) => screen.getByRole('button', `Refresh usage of ${name}`);
/** The Refresh button on the card that shows `who`, whatever it reads at the moment. */
const refreshIn = (browser: Browser, who: string) => browser.locator(`main .card:has-text("${who}") .account-refresh`);
/** A press that reaches the button's own handler, also while it is off (`aria-disabled`). */
const press = (browser: Browser, css: string, who = '') => browser.evaluate(([css, who]) => {
  [...document.querySelectorAll<HTMLElement>(css)].find((el) => !who || el.closest('.card')!.textContent!.includes(who))!.click();
}, [css, who]);

test('Refresh reads one subscription again: the button says so and keeps the keyboard, and the card shows the new reading (SPEC-473)', async ({ app, browser, screen }) => {
  await openSubscriptions(app, browser);
  await holdUsage(browser);
  const button = refreshIn(browser, 'mkarp@example.com');
  await expect(refreshOf(screen, 'Claude #1')).toHaveText('Refresh usage of Claude #1');
  // The press is answered at once, not at the clock's next second: read in the same turn.
  const atOnce = await browser.evaluate(() => {
    const card = [...document.querySelectorAll('main .card')].find((el) => el.textContent!.includes('mkarp@example.com'))!;
    card.querySelector<HTMLElement>('.account-refresh')!.focus();
    card.querySelector<HTMLElement>('.account-refresh')!.click();
    return card.querySelector('.account-refresh')!.textContent;
  });
  expect(atOnce).toBe('Reading… usage of Claude #1');
  // It says that it is reading, by its text and by its name, and whose it is.
  await expect(screen.getByRole('button', 'Reading… usage of Claude #1')).toBeVisible();
  await expect(button).toHaveAttribute('aria-busy', 'true');
  // Off without losing the keyboard, and a second press asks nothing more.
  await expect(button).toHaveAttribute('aria-disabled', 'true');
  await expect(button).toBeFocused();
  await press(browser, 'main .account-refresh', 'mkarp@example.com');
  await browser.keyboard.press('Enter');
  expect((await cardSays(browser, 'main .account-card', 'mkarp@example.com')).age).toBe('Usage read 2 min ago');
  expect(await axeViolations(browser)).toEqual([]);
  await openUsageGate(browser);
  await expect.poll(async () => (await cardSays(browser, 'main .account-card', 'mkarp@example.com')).age).toBe('Usage read just now');
  await expect(browser.locator('.toast-ok .toast-msg:has-text("Usage of Claude #1 read again.")')).toBeVisible();
  // Said once: the presses while it was reading started nothing of their own.
  await expect(browser.locator('.toast-ok')).toHaveCount(1);
  await expect(refreshOf(screen, 'Claude #1')).toBeVisible();
  expect(await button.getAttribute('aria-busy')).toBeNull();
  expect(await button.getAttribute('aria-disabled')).toBeNull();
  await expect(button).toBeFocused();
  expect((await usageRequests(browser)).acc_c1).toBe(1);
  // One that needs a sign-in has no Refresh: its card offers the sign-in.
  await expect(refreshOf(screen, 'Claude #3')).toHaveCount(0);
  expect(await axeViolations(browser)).toEqual([]);
});

test('a refresh that Claude does not answer says why in one sentence, on a toast and on the card for the visit (SPEC-473)', async ({ app, browser, screen }) => {
  await openSubscriptions(app, browser);
  const sentence = 'Claude did not answer when asked whether the limit still holds; the limit stays as it was.';
  // The server keeps the sentence as the usage note; here it does not, so what stays on the card
  // is what the page itself keeps for the visit.
  await browser.evaluate((fail) => { (window as unknown as Usage).__yuliusMock.usageAnswers = { acc_x1: { fail, note: false } }; }, sentence);
  const button = refreshOf(screen, 'Claude #4');
  await button.click();
  await expect(browser.locator('.toast-error .toast-msg')).toHaveText(`Couldn’t refresh Claude #4: ${sentence}`);
  const note = browser.locator('main .account-card:has-text("pawel@example.com") .account-note');
  await expect(note).toHaveText(sentence);
  await expect(button).toBeVisible();
  expect(await browser.evaluate(() => (document.querySelector('main') || document.body).innerText.includes('HTTP'))).toBe(false);
  expect((await cardSays(browser, 'main .account-card', 'pawel@example.com')).age).toBe('Usage read 4 min ago');
  // The card was drawn from the list read again, and the sentence is still there.
  await expect.poll(() => browser.evaluate(() => ((window as unknown as Usage).__yuliusMock.asked ?? {})['/api/admin/accounts'])).toBe(2);
  await expect(note).toHaveText(sentence);
  expect(await axeViolations(browser)).toEqual([]);
  // A refresh that works takes it away.
  await browser.evaluate(() => { (window as unknown as Usage).__yuliusMock.usageAnswers = {}; });
  await button.click();
  await expect(note).toHaveCount(0);
  expect((await cardSays(browser, 'main .account-card', 'pawel@example.com')).age).toBe('Usage read just now');
});

test('a refresh that finds the limit gone draws the card ready and reads the capacity again (SPEC-473)', async ({ app, browser, screen }) => {
  await openSubscriptions(app, browser);
  expect((await cardSays(browser, 'main .account-card', 'studio@example.com')).state).toBe('Limit reached');
  // Claude answers that the default model runs: nothing is stored about its allowance any more.
  await browser.evaluate(() => {
    const db = (window as unknown as Usage).__yuliusMock;
    const { weekly_fable: _gone, ...usage } = db.accounts.find((a) => a.id === 'acc_x2')!.usage;
    db.usageAnswers = { acc_x2: { status: 'ok', usage: { ...usage, primary: { used_percent: 12, resets_at: new Date(Date.now() + 3600e3).toISOString(), window_minutes: 300 } } } };
  });
  const capacityReads = () => browser.evaluate(() => ((window as unknown as Usage).__yuliusMock.asked ?? {})['/api/capacity'] ?? 0);
  const before = await capacityReads();
  // Read beside the list, and again after the page's own reading of the old card.
  expect(before).toBe(2);
  await refreshOf(screen, 'Claude #5').click();
  await expect(browser.locator('.toast-ok .toast-msg:has-text("Usage of Claude #5 read again.")')).toBeVisible();
  const after = await cardSays(browser, 'main .account-card', 'studio@example.com');
  expect(after.state).toBeNull();
  expect(after.limits).toEqual([]);
  expect(after.age).toBe('Usage read just now');
  expect(after.rows.map((row) => row[0])).toEqual(['5-hour window', 'This week']);
  await expect.poll(capacityReads).toBe(before + 1);
});

test('a refresh for which Claude gives no usage reading says so on the card (SPEC-473)', async ({ app, browser, screen }) => {
  await openSubscriptions(app, browser);
  await browser.evaluate(() => { (window as unknown as Usage).__yuliusMock.usageAnswers = { acc_c1: { unread: true } }; });
  await refreshOf(screen, 'Claude #1').click();
  const note = browser.locator('main .account-card:has-text("mkarp@example.com") .account-note');
  await expect(note).toHaveText('Claude gave no new usage reading: it shares this subscription’s usage only with its running tasks.');
  await expect(browser.locator('.toast-msg:has-text("Claude gave no new usage reading for Claude #1.")')).toBeVisible();
  expect(await toastsNow(browser, '.toast-error')).toBe(0);
  expect(await toastsNow(browser, '.toast-ok')).toBe(0);
  expect((await cardSays(browser, 'main .account-card', 'mkarp@example.com')).age).toBe('Usage read 2 min ago');
});

// The server writes a reading's time to the second, so two readings may carry the same time.
// The answer itself says whether Claude gave a reading (`usage_read`): a second reading in the
// same second is a reading, not "no new reading".
test('a reading that carries the same time as the one before is still a reading (SPEC-473)', async ({ app, browser, screen }) => {
  await openSubscriptions(app, browser);
  await browser.evaluate(() => {
    const db = (window as unknown as Usage).__yuliusMock;
    db.usageAnswers = { acc_c1: { at: db.accounts.find((a) => a.id === 'acc_c1')!.usage.observed_at as string } };
  });
  await refreshOf(screen, 'Claude #1').click();
  await expect(browser.locator('.toast-ok .toast-msg:has-text("Usage of Claude #1 read again.")')).toBeVisible();
  await expect(browser.locator('main .account-card:has-text("mkarp@example.com") .account-note')).toHaveCount(0);
});

// The same for a subscription that was never read: no reading before, none after.
test('a refresh of a subscription that was never read and still gets no reading says so (SPEC-473)', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio', '&ambient=0'));
  await browser.evaluate(() => {
    const db = (window as unknown as Usage).__yuliusMock;
    const four = db.accounts.find((a) => a.id === 'acc_x1')!;
    four.usage = { primary: null, secondary: null, observed_at: null };
    db.usageAnswers = { acc_x1: { unread: true } };
    const w = window as unknown as Usage;
    w.__yuliusMock.usageGate = new Promise<void>((resolve) => { w.__openUsageGate = resolve; });
    location.hash = '#/admin/accounts';
  });
  await firstShown(browser.locator('main .account-card').first());
  // The card says that the page is reading it from its first look (the reading is held here);
  // then that reading is over, and said nothing.
  expect(await browser.evaluate(() => [...document.querySelectorAll('main .card')].find((el) => el.textContent!.includes('pawel@example.com'))!.querySelector('.account-refresh')!.textContent)).toBe('Reading… usage of Claude #4');
  await openUsageGate(browser);
  await expect(refreshOf(screen, 'Claude #4')).toBeVisible();
  expect((await usageRequests(browser)).acc_x1).toBe(1);
  const note = browser.locator('main .account-card:has-text("pawel@example.com") .account-note');
  await expect(note).toHaveCount(0);
  await refreshOf(screen, 'Claude #4').click();
  await expect(note).toHaveText('Claude gave no new usage reading: it shares this subscription’s usage only with its running tasks.');
  expect(await toastsNow(browser, '.toast-ok')).toBe(0);
  expect((await cardSays(browser, 'main .account-card', 'pawel@example.com')).age).toBe('Usage not read yet');
});

test('Refresh all reads every signed-in subscription, three at a time, and says how it went (SPEC-473)', async ({ app, browser, screen }) => {
  await openSubscriptions(app, browser);
  const capacityBefore = await browser.evaluate(() => {
    const db = (window as unknown as Usage).__yuliusMock;
    db.sent = {};
    db.usageMostInFlight = 0;
    return (db.asked ?? {})['/api/capacity'] ?? 0;
  });
  await holdUsage(browser);
  const all = browser.locator('.page-actions .refresh-all');
  await screen.getByRole('button', 'Refresh all').click();
  // What it does is in its name while it runs too.
  await expect(all).toHaveText('Reading… all subscriptions');
  await expect(all).toHaveAttribute('aria-busy', 'true');
  await expect(all).toHaveAttribute('aria-disabled', 'true');
  await expect(all).toBeFocused();
  // Each card says that it is being read, and three requests are under way, not five.
  await expect(refreshIn(browser, 'mkarp@example.com')).toHaveText('Reading… usage of Claude #1');
  await expect(refreshIn(browser, 'Claude spare')).toHaveText('Reading… usage of Claude spare');
  await expect.poll(() => browser.evaluate(() => (window as unknown as Usage).__yuliusMock.usageInFlight)).toBe(3);
  // A press on a card meanwhile, and on the button again, join what is under way.
  await press(browser, 'main .account-refresh', 'mkarp@example.com');
  await press(browser, '.page-actions .refresh-all');
  expect(await axeViolations(browser)).toEqual([]);
  await openUsageGate(browser);
  await expect(browser.locator('.toast-ok .toast-msg:has-text("Usage read again for 5 subscriptions.")')).toBeVisible();
  // Said once, with nothing else: the presses meanwhile started nothing of their own.
  await expect(browser.locator('.toast-ok')).toHaveCount(1);
  await expect.poll(() => browser.evaluate(() => ((window as unknown as Usage).__yuliusMock.asked ?? {})['/api/capacity'])).toBe(capacityBefore + 1);
  await expect(all).toHaveText('Refresh all');
  expect(await all.getAttribute('aria-disabled')).toBeNull();
  expect(await all.getAttribute('aria-busy')).toBeNull();
  // Each signed-in one once; the one that needs a sign-in not at all.
  expect(await usageRequests(browser)).toEqual({ acc_c1: 1, acc_c2: 1, acc_x1: 1, acc_x2: 1, acc_x3: 1 });
  expect(await browser.evaluate(() => (window as unknown as Usage).__yuliusMock.usageMostInFlight)).toBe(3);
  for (const who of ['mkarp@example.com', 'team@example.com', 'pawel@example.com', 'studio@example.com', 'Claude spare']) {
    expect((await cardSays(browser, 'main .account-card', who)).age, who).toBe('Usage read just now');
  }

  // One of them cannot be read: the toast counts it and its card says why.
  const sentence = 'Claude did not accept this subscription’s sign-in. If its tasks fail too, sign in again.';
  await browser.evaluate((fail) => { (window as unknown as Usage).__yuliusMock.usageAnswers = { acc_c2: { fail } }; }, sentence);
  await all.click();
  await expect(browser.locator('.toast-error .toast-msg:has-text("1 of 5 subscriptions could not be read: each card says why.")')).toBeVisible();
  await expect(browser.locator('main .account-card:has-text("team@example.com") .account-note')).toHaveText(sentence);
  await expect(all).toHaveText('Refresh all');

  // Claude gives no reading for one of them: the toast does not count it as read again.
  await browser.evaluate(() => { (window as unknown as Usage).__yuliusMock.usageAnswers = { acc_x3: { unread: true } }; });
  await all.click();
  await expect(browser.locator('.toast-msg:has-text("Usage read again for 4 of 5 subscriptions: Claude gave no new reading for the rest.")')).toBeVisible();
  await expect(browser.locator('main .account-card:has-text("Claude spare") .account-note')).toContainText('Claude gave no new usage reading');
});

test('Refresh all is off when no subscription is signed in, and one subscription is counted as one (SPEC-473)', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio', '&ambient=0'));
  await browser.evaluate(() => {
    const db = (window as unknown as Usage).__yuliusMock;
    db.accounts.forEach((account, i) => { if (i > 0) account.status = 'relogin'; });
    location.hash = '#/admin/accounts';
  });
  await firstShown(browser.locator('main .account-card').first());
  const all = screen.getByRole('button', 'Refresh all');
  await all.click();
  await expect(browser.locator('.toast-ok .toast-msg:has-text("Usage read again for 1 subscription.")')).toBeVisible();
  expect(await usageRequests(browser)).toEqual({ acc_c1: 1 });
  // No reading for any: said as that, not as "read again for 0".
  await browser.evaluate(() => { (window as unknown as Usage).__yuliusMock.usageAnswers = { acc_c1: { unread: true } }; });
  await all.click();
  await expect(browser.locator('.toast-msg:has-text("Claude gave no new usage reading: each card says so.")')).toBeVisible();
  await expect(browser.locator('.toast-ok')).toHaveCount(1);
  await browser.evaluate(() => { const db = (window as unknown as Usage).__yuliusMock; db.usageAnswers = {}; db.sent = { 'POST /api/admin/accounts/acc_c1/usage': 1 }; });
  // None signed in: the button says so and asks nothing.
  await browser.evaluate(() => {
    const db = (window as unknown as Usage).__yuliusMock;
    db.accounts[0].status = 'relogin';
    location.hash = '#/s/studio';
  });
  await expect(screen.getByRole('heading', { level: 1 })).not.toHaveText('Subscriptions');
  await browser.evaluate(() => { location.hash = '#/admin/accounts'; });
  await firstShown(browser.locator('main .account-card').first());
  await expect(all).toHaveAttribute('aria-disabled', 'true');
  await expect(all).toHaveAttribute('title', 'No subscription is signed in');
  expect(await all.getAttribute('aria-busy')).toBeNull();
  await press(browser, '.page-actions .refresh-all');
  await expect(all).toHaveText('Refresh all');
  expect(await usageRequests(browser)).toEqual({ acc_c1: 1 });
  expect(await axeViolations(browser)).toEqual([]);
});

// RFR-015: the page reads a card that may be out of date by itself once per visit. A redraw of the
// list does not ask again, a failure of that reading says nothing, and a later Refresh does ask.
test('the page reads an old card by itself once per visit and says nothing when that fails (SPEC-473)', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio', '&ambient=0'));
  await browser.evaluate(() => {
    (window as unknown as Usage).__yuliusMock.usageAnswers = { acc_x3: { fail: 'Usage could not be read just now; it updates again in a few minutes.', note: false } };
    location.hash = '#/admin/accounts';
  });
  await firstShown(browser.locator('main .account-card').first());
  await expect.poll(async () => (await usageRequests(browser)).acc_x3).toBe(1);
  // The list is drawn again (a rename elsewhere): the old card is not asked for again. The
  // cards on the page are marked, so the redraw is seen when a card without the mark is there.
  await browser.evaluate(() => { for (const card of document.querySelectorAll('main .account-card')) (card as HTMLElement & { old?: boolean }).old = true; });
  await screen.getByRole('button', 'Rename Claude #4').click();
  await screen.getByRole('textbox', 'New name for Claude #4').fill('Claude four');
  await screen.getByRole('textbox', 'New name for Claude #4').press('Enter');
  await expect(screen.getByText('Subscription renamed to Claude four.')).toBeVisible();
  await expect.poll(() => browser.evaluate(() => {
    const cards = [...document.querySelectorAll('main .account-card')] as (HTMLElement & { old?: boolean })[];
    return cards.length > 0 && cards.every((card) => !card.old);
  }), { message: 'the list was never drawn again' }).toBe(true);
  expect(await usageRequests(browser)).toEqual({ acc_x3: 1 });
  expect(await toastsNow(browser, '.toast-error')).toBe(0);
  await expect(browser.locator('main .account-card:has-text("Claude spare") .account-note')).toHaveCount(0);
  expect((await cardSays(browser, 'main .account-card', 'Claude spare')).age).toBe('Usage read 3 d ago · may be out of date');
  // Asked for by a person, the same failure is said.
  await refreshOf(screen, 'Claude spare').click();
  await expect(browser.locator('.toast-error .toast-msg')).toContainText('Couldn’t refresh Claude spare: Usage could not be read just now');
  expect((await usageRequests(browser)).acc_x3).toBe(2);
});

// RFR-015: Refresh all joins a reading that is already under way (here the page's own reading of
// the old card): one request for that subscription, and since a person now asked for it, what
// comes of it is said on its card. The card's own button is off meanwhile.
test('Refresh all joins a reading the page already has under way, and its card says how it went (SPEC-473)', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio', '&ambient=0'));
  const sentence = 'Usage could not be read just now; it updates again in a few minutes.';
  await browser.evaluate((fail) => {
    const w = window as unknown as Usage;
    w.__yuliusMock.usageAnswers = { acc_x3: { fail, note: false } };
    w.__yuliusMock.usageGate = new Promise<void>((resolve) => { w.__openUsageGate = resolve; });
    location.hash = '#/admin/accounts';
  }, sentence);
  await firstShown(browser.locator('main .account-card').first());
  // The page's own reading of the old card is under way: its button says so and is off.
  await expect(refreshIn(browser, 'Claude spare')).toHaveText('Reading… usage of Claude spare');
  await press(browser, 'main .account-refresh', 'Claude spare');
  await screen.getByRole('button', 'Refresh all').click();
  await expect.poll(() => browser.evaluate(() => (window as unknown as Usage).__yuliusMock.usageInFlight)).toBe(3);
  await openUsageGate(browser);
  await expect(browser.locator('.toast-error .toast-msg')).toHaveText('1 of 5 subscriptions could not be read: each card says why.');
  await expect(browser.locator('main .account-card:has-text("Claude spare") .account-note')).toHaveText(sentence);
  expect((await usageRequests(browser)).acc_x3).toBe(1);
  await expect(refreshOf(screen, 'Claude spare')).toBeVisible();
});

// RFR-015: the queue is dropped with the page. A card's Refresh and Refresh all have three
// readings under way and two waiting when the page is left: the two are never sent, and the
// three that end say nothing on the page the person went to, for the card or for all.
test('leaving the Subscriptions page drops the readings that wait and says nothing of those that end (SPEC-473)', async ({ app, browser, screen }) => {
  await openSubscriptions(app, browser);
  await browser.evaluate(() => { (window as unknown as Usage).__yuliusMock.sent = {}; });
  await holdUsage(browser);
  await refreshOf(screen, 'Claude #1').click();
  await screen.getByRole('button', 'Refresh all').click();
  await expect.poll(() => browser.evaluate(() => (window as unknown as Usage).__yuliusMock.usageInFlight)).toBe(3);
  await browser.evaluate(() => { location.hash = '#/s/studio'; });
  await expect(screen.getByRole('heading', { level: 1 })).not.toHaveText('Subscriptions');
  await openUsageGate(browser);
  // The answers have come and every step that follows them has run.
  await expect.poll(() => browser.evaluate(() => (window as unknown as Usage).__yuliusMock.usageInFlight)).toBe(0);
  await browser.evaluate(() => new Promise((resolve) => { setTimeout(resolve, 0); }));
  expect(await toastsNow(browser, '.toast')).toBe(0);
  expect(await usageRequests(browser)).toEqual({ acc_c1: 1, acc_c2: 1, acc_x1: 1 });
});

// Refresh all does nothing before the list is there and when it could not be read, and says so
// by being off.
test('Refresh all is off until the subscriptions are shown, and when they could not be read (SPEC-473)', async ({ app, browser }) => {
  await app.open(mock('/admin/accounts', '&ambient=0&hold=/api/admin/accounts'));
  const all = browser.locator('.page-actions .refresh-all');
  await expect(all).toHaveAttribute('aria-disabled', 'true');
  expect(await all.getAttribute('title')).toBeNull();
  await press(browser, '.page-actions .refresh-all');
  expect(await toastsNow(browser, '.toast')).toBe(0);
  await browser.evaluate(() => (window as unknown as Usage).__yuliusMock.release?.());
  await firstShown(browser.locator('main .account-card').first());
  await expect.poll(() => all.getAttribute('aria-disabled')).toBeNull();
  expect(await axeViolations(browser)).toEqual([]);

  await app.open(mock('/admin/accounts', '&ambient=0&fail=/api/admin/accounts'));
  await expect(browser.locator('main .load-error')).toBeVisible();
  await expect(all).toHaveAttribute('aria-disabled', 'true');
  expect(await all.getAttribute('title')).toBeNull();
  await press(browser, '.page-actions .refresh-all');
  expect(await usageRequests(browser)).toEqual({});
  expect(await toastsNow(browser, '.toast')).toBe(0);
});

test('only the Subscriptions page offers Refresh: the Dashboard and the Status panel show the same card without it (SPEC-473)', async ({ app, browser }) => {
  await app.open(mock('/s/studio/dashboard', '&ambient=0'));
  await expect(browser.locator('.db-card .account-fold').first()).toBeVisible();
  await browser.locator('.db-card .account-fold:has-text("Claude #1") > summary').click();
  await expect(browser.locator('.db-card .account-fold:has-text("Claude #1") .account-reading')).toHaveText('Usage read 2 min ago');
  await browser.locator('.btn-status').click();
  await expect(browser.locator('.status-pop .account-fold').first()).toBeVisible();
  expect(await browser.evaluate(() => document.querySelectorAll('.account-refresh').length)).toBe(0);
});

test('a task shows its subscription and a person runs it on another (SPEC-447 MOV-001, MOV-004)', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/t/12'));
  const chip = browser.locator('.taskview .stepstrip-main button.account-chip');
  await expect(chip).toHaveText('Claude #2');
  await chip.click();
  await expect(screen.getByText('Run #12 on…')).toBeVisible();
  // Its own is marked, one needing a sign-in says why; the others show their use.
  await expect(screen.getByRole('menuitem', /^Claude #2\s*Runs on it now/)).toHaveAttribute('aria-disabled', 'true');
  await expect(screen.getByRole('menuitem', /^Claude #3\s*Needs a new sign-in/)).toHaveAttribute('aria-disabled', 'true');
  await expect(screen.getByRole('menuitem', /^Claude #4\s*5 h 12% · week 33%/)).toBeVisible();
  // Out of the default model's usage credits only: this task runs Opus, so it can go there (SPEC-464).
  await expect(screen.getByRole('menuitem', /^Claude #5\s*5 h 100% · week 71%/)).toBeVisible();
  expect(await axeViolations(browser)).toEqual([]);
  await screen.getByRole('menuitem', /^Claude #4/).click();
  await expect(screen.getByText('#12 moves to Claude #4…')).toBeVisible();
  await expect(chip).toHaveText('Claude #4');
});

// Review round 2 of SPEC-464: the task's model is in its details, which the task view reads after
// it drew the chip. A person who opens the menu before that read answered (or whose read failed)
// still gets the subscriptions judged for the task's model: the menu reads what it needs itself.
test('"Run #N on…" knows the task\'s model before the task view has its details (SPEC-464)', async ({ app, browser, screen }) => {
  type Holding = { __yuliusMock: { holdDetail?: Promise<void> | null; detailReads?: Record<string, number> }; __releaseDetail?: () => void };
  await app.open(mock('/s/studio', '&ambient=0'));
  // The view's own read of the task waits on its way, as on a slow connection.
  await browser.evaluate(() => {
    const w = window as unknown as Holding;
    w.__yuliusMock.holdDetail = new Promise<void>((resolve) => { w.__releaseDetail = resolve; });
    location.hash = '#/s/studio/t/12';
  });
  await expect.poll(() => browser.evaluate(() => {
    const db = (window as unknown as Holding).__yuliusMock;
    return Boolean(db.detailReads?.tsk_studio_12) && !db.holdDetail;
  }), { message: 'the view never read the task' }).toBe(true);
  const chip = browser.locator('.taskview .stepstrip-main button.account-chip');
  await expect(chip).toHaveText('Claude #2');
  await chip.click();
  await expect(screen.getByText('Run #12 on…')).toBeVisible();
  // Task #12 runs Opus: a subscription that is only out of the default model's usage credits can take it.
  await expect(screen.getByRole('menuitem', /^Claude #5\s*5 h 100% · week 71%/)).toBeVisible();
  await expect(screen.getByRole('menuitem', /^Claude #3\s*Needs a new sign-in/)).toHaveAttribute('aria-disabled', 'true');
  // When the menu cannot read the task either, it still opens, and a task of no known model is
  // not let through on a subscription that reads "Limit reached".
  await browser.keyboard.press('Escape');
  await expect(screen.getByText('Run #12 on…')).toHaveCount(0);
  await browser.evaluate(() => { (window as unknown as { __yuliusMock: { failNextDetail?: boolean } }).__yuliusMock.failNextDetail = true; });
  await chip.click();
  await expect(screen.getByRole('menuitem', /^Claude #5\s*At its usage limit/)).toHaveAttribute('aria-disabled', 'true');
  await expect(screen.getByRole('menuitem', /^Claude #4\s*5 h 12% · week 33%/)).toBeVisible();
  await browser.evaluate(() => (window as unknown as Holding).__releaseDetail?.());
});

test('"Run #N on…" judges a subscription by the windows that bind the task\'s model (unit, in the page; SPEC-464)', async ({ app, browser }) => {
  await app.open(mock('/s/studio'));
  const reasons = await browser.evaluate(async () => {
    const url = '/assets/actions.js';
    const { cannotTake } = await import(url);
    const ahead = new Date(Date.now() + 3600e3).toISOString();
    const past = new Date(Date.now() - 3600e3).toISOString();
    const w = (used, resets_at = ahead) => ({ used_percent: used, resets_at, window_minutes: 10080 });
    // "Limited" because the default model's allowance is used up; its own windows have room.
    const fableOut = { status: 'limited', usage: { primary: w(10), secondary: w(20), weekly_fable: w(100) } };
    const opus = { model: 'claude-opus-5-5' };
    return {
      fableTaskOnFableOut: cannotTake(fableOut, { model: 'claude-fable-5-2' }),
      opusTaskOnFableOut: cannotTake(fableOut, opus),
      unknownModelOnFableOut: cannotTake(fableOut, {}),
      opusTaskOnOpusOut: cannotTake({ status: 'ok', usage: { primary: w(10), weekly_opus: w(100) } }, opus),
      weekUsedUp: cannotTake({ status: 'limited', usage: { secondary: w(100) } }, opus),
      // A limit of the whole subscription binds every model, whatever else is used up beside it.
      limitOfTheSubscription: cannotTake({ status: 'limited', usage: { primary: w(10), limit_reached: true, limit_resets_at: ahead } }, opus),
      limitOfTheSubscriptionAndFableOut: cannotTake({ status: 'limited', usage: { limit_reached: true, limit_resets_at: null, weekly_fable: w(100) } }, opus),
      limitPastItsReset: cannotTake({ status: 'ok', usage: { primary: w(10), limit_reached: true, limit_resets_at: past } }, opus),
      windowPastItsReset: cannotTake({ status: 'ok', usage: { primary: w(100, past) } }, opus),
      needsSignIn: cannotTake({ status: 'relogin', usage: {} }, opus),
      room: cannotTake({ status: 'ok', usage: { primary: w(40), secondary: w(12) } }, { model: 'claude-fable-5-2' }),
      // Stored "limited" at the last reading, and the window that made it so has reset since
      // (SPEC-473 RFR-008): a task of no known model is no longer held back by the stored status.
      storedLimitedPastItsReset: cannotTake({ status: 'limited', usage: { primary: w(10), weekly_fable: w(100, past) } }, {}),
    };
  });
  expect(reasons).toEqual({
    fableTaskOnFableOut: 'At its usage limit',
    opusTaskOnFableOut: null,
    unknownModelOnFableOut: 'At its usage limit',
    opusTaskOnOpusOut: 'At its usage limit',
    weekUsedUp: 'At its usage limit',
    limitOfTheSubscription: 'At its usage limit',
    limitOfTheSubscriptionAndFableOut: 'At its usage limit',
    limitPastItsReset: null,
    windowPastItsReset: null,
    needsSignIn: 'Needs a new sign-in',
    room: null,
    storedLimitedPastItsReset: null,
  });
});

// SPEC-473 RFR-008: one function says what a subscription's stored usage means at a moment: whether
// the default model can run, which windows are used up with their resets, and how old the reading
// is. The stored status is the last reading's; the card, the Status panel and "Run #N on…" go by
// this.
test('what a subscription\'s usage means now is worked out from its windows (unit, in the page; SPEC-473)', async ({ app, browser }) => {
  await app.open(mock('/s/studio'));
  const read = await browser.evaluate(async () => {
    const [{ limitsNow, percentNow, STALE_AFTER_MS }, { resetLabel }] = await Promise.all([import('/assets/usage.js' as string), import('/assets/format.js' as string)]);
    const now = Date.parse('2026-11-12T12:00:00Z');
    const iso = (ms: number) => new Date(now + ms).toISOString();
    const ahead = iso(3 * 3600e3);
    const past = iso(-60e3);
    const resets = (at: string) => { const label = resetLabel(at, now); return `${label[0].toLowerCase()}${label.slice(1)}`; };
    const w = (used: number, resets_at: string | null = ahead) => ({ used_percent: used, resets_at, window_minutes: 10080 });
    const of = (status: string, usage: Record<string, unknown>) => {
      const { state, limited, lines, age, stale } = limitsNow({ status, usage }, now);
      return { state, limited, lines: lines.map((l: { text: string }) => l.text), age, stale };
    };
    return {
      resets: resets(ahead),
      staleAfterMin: STALE_AFTER_MS / 60e3,
      room: of('ok', { primary: w(40), secondary: w(12), observed_at: iso(-2 * 60e3) }),
      // A stored "ok" with a binding window at 100 % is at its limit now (evidence (b)).
      fiveHour: of('ok', { primary: w(100), secondary: w(12), observed_at: iso(-30e3) }),
      week: of('limited', { primary: w(10), secondary: w(100) }),
      fable: of('limited', { primary: w(10), weekly_fable: w(100) }),
      // Another model's window: its line, and the default model still runs here.
      opus: of('ok', { primary: w(10), weekly_opus: w(100), weekly_sonnet: w(100, null) }),
      noReset: of('limited', { weekly_fable: w(100, null) }),
      // A limit of the whole subscription that no window shows, with and without its reset.
      reported: of('limited', { primary: w(10), limit_reached: true, limit_resets_at: ahead }),
      reportedBlind: of('limited', { limit_reached: true, limit_resets_at: null }),
      // Shown by its window: said once.
      reportedByItsWindow: of('limited', { primary: w(100), limit_reached: true, limit_resets_at: ahead }),
      // Stored "limited", and everything that made it so has reset or ended since.
      reset: of('limited', { primary: w(100, past), weekly_fable: w(100, past), limit_reached: true, limit_resets_at: past, observed_at: iso(-11 * 60e3) }),
      several: of('limited', { primary: w(100), secondary: w(100), weekly_fable: w(100) }),
      // A subscription that needs a sign-in keeps saying so, whatever its windows.
      relogin: of('relogin', { primary: w(100) }),
      pending: of('pending_login', {}),
      // The reading's age, to the second it turns stale.
      neverRead: of('ok', {}),
      atTheEdge: of('ok', { observed_at: iso(-STALE_AFTER_MS) }),
      pastTheEdge: of('ok', { observed_at: iso(-STALE_AFTER_MS - 1000) }),
      // A window's percent is not known after its reset.
      percents: [percentNow(w(55), now), percentNow(w(55, past), now), percentNow(w(55, null), now), percentNow(null, now), percentNow({ used_percent: null, resets_at: ahead }, now)],
    };
  });
  const none = { state: 'ok', limited: false, lines: [] as string[] };
  expect(read.staleAfterMin).toBe(10);
  expect(read.resets).toMatch(/^resets at \d/);
  expect(read.room).toEqual({ ...none, age: 'Usage read 2 min ago', stale: false });
  expect(read.fiveHour).toEqual({ state: 'limited', limited: true, lines: [`5-hour window used up · ${read.resets}`], age: 'Usage read just now', stale: false });
  expect(read.week).toEqual({ state: 'limited', limited: true, lines: [`This week used up · ${read.resets}`], age: 'Usage not read yet', stale: false });
  expect(read.fable.lines).toEqual([`Fable this week used up · ${read.resets}`]);
  expect(read.fable.limited).toBe(true);
  expect(read.opus).toEqual({ ...none, lines: [`Opus this week used up · ${read.resets}`, 'Sonnet this week used up · Claude did not say when it resets'], age: 'Usage not read yet', stale: false });
  expect(read.noReset.lines).toEqual(['Fable this week used up · Claude did not say when it resets']);
  expect(read.reported).toEqual({ state: 'limited', limited: true, lines: [`Claude reported a usage limit · ${read.resets}`], age: 'Usage not read yet', stale: false });
  expect(read.reportedBlind.lines).toEqual(['Claude reported a usage limit without saying when it resets']);
  expect(read.reportedBlind.limited).toBe(true);
  expect(read.reportedByItsWindow.lines).toEqual([`5-hour window used up · ${read.resets}`]);
  expect(read.reset).toEqual({ ...none, age: 'Usage read 11 min ago', stale: true });
  expect(read.several.lines).toEqual([`5-hour window used up · ${read.resets}`, `This week used up · ${read.resets}`, `Fable this week used up · ${read.resets}`]);
  expect(read.relogin).toEqual({ state: 'relogin', limited: true, lines: [`5-hour window used up · ${read.resets}`], age: 'Usage not read yet', stale: false });
  expect(read.pending.state).toBe('pending_login');
  expect(read.neverRead).toEqual({ ...none, age: 'Usage not read yet', stale: false });
  expect(read.atTheEdge).toEqual({ ...none, age: 'Usage read 10 min ago', stale: false });
  expect(read.pastTheEdge.stale).toBe(true);
  expect(read.percents).toEqual([55, null, 55, null, null]);
});

// SPEC-473 RFR-008: the Status panel counts and marks a subscription by what its windows say now,
// not by the status stored at its last reading: one at 100 % of its 5-hour window is at the limit
// though it was stored "ok" (the person's evidence), and one whose full window has reset is ready
// though it was stored "limited".
test('the Status panel goes by what the windows say now, not by the stored status (SPEC-473)', async ({ app, browser }) => {
  await app.open(mock('/s/studio/git', '&ambient=0'));
  await browser.evaluate(() => {
    type Account = { label: string; status: string; usage: Record<string, unknown> };
    const db = (window as unknown as { __yuliusMock: { accounts: Account[] } }).__yuliusMock;
    const find = (label: string) => db.accounts.find((a) => a.label === label)!;
    const ahead = new Date(Date.now() + 3600e3).toISOString();
    const past = new Date(Date.now() - 60e3).toISOString();
    const one = find('Claude #1');
    one.usage = { ...one.usage, primary: { used_percent: 100, resets_at: ahead, window_minutes: 300 } };
    const two = find('Claude #2');
    Object.assign(two, { status: 'limited', usage: { ...two.usage, weekly_fable: { used_percent: 100, resets_at: past, window_minutes: 10080 } } });
  });
  await browser.locator('.btn-status').click();
  await expect(browser.locator('.status-pop .account-fold').first()).toBeVisible();
  const rows = await glances(browser, '.status-pop');
  const state = (label: string) => rows.find((r) => r.label === label)!.state;
  expect(state('Claude #1')).toBe('Limit reached');
  expect(state('Claude #2')).toBeNull();
  expect(state('Claude #5')).toBe('Limit reached');
  const summary = (await sectionHeads(browser)).find((h) => h.title === 'Subscriptions')!.summary;
  expect(summary).toBe('3 ready · 2 at the limit · 1 need attention');
  expect(await axeViolations(browser)).toEqual([]);
});

// "Limit reached" also means only the default model's own window is used up (SPEC-464), so the
// status line for a pool whose every subscription reads so does not claim that nothing can run.
test('the status line of a pool at its limits says so without claiming nothing runs (SPEC-464)', async ({ app, browser }) => {
  await app.open(mock('/s/studio', '&ambient=0'));
  await browser.evaluate(() => {
    const db = (window as unknown as { __yuliusMock: { accounts: { status: string; usage: Record<string, unknown> }[] } }).__yuliusMock;
    // Each one's 5-hour window is used up; what their last reading stored as status does not
    // matter (SPEC-473 RFR-008).
    const resets_at = new Date(Date.now() + 3600e3).toISOString();
    db.accounts.forEach((account, i) => Object.assign(account, { status: i % 2 ? 'ok' : 'limited', usage: { primary: { used_percent: 100, resets_at, window_minutes: 300 }, secondary: null, observed_at: new Date().toISOString() } }));
  });
  await browser.locator('.btn-status').click();
  await expect(browser.locator('.status-pop .status-health')).toHaveText('Every subscription is at a usage limit: tasks continue when a window resets');
});

test('the Tasks list says which subscription each task runs on (SPEC-447 MOV-004)', async ({ app, browser }) => {
  await app.open(mock('/s/studio/tasks'));
  await expect(browser.locator('.tl-row:has-text("Checkout API") .tl-status')).toContainText('on Claude #2');
});

test('a refused subscription says why, and one in use is removed with its tasks moved (SPEC-447 MOV-003, MOV-005)', async ({ app, browser, screen }) => {
  await app.open(mock('/admin/accounts'));
  await expect(screen.getByText('Claude refused it: Your organization has disabled Claude subscription access for Claude Code. Sign in again.')).toBeVisible();
  const remove = browser.locator('.card:has-text("team@example.com") button:has-text("Remove")');
  await expect(remove).toBeEnabled();
  await remove.click();
  await expect(screen.getByText(/The 3 tasks on it move to another subscription in the same session, or wait for one\./)).toBeVisible();
  await screen.getByRole('button', 'Remove').last().click();
  await expect(screen.getByText('Subscription Claude #2 removed.')).toBeVisible();
  await app.open(mock('/s/studio/t/12'));
  await expect(browser.locator('.taskview .stepstrip-main button.account-chip')).not.toHaveText('Claude #2');
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

/** The Status panel's Sessions tile: its value, the over-the-limit mark, its detail lines and its title. */
const sessionsTile = (browser: Browser) => browser.evaluate(() => {
  const tile = [...document.querySelectorAll<HTMLElement>('.status-pop .stat-tile')]
    .find((t) => t.querySelector('.stat-head')?.textContent?.trim() === 'Sessions')!;
  const value = tile.querySelector('.tile-value')!;
  const details = [...tile.querySelectorAll<HTMLElement>('.stat-detail')];
  return {
    value: [...value.childNodes].filter((n) => n.nodeType === Node.TEXT_NODE).map((n) => n.textContent).join('').trim(),
    over: value.querySelector('.tile-over')?.textContent ?? null,
    details: details.map((d) => d.textContent),
    // A detail line cut off with an ellipsis (its text wider than it is shown).
    clipped: details.filter((d) => d.scrollWidth > d.clientWidth).map((d) => d.textContent),
    title: tile.getAttribute('title'),
  };
});

test('the Sessions tile shows the limit full and the overshoot, not 7 / 4 (SPEC-334)', async ({ app, browser }) => {
  // 7 running mock tasks against a limit of 4, as resumed Done tasks can take it (SES-101, #928);
  // 1 queued task and 5 more.
  await app.open(mock('/s/studio/git', '&ambient=0&sessionlimit=4&sessionqueue=5'));
  await browser.locator('.btn-status').click();
  await expect(browser.locator('.status-pop .stat-tile').first()).toBeVisible();
  const over = await sessionsTile(browser);
  // OVL-001
  expect(over.value).toBe('4 / 4');
  expect(over.over).toBe('+3');
  expect(over.details).toEqual(['3 over the limit', '6 queued']);
  // OVL-002: why, without naming one cause (a restart with a lower limit keeps its runs too).
  expect(over.title).toBe('3 sessions more than the limit of 4: a Done task that works again counts again, and runs started before a restart with a lower limit keep running. Queued tasks start once fewer than 4 run.');
  expect(await axeViolations(browser)).toEqual([]);

  // At phone width both detail lines are shown whole.
  await browser.setViewport({ width: 360, height: 780 });
  expect((await sessionsTile(browser)).clipped).toEqual([]);

  // OVL-003: within the limit the tile reads as before.
  await app.open(mock('/s/studio/git', '&ambient=0&sessionlimit=20'));
  await browser.locator('.btn-status').click();
  await expect(browser.locator('.status-pop .stat-tile').first()).toBeVisible();
  const within = await sessionsTile(browser);
  expect(within.value).toBe('7 / 20');
  expect(within.over).toBeNull();
  expect(within.details).toEqual(['1 queued']);
  expect(within.title).toBeNull();
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

test('a sign-in of a subscription whose email was typed does not say Claude confirmed it (SPEC-463 EML-006)', async ({ app, browser, screen }) => {
  await app.open(mock('/admin/accounts', '&ambient=0'));
  await firstShown(browser.locator('main .account-card').first());
  // Claude #3 needs a new sign-in, and an administrator types its email first.
  await screen.getByRole('button', 'Change the email of Claude #3').click();
  await screen.getByRole('textbox', 'Email of Claude #3').fill('agent@example.com');
  await screen.getByRole('textbox', 'Email of Claude #3').press('Enter');
  await expect(screen.getByText('Email of Claude #3 is now agent@example.com.')).toBeVisible();
  await browser.locator('main .card:has-text("Claude #3") button:has-text("Sign in again")').click();
  const dialog = screen.getByRole('dialog', 'Sign in · Claude #3');
  await enterCode(browser, 'code-from-claude');
  await expect(dialog).toBeHidden();
  // The toast names an email only when Claude gave it; the typed one stays on the card.
  await expect(browser.locator('.toast-ok .toast-msg:has-text("Claude #3 is connected")')).toHaveText('Claude #3 is connected. Tasks can use it now.');
  await expect(browser.locator('main .account-card:has-text("Claude #3") .account-sub')).toHaveText('agent@example.com');
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
