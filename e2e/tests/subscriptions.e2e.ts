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
  await expect(screen.getByText('Refresh usage')).toBeHidden();
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
  });
});

// "Limit reached" also means only the default model's own window is used up (SPEC-464), so the
// status line for a pool whose every subscription reads so does not claim that nothing can run.
test('the status line of a pool at its limits says so without claiming nothing runs (SPEC-464)', async ({ app, browser }) => {
  await app.open(mock('/s/studio', '&ambient=0'));
  await browser.evaluate(() => {
    const db = (window as unknown as { __yuliusMock: { accounts: { status: string }[] } }).__yuliusMock;
    for (const account of db.accounts) account.status = 'limited';
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
