import { test } from '@e2e-dev/web';
import { expect } from 'e2e';
import { axeViolations, failOnPageErrors, firstShown, mock } from './support.ts';

failOnPageErrors();

type MockWindow = Window & { __yuliusMock: { analyticsSummary?: object; failAnalyticsSummary?: boolean } };

test('Harnesses compares workload and elapsed times, with token coverage beside usage', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/dashboard'));
  const tab = screen.getByRole('radio', 'Harnesses');
  await firstShown(tab);
  await tab.click();
  await expect(screen.getByRole('heading', 'Run performance')).toBeVisible();
  const performance = (selector: string) => browser.locator(`table[aria-label="Run performance"] ${selector}`);
  const usage = (selector: string) => browser.locator(`table[aria-label="Token usage"] ${selector}`);
  await expect(performance('tbody th')).toHaveText(['Antigravity', 'Claude Code', 'Codex']);
  await expect(performance('tr:has(th:text-is("Codex")) td').last()).toHaveText('45 min');
  await expect(usage('tr:has(th:text-is("Antigravity")) td').last()).toHaveText('—');
  const before = await performance('tr:has(th:text-is("Claude Code")) td').first().textContent();
  await screen.getByRole('radiogroup', 'Date range').getByText('7 days').click();
  await expect(browser).toHaveURL(/section=harnesses&days=7$/);
  await expect(performance('tr:has(th:text-is("Claude Code")) td').first()).not.toHaveText(before ?? '');
  await browser.reload();
  await firstShown(screen.getByRole('heading', 'Run performance'));
  await expect(screen.getByRole('radio', '7 days')).toBeChecked();
  expect(await axeViolations(browser)).toEqual([]);
});

test('Harnesses handles task-only and run-only groups, missing usage, capped and empty cohorts', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/dashboard'));
  await browser.evaluate(() => {
    Object.assign((window as MockWindow).__yuliusMock, { analyticsSummary: {
      tasks: { total: 2, by_harness: { codex: 2 } },
      tokens_by_harness: [{ harness: 'antigravity', runs: 3, finished_runs: 0,
        median_wall_ms: null, p90_wall_ms: null, input: 0, output: null, cache_read: null,
        cache_write: null, unknown_usage_runs: 2 }], truncated: true,
    } });
  });
  await screen.getByRole('radio', 'Harnesses').click();
  await expect(screen.getByText('Showing the newest 2 matching tasks.')).toBeVisible();
  const rows = (title: string, harness: string) => browser.locator(`table[aria-label="${title}"] tr:has(th:text-is("${harness}")) td`);
  await expect(rows('Run performance', 'Antigravity')).toHaveText(['0', '3', '0', '—', '—']);
  await expect(rows('Run performance', 'Codex')).toHaveText(['2', '0', '0', '—', '—']);
  await expect(rows('Token usage', 'Antigravity')).toHaveText(['1 of 3', '0', '—', '—', '—']);
  await browser.evaluate(() => { (window as MockWindow).__yuliusMock.analyticsSummary = { tasks: { total: 0, by_harness: {} }, tokens_by_harness: [] }; });
  await screen.getByRole('button', 'Refresh').click();
  await expect(screen.getByText('No tasks were created in this period.')).toBeVisible();
});

test('a failed comparison can be retried on a narrow screen', async ({ app, browser, screen }) => {
  await browser.setViewport({ width: 360, height: 800 });
  await app.open(mock('/s/studio/dashboard'));
  const tab = screen.getByRole('radio', 'Harnesses');
  await firstShown(tab);
  await browser.evaluate(() => { (window as MockWindow).__yuliusMock.failAnalyticsSummary = true; });
  await tab.click();
  await expect(screen.getByRole('alert')).toContainText('Couldn’t load the harness comparison');
  await browser.evaluate(() => { (window as MockWindow).__yuliusMock.failAnalyticsSummary = false; });
  await screen.getByRole('button', 'Try again').click();
  await expect(screen.getByRole('table', 'Token usage')).toBeVisible();
  expect(await browser.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(360);
});
