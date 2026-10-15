import { test, type Browser } from '@e2e-dev/web';
import { expect } from 'e2e';
import { axeViolations, failOnPageErrors, mock } from './support.ts';

failOnPageErrors();

// Task #116 (SPEC-137): every stage of tasks, CI and CD measured and shown. The mock's stages are
// in assets/mock.js ("Stages"): a stage table, a daily trend, dev and production delivery figures,
// and the waterfalls of a CI run (ci_s2 failed in its test job), a dev deploy and a task.

test('the Pipeline page shows delivery figures, a stage trend with p50 and p90, and the stage table', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/pipeline'));
  await expect(screen.getByRole('heading', { level: 1 })).toHaveText('Pipeline');
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
  // #488 (SPEC-154): a job's CPU time and peak memory, p50 · p90, beside its duration; a stage
  // that measured none shows a dash, never 0.
  await expect(stages.getByRole('columnheader', 'CPU time p50 · p90')).toBeVisible();
  await expect(stages.getByRole('columnheader', 'Peak memory p50 · p90')).toBeVisible();
  const quality = '.pl-table tbody tr:has-text("CI · Job quality")';
  await expect(browser.locator(`${quality} td.pl-cpu`)).toHaveText('31 min · 52 min');
  await expect(browser.locator(`${quality} td.pl-memory`)).toHaveText('2.0 GiB · 3.5 GiB');
  const queue = '.pl-table tbody tr:has-text("CI · Queue: waiting for a lane")';
  await expect(browser.locator(`${queue} td.pl-cpu`)).toHaveText('—');
  await expect(browser.locator(`${queue} td.pl-memory`)).toHaveText('—');
  expect(await axeViolations(browser)).toEqual([]);

  // A stage of the table becomes the trend, and the choice stays in the address.
  await stages.getByRole('button', 'Task · Needs you').click();
  await expect(screen.getByRole('region', 'Trend: Task · Needs you')).toBeVisible();
  await expect(browser).toHaveURL(/#\/s\/studio\/pipeline\?trend=task%3Astate%3Aneeds_you$/);
});

test('filters narrow the stages and stay in the address', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/pipeline?days=7'));
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
  await expect(run).toContainText('quality');
  // SPEC-165: a passed job's stages are folded under it until it is opened.
  await browser.locator('.st-row:has-text("quality") .st-fold').click();
  await expect(run).toContainText('lint');
  await expect(run).toContainText('unit');
  // The failed job (and so the run) is marked with a word, not colour alone.
  await expect(browser.locator('.st-row:has-text("test (2/2)")')).toHaveAttribute('title', /: .+, failed/);
  await expect(browser.locator('.st-row:has-text("test (2/2)") [aria-label="failed"]')).toBeVisible();
  // Its mark sits beside its name, which stays readable (the mark once covered the name).
  const name = await browser.evaluate(() => {
    const text = [...document.querySelectorAll('.st-row')].find((r) => r.textContent?.includes('test (2/2)'))?.querySelector('.st-name-text');
    return { width: text?.getBoundingClientRect().width ?? 0, cut: text ? text.scrollWidth > text.clientWidth : true };
  });
  expect(name.width).toBeGreaterThan(40);
  expect(name.cut).toBe(false);
  await expect(browser.locator('.st-row:has-text("unit")')).toHaveAttribute('title', /1,234 passed, 2 flaky, 3 skipped/);
  // #488: a job's own CPU time, peak memory and bytes written, on hover and in its title.
  const qualityRow = browser.locator('.st-row.st-job:has-text("quality")');
  await expect(qualityRow).toHaveAttribute('title', /CPU 31 min · peak 2\.0 GiB · wrote 640 MiB/);
  await qualityRow.hover();
  await expect(browser.locator('.st-tip')).toContainText('CPU 31 min · peak 2.0 GiB · wrote 640 MiB');
  await run.getByRole('button', 'Table').click();
  await expect(run.getByRole('table')).toContainText('test (2/2)');
  expect(await axeViolations(browser)).toEqual([]);

  // The same run on the Pipeline page.
  await screen.getByRole('link', 'In Pipeline').click();
  await expect(browser).toHaveURL(/#\/s\/studio\/pipeline\?open=ci_run%3Aci_s2$/);
  await expect(screen.getByRole('region', 'CI run stages')).toContainText('quality');

  // A deploy's checks, run, deploy and the stages of its output.
  await app.open(mock('/s/studio/cd?run=cd_s2'));
  const deploy = screen.getByRole('region', 'Stages of this deploy');
  for (const text of ['Checks', 'Deployment', 'Deploy', 'build', 'upload']) await expect(deploy).toContainText(text);

  // A task's states, steps, delivery and its pull request's CI runs.
  await app.open(mock('/s/studio/tasks/12'));
  const task = screen.getByRole('region', 'Stages of this task');
  for (const text of ['Queued', 'Needs you', 'Spec written and validated', 'Pull request open', 'Armed merge, waiting for checks', 'CI runs of its pull request']) {
    await expect(task).toContainText(text);
  }
  expect(await axeViolations(browser)).toEqual([]);
});

