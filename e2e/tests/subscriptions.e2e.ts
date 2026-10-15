import { type Browser, test } from '@e2e-dev/web';
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

/** Each folded subscription under `root`: its name, the glance's lines and whether its bars show. */
const glances = (browser: Browser, root: string) => browser.evaluate((css) =>
  [...document.querySelectorAll(`${css} .account-fold`)].map((fold) => ({
    label: fold.querySelector('.account-name strong')?.textContent ?? null,
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

test('at phone width the Status panel keeps both percents and drops the glance bars (SPEC-172)', async ({ app, browser }) => {
  await browser.setViewport({ width: 360, height: 780 });
  await app.open(mock('/s/studio/git'));
  await browser.locator('.btn-status').click();
  await expect(browser.locator('.status-pop .account-fold').first()).toBeVisible();
  const rows = await glances(browser, '.status-pop');
  expect(rows.length).toBeGreaterThan(3);
  // GLN-004: names and percents stay, bars go, nothing wider than its row.
  for (const row of rows) {
    expect(row.lines.map((l) => [l.name, l.bar])).toEqual([['5 h', false], ['Week', false]]);
    expect(row.lines.every((l) => l.pct === '—' || /^\d+%$/.test(l.pct ?? ''))).toBe(true);
    expect(row.overflows).toBe(false);
  }

  expect(await axeViolations(browser)).toEqual([]);
});
