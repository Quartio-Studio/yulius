import { test } from '@e2e-dev/web';
import { expect } from 'e2e';
import { axeViolations, failOnPageErrors, mock } from './support.ts';

failOnPageErrors();

// Task #116 (SPEC-137): every stage of tasks, CI and CD measured and shown. The mock's stages are
// in assets/mock.js ("Stages"): a stage table, a daily trend, dev and production delivery figures,
// and the waterfalls of a CI run (ci_s2 failed in its test job), a dev deploy and a task.
// Task #152 (SPEC-161): the Pipeline (stages) is a tab of the Dashboard, at ?section=pipeline.

test('the Pipeline shows delivery figures, a stage trend with p50 and p90, and the stage table', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/dashboard?section=pipeline'));
  await expect(screen.getByRole('heading', { level: 1 })).toHaveText('Dashboard');
  await expect(screen.getByRole('radio', 'Pipeline (stages)')).toHaveAttribute('aria-checked', 'true');
  const delivery = screen.getByRole('region', 'Delivery');
  const dev = delivery.getByRole('group', 'Dev');
  await expect(dev).toContainText(/Deployments\s*2\.6 a week/);
  await expect(dev).toContainText(/Lead time\s*5 h 30 min/);
  await expect(dev).toContainText(/Change failure rate\s*21%/);
  await expect(dev).toContainText(/Time to restore\s*47 min/);
  await expect(delivery.getByRole('group', 'Production')).toContainText('no failure restored');

  // The CI run's trend by default: two lines, labelled and in a legend, and a table of every day.
  const trend = screen.getByRole('region', 'Trend: CI · Run');
  await expect(trend.getByRole('img', /CI · Run, p50 and p90 by day/)).toBeVisible();
  await expect(trend).toContainText('p50 (median)');
  await expect(trend).toContainText('p90 (9 in 10 within)');
  await trend.getByRole('button', 'Table').click();
  await expect(browser.locator('.db-table tbody tr')).toHaveCount(30);
  const stages = screen.getByRole('region', 'Stages');
  await expect(stages).toContainText('3 reused CI results not counted');
  await expect(stages).toContainText('backfilled');
  expect(await axeViolations(browser)).toEqual([]);

  // A stage of the table becomes the trend, and the choice stays in the address.
  await stages.getByRole('button', 'Task · Needs you').click();
  await expect(screen.getByRole('region', 'Trend: Task · Needs you')).toBeVisible();
  await expect(browser).toHaveURL(/#\/s\/studio\/dashboard\?section=pipeline&trend=task%3Astate%3Aneeds_you$/);
});