// Task #155 (SPEC-165): the screenshot's busy run (mock ci_s4) read as one row per job.

/** The rows under the "Jobs" heading: name, the duration column, and whether wait and run touch. */
function jobRows(browser: Browser) {
  return browser.evaluate(() => [...document.querySelectorAll('.st-row.st-job')].map((row) => {
    const wait = row.querySelector('.st-bar-wait')?.getBoundingClientRect();
    const bar = row.querySelector('.st-bar:not(.st-bar-wait)')?.getBoundingClientRect();
    return {
      name: row.querySelector('.st-name-text')?.textContent ?? null,
      took: row.querySelector('.st-took')?.textContent ?? null,
      touch: wait && bar ? Math.abs(wait.right - bar.left) <= 1 : null,
    };
  }));
}

test('a busy CI run reads one row per job: its wait for a slot and its run on one bar', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/ci?run=ci_s4'));
  const run = screen.getByRole('region', 'Stages of this run');
  await expect(run).toContainText('Jobs');
  expect(await browser.evaluate(() => [...document.querySelectorAll('.st-waterfall .st-group')].map((g) => g.textContent))).toEqual(['Run', 'Jobs']);
  // No anonymous "Waiting for a job slot" row: each wait is on its job's row, in waiting order.
  await expect(browser.locator('.st-row:has-text("Waiting for a job slot")')).toHaveCount(0);
  const rows = await jobRows(browser);
  expect(rows.map((r) => r.name)).toEqual(['shared', 'engine', 'prototypes', 'package (gateway)', 'package (broker)', 'package (deployer)', 'package (mcp/browser)', 'package (mcp/email)']);
  expect(rows.every((r) => r.touch === true)).toBe(true);
  const engine = rows.find((r) => r.name === 'engine');
  expect(engine?.took).toMatch(/^3 min so far\s*waited 6 min$/);
  const engineRow = browser.locator('.st-row.st-job:has-text("engine")');
  await expect(engineRow).toHaveAttribute('title', /^engine: waited 6 min for a job slot, ran 3 min so far, running/);
  await expect(browser.locator('.st-row.st-job:has-text("engine") [aria-label="running"]')).toBeVisible();
  await expect(browser.locator('.st-row.st-job:has-text("shared") [aria-label="passed"]')).toBeVisible();
  await engineRow.hover();
  await expect(browser.locator('.st-tip')).toContainText('waited 6 min for a job slot');
  // The table keeps the wait as a column.
  await run.getByRole('button', 'Table').click();
  await expect(run.getByRole('columnheader', 'Waited')).toBeVisible();
  await expect(browser.locator('.st-table tr:has-text("engine") td.st-waited')).toHaveText('6 min');
  expect(await axeViolations(browser)).toEqual([]);
});