// SPEC-161 DPL-001..003: no sidebar entry; the Dashboard's tabs, like the CI/CD page's.
test('the Pipeline is the second tab of the Dashboard, not a sidebar entry', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/dashboard'));
  const nav = screen.getByRole('navigation', 'Sandbox');
  await expect(nav.getByRole('link', 'Pipeline (stages)')).toBeHidden();
  await expect(nav.getByRole('link', 'Dashboard')).toHaveAttribute('aria-current', 'page');
  await expect(screen.getByRole('radiogroup', 'Section')).toBeVisible();
  await expect(screen.getByRole('radio', 'Overview')).toHaveAttribute('aria-checked', 'true');
  await expect(screen.getByRole('link', 'Open Pipeline')).toBeVisible();

  // A click shows the Pipeline in place, with its own header actions; the address names the tab.
  await screen.getByRole('radio', 'Pipeline (stages)').click();
  await expect(screen.getByRole('radio', 'Pipeline (stages)')).toHaveAttribute('aria-checked', 'true');
  await expect(screen.getByRole('region', 'Delivery')).toBeVisible();
  await expect(screen.getByRole('combobox', 'What')).toBeVisible();
  await expect(screen.getByRole('link', 'Open Pipeline')).toBeHidden();
  await expect(browser.locator('.page-sub')).toHaveText('how long every stage of tasks, CI and CD takes');
  await expect(browser).toHaveURL(/#\/s\/studio\/dashboard\?section=pipeline$/);
  await expect(nav.getByRole('link', 'Dashboard')).toHaveAttribute('aria-current', 'page');
  expect(await axeViolations(browser)).toEqual([]);

  // Its state follows the tab in the address, and a reload keeps both.
  await screen.getByRole('radiogroup', 'Date range').getByText('7 days').click();
  await expect(browser).toHaveURL(/#\/s\/studio\/dashboard\?section=pipeline&days=7$/);
  await browser.reload();
  await expect(screen.getByRole('radio', 'Pipeline (stages)')).toHaveAttribute('aria-checked', 'true');
  await expect(screen.getByRole('radiogroup', 'Date range').getByRole('radio', '7 days')).toBeChecked();

  // An arrow key goes back to the Overview and keeps the focus on the tabs.
  await screen.getByRole('radio', 'Pipeline (stages)').press('ArrowLeft');
  await expect(screen.getByRole('radio', 'Overview')).toHaveAttribute('aria-checked', 'true');
  await expect(screen.getByRole('radio', 'Overview')).toBeFocused();
  await expect(screen.getByRole('link', 'Open Pipeline')).toBeVisible();
  await expect(screen.getByRole('region', 'Delivery')).toBeHidden();
  await expect(browser.locator('.page-sub')).toHaveText('what the tasks of this sandbox did, and what it took');
  await expect(browser).toHaveURL(/#\/s\/studio\/dashboard$/);
});

// SPEC-161 DPL-004: the Pipeline page's own address, in older links, opens the tab with its state.
test('the earlier Pipeline address opens the Dashboard\'s Pipeline tab', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/pipeline'));
  await expect(screen.getByRole('heading', { level: 1 })).toHaveText('Dashboard');
  await expect(screen.getByRole('radio', 'Pipeline (stages)')).toHaveAttribute('aria-checked', 'true');
  await expect(screen.getByRole('region', 'Delivery')).toBeVisible();
  await expect(browser).toHaveURL(/#\/s\/studio\/dashboard\?section=pipeline$/);
  await expect(screen.getByRole('navigation', 'Sandbox').getByRole('link', 'Dashboard')).toHaveAttribute('aria-current', 'page');

  // With its state: the filters come with it, after the tab in the canonical address.
  await app.open(mock('/s/studio/pipeline?days=7&subject=cd_run&environment=prod'));
  await expect(screen.getByRole('combobox', 'Environment')).toHaveValue('prod');
  await expect(screen.getByRole('radiogroup', 'Date range').getByRole('radio', '7 days')).toBeChecked();
  await expect(browser).toHaveURL(/#\/s\/studio\/dashboard\?section=pipeline&days=7&subject=cd_run&environment=prod$/);
});

test('filters narrow the stages and stay in the address', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/pipeline?days=7'));
  await expect(browser).toHaveURL(/#\/s\/studio\/dashboard\?section=pipeline&days=7$/);
  await expect(screen.getByRole('radio', '7 days')).toBeChecked();
  await screen.getByRole('combobox', 'What').selectOption('CD runs');
  await expect(browser).toHaveURL(/subject=cd_run/);
  const stages = screen.getByRole('region', 'Stages');
  await expect(stages.getByRole('button', 'CD · Deploy')).toBeVisible();
  await expect(stages.getByRole('button', 'CI · Run')).toBeHidden();
  await screen.getByRole('combobox', 'Environment').selectOption('Production');
  await expect(stages.getByRole('button', 'CD · Confirmation')).toBeVisible();
  await expect(stages.getByRole('button', 'CD · Deploy')).toBeHidden();
  await app.open(mock('/s/studio/pipeline?days=7&subject=cd_run&environment=prod'));
  await expect(screen.getByRole('combobox', 'Environment')).toHaveValue('prod');
});

test('a CI run, a deploy and a task each show a waterfall of their stages', async ({ app, browser, screen }) => {
  // A CI run's dialog: its queue, preparation and jobs with the stages their output named.
  await app.open(mock('/s/studio/ci?run=ci_s2'));
  const run = screen.getByRole('region', 'Stages of this run');
  await expect(run).toContainText('Queue');
  await expect(run).toContainText('Job quality');
  await expect(run).toContainText('lint');
  await expect(run).toContainText('unit');
  // The failed job (and so the run) is marked with a word, not colour alone.
  await expect(browser.locator('.st-row:has-text("Job test (2/2)")')).toHaveAttribute('title', /: .+, failed/);
  await expect(browser.locator('.st-row:has-text("Job test (2/2)") [aria-label="failed"]')).toBeVisible();
  // Its mark sits beside its name, which stays readable (the mark once covered the name).
  const nameWidth = await browser.evaluate(() => {
    const row = [...document.querySelectorAll('.st-row')].find((r) => r.textContent?.includes('Job test (2/2)'));
    return row?.querySelector('.st-name-text')?.getBoundingClientRect().width ?? 0;
  });
  expect(nameWidth).toBeGreaterThan(60);
  await expect(browser.locator('.st-row:has-text("unit")')).toHaveAttribute('title', /1,234 passed, 2 flaky, 3 skipped/);
  await run.getByRole('button', 'Table').click();
  await expect(run.getByRole('table')).toContainText('Job test (2/2)');
  expect(await axeViolations(browser)).toEqual([]);

  // The same run on the Dashboard's Pipeline tab.
  await screen.getByRole('link', 'In Pipeline').click();
  await expect(browser).toHaveURL(/#\/s\/studio\/dashboard\?section=pipeline&open=ci_run%3Aci_s2$/);
  await expect(screen.getByRole('radio', 'Pipeline (stages)')).toHaveAttribute('aria-checked', 'true');
  await expect(screen.getByRole('region', 'CI run stages')).toContainText('Job quality');

  // A deploy's checks, run, deploy and the stages of its output.
  await app.open(mock('/s/studio/cd?run=cd_s2'));
  const deploy = screen.getByRole('region', 'Stages of this deploy');
  for (const text of ['Checks', 'Deployment', 'Deploy', 'build', 'upload']) await expect(deploy).toContainText(text);
  await screen.getByRole('dialog', 'Deploy').getByRole('link', 'In Pipeline').click();
  await expect(browser).toHaveURL(/#\/s\/studio\/dashboard\?section=pipeline&open=cd_run%3Acd_s2$/);
  await expect(screen.getByRole('region', 'Deployment stages')).toBeVisible();

  // A task's states, steps, delivery and its pull request's CI runs.
  await app.open(mock('/s/studio/tasks/12'));
  const task = screen.getByRole('region', 'Stages of this task');
  for (const text of ['Queued', 'Needs you', 'Spec written and validated', 'Pull request open', 'Armed merge, waiting for checks', 'CI runs of its pull request']) {
    await expect(task).toContainText(text);
  }
  expect(await axeViolations(browser)).toEqual([]);
  await screen.getByRole('link', 'In Pipeline').click();
  await expect(browser).toHaveURL(/#\/s\/studio\/dashboard\?section=pipeline&open=task%3A/);
  await expect(screen.getByRole('region', 'Task stages')).toBeVisible();
});

test('the Dashboard shows the pipeline overview and opens the Pipeline tab', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/dashboard'));
  const pipeline = browser.locator('.db-card:has-text("Pipeline")');
  await expect(pipeline).toContainText(/Deployments a week\s*0\.9 · 2\.6/);
  await expect(pipeline).toContainText(/Lead time\s*2 d 2 h/);
  await expect(pipeline).toContainText(/Change failure rate\s*17%/);
  await expect(pipeline).toContainText(/CI run\s*14 min/);
  await screen.getByRole('link', 'Open Pipeline').click();
  await expect(screen.getByRole('radio', 'Pipeline (stages)')).toHaveAttribute('aria-checked', 'true');
  await expect(screen.getByRole('region', 'Delivery')).toBeVisible();
  await expect(browser).toHaveURL(/#\/s\/studio\/dashboard\?section=pipeline$/);

  // The card keeps the Overview's range (SPEC-161 review round 1).
  await app.open(mock('/s/studio/dashboard?days=7'));
  await screen.getByRole('link', 'Open Pipeline').click();
  await expect(browser).toHaveURL(/#\/s\/studio\/dashboard\?section=pipeline&days=7$/);
  await expect(screen.getByRole('radiogroup', 'Date range').getByRole('radio', '7 days')).toBeChecked();
});

// SPEC-161 DPL-002 (review round 1): the tab left stops its refreshes (the Overview's every 30 s,
// the Pipeline's every 5 minutes), and so does the page left. The pages' live refreshes (their
// callback reloads quietly; the sidebar's clock is another 30 s one) are counted from here on.
test('a tab left behind and the page left stop their refreshes', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/tasks'));
  await browser.evaluate(() => {
    const w = window as unknown as Record<string, unknown> & typeof window;
    const live = new Map<unknown, unknown>();
    const set = window.setInterval.bind(window);
    const clear = window.clearInterval.bind(window);
    w.setInterval = ((fn: TimerHandler, ms?: number, ...rest: unknown[]) => {
      const id = set(fn, ms, ...rest);
      if (String(fn).includes('reload({ quiet: true })')) live.set(id, ms);
      return id;
    }) as typeof window.setInterval;
    w.clearInterval = ((id?: number) => { live.delete(id); clear(id); }) as typeof window.clearInterval;
    w.__liveIntervals = (ms: number) => [...live.values()].filter((v) => v === ms).length;
  });
  const live = (ms: number) => browser.evaluate((m) => (window as unknown as { __liveIntervals: (n: number) => number }).__liveIntervals(m), ms);
  const nav = screen.getByRole('navigation', 'Sandbox');

  await nav.getByRole('link', 'Dashboard').click();
  await expect(screen.getByRole('radio', 'Overview')).toHaveAttribute('aria-checked', 'true');
  await expect.poll(() => live(30_000)).toBe(1);
  await expect.poll(() => live(300_000)).toBe(0);

  await screen.getByRole('radio', 'Pipeline (stages)').click();
  await expect(screen.getByRole('region', 'Delivery')).toBeVisible();
  await expect.poll(() => live(30_000)).toBe(0);
  await expect.poll(() => live(300_000)).toBe(1);

  await screen.getByRole('radio', 'Overview').click();
  await expect.poll(() => live(300_000)).toBe(0);
  await expect.poll(() => live(30_000)).toBe(1);

  await screen.getByRole('radio', 'Pipeline (stages)').click();
  await nav.getByRole('link', 'Tasks').click();
  await expect(screen.getByRole('heading', { level: 1 })).toHaveText('Tasks');
  await expect.poll(() => live(30_000)).toBe(0);
  await expect.poll(() => live(300_000)).toBe(0);
});