test('a job’s stages fold under it, a failed job opens, and the fold outlives a refresh', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/ci?run=ci_s4'));
  await expect(screen.getByRole('region', 'Stages of this run')).toContainText('engine');
  await expect(browser.locator('.st-row:has-text("Checkout")')).toHaveCount(0);
  const fold = browser.locator('.st-row.st-job:has-text("engine") .st-fold');
  await expect(fold).toHaveAttribute('aria-expanded', 'false');
  await expect(fold).toContainText('2 stages');
  await fold.click();
  await expect(fold).toHaveAttribute('aria-expanded', 'true');
  await expect(browser.locator('.st-row:has-text("Checkout")')).toHaveCount(1);
  await expect(browser.locator('.st-row:has-text("build")')).toBeVisible();
  // The running run's dialog redraws its stages every few seconds: what was opened stays open.
  await browser.evaluate(() => { document.querySelector('.st-waterfall')?.setAttribute('data-probe', 'old'); });
  await expect(browser.locator('.st-waterfall[data-probe]')).toHaveCount(0);
  await expect(browser.locator('.st-row.st-job:has-text("engine") .st-fold')).toHaveAttribute('aria-expanded', 'true');
  await expect(browser.locator('.st-row:has-text("Checkout")')).toHaveCount(1);

  // A failed job opens by itself, on the stage that failed; a passed one stays folded.
  await app.open(mock('/s/studio/ci?run=ci_s2'));
  await expect(screen.getByRole('region', 'Stages of this run')).toContainText('test (2/2)');
  await expect(browser.locator('.st-row.st-job:has-text("test (2/2)") .st-fold')).toHaveAttribute('aria-expanded', 'true');
  await expect(browser.locator('.st-row:has-text("integration") [aria-label="failed"]')).toBeVisible();
  await expect(browser.locator('.st-row.st-job:has-text("quality") .st-fold')).toHaveAttribute('aria-expanded', 'false');
  await expect(browser.locator('.st-row:has-text("lint")')).toHaveCount(0);
});

test('every waterfall has a time axis from 0 and a legend of its bars', async ({ app, browser, screen }) => {
  const axis = () => browser.evaluate(() => [...document.querySelectorAll('.st-waterfall .st-tick')].map((t) => t.textContent));
  const legend = () => browser.evaluate(() => [...document.querySelectorAll('.st-waterfall .st-bars-legend li')].map((t) => t.textContent));
  await app.open(mock('/s/studio/ci?run=ci_s4'));
  await expect(screen.getByRole('region', 'Stages of this run')).toContainText('engine');
  expect(await axis()).toEqual(['0', '2 min', '4 min', '6 min', '8 min', 'now']);
  expect(await legend()).toEqual(['Waiting', 'Ran', 'Still running']);

  await app.open(mock('/s/studio/ci?run=ci_s2'));
  await expect(screen.getByRole('region', 'Stages of this run')).toContainText('quality');
  expect((await axis())[0]).toBe('0');
  expect(await axis()).not.toContain('now');
  expect(await legend()).toEqual(['Waiting', 'Ran', 'Failed']);

  // A deploy and a task get the same axis and legend; a deploy's stages fold under it.
  await app.open(mock('/s/studio/cd?run=cd_s2'));
  const deploy = screen.getByRole('region', 'Stages of this deploy');
  await expect(deploy).toContainText('Deploy');
  expect((await axis())[0]).toBe('0');
  expect(await legend()).toEqual(['Ran', 'Failed']);
  await browser.setViewport({ width: 390, height: 844 });
  await app.open(mock('/s/studio/tasks/12'));
  await expect(screen.getByRole('region', 'Stages of this task')).toContainText('Needs you');
  expect((await axis())[0]).toBe('0');
  expect(await browser.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(390);
  expect(await axeViolations(browser)).toEqual([]);
});

test('the Dashboard shows the pipeline overview and opens the Pipeline page', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/dashboard'));
  const pipeline = browser.locator('.db-card:has-text("Pipeline")');
  await expect(pipeline).toContainText(/Deployments a week\s*0\.9 · 2\.6/);
  await expect(pipeline).toContainText(/Lead time\s*2 d 2 h/);
  await expect(pipeline).toContainText(/Change failure rate\s*17%/);
  await expect(pipeline).toContainText(/CI run\s*14 min/);
  await screen.getByRole('link', 'Open Pipeline').click();
  await expect(screen.getByRole('heading', { level: 1 })).toHaveText('Pipeline');
  await expect(browser).toHaveURL(/#\/s\/studio\/pipeline$/);
});
