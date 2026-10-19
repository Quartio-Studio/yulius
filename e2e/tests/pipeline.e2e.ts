import { test, type Browser } from '@e2e-dev/web';
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
  // A CI run's view: its queue, preparation and jobs with the stages their output named.
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
  // Its jobs got a slot at once (0 ms waits): nothing says "waited", no Waited column.
  await expect(browser.locator('.st-waited')).toHaveCount(0);
  await expect(browser.locator('.st-row.st-job:has-text("quality") .st-bar-wait')).toHaveCount(0);
  expect(await browser.locator('.st-row.st-job:has-text("quality")').getAttribute('title')).not.toContain('waited');
  await run.getByRole('button', 'Table').click();
  await expect(run.getByRole('table')).toContainText('test (2/2)');
  await expect(run.getByRole('columnheader', 'Waited')).toHaveCount(0);
  expect(await axeViolations(browser)).toEqual([]);

  // The same run on the Dashboard's Pipeline tab.
  await screen.getByRole('link', 'In Pipeline').click();
  await expect(browser).toHaveURL(/#\/s\/studio\/dashboard\?section=pipeline&open=ci_run%3Aci_s2$/);
  await expect(screen.getByRole('radio', 'Pipeline (stages)')).toHaveAttribute('aria-checked', 'true');
  await expect(screen.getByRole('region', 'CI run stages')).toContainText('quality');

  // A deploy's checks, run, deploy and the stages of its output.
  await app.open(mock('/s/studio/cd?run=cd_s2'));
  const deploy = screen.getByRole('region', 'Stages of this deploy');
  for (const text of ['Checks', 'Deployment', 'Deploy', 'build', 'upload']) await expect(deploy).toContainText(text);
  await screen.getByRole('region', 'Deploy').getByRole('link', 'In Pipeline').click();
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
  // The plot itself, a fold open, passes axe (its buttons, marks and counts).
  await browser.locator('.st-row.st-job:has-text("shared") .st-fold').click();
  await expect(browser.locator('.st-row[data-key="job:j0@1/checkout"]')).toBeVisible();
  expect(await axeViolations(browser)).toEqual([]);
  // The table keeps the wait as a column.
  await run.getByRole('button', 'Table').click();
  await expect(run.getByRole('columnheader', 'Waited')).toBeVisible();
  await expect(browser.locator('.st-table tr:has-text("engine") td.st-waited')).toHaveText('6 min');
  expect(await axeViolations(browser)).toEqual([]);
});

test('a job’s stages fold under it, a failed job opens, and the fold outlives a refresh', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/ci?run=ci_s4'));
  await expect(screen.getByRole('region', 'Stages of this run')).toContainText('engine');
  // Only the job with a failed stage (deployer) is open: one Checkout row, its own.
  await expect(browser.locator('.st-row:has-text("Checkout")')).toHaveCount(1);
  await expect(browser.locator('.st-row[data-key="job:j1@1/checkout"]')).toHaveCount(0);
  const fold = browser.locator('.st-row.st-job:has-text("engine") .st-fold');
  await expect(fold).toHaveAttribute('aria-expanded', 'false');
  await expect(fold).toContainText('2 stages');
  await fold.click();
  await expect(fold).toHaveAttribute('aria-expanded', 'true');
  await expect(browser.locator('.st-row[data-key="job:j1@1/checkout"]')).toBeVisible();
  await expect(browser.locator('.st-row:has-text("build")')).toBeVisible();
  // The running run's view redraws its stages every few seconds: what was opened stays open.
  await browser.evaluate(() => { document.querySelector('.st-waterfall')?.setAttribute('data-probe', 'old'); });
  await expect(browser.locator('.st-waterfall[data-probe]')).toHaveCount(0);
  await expect(browser.locator('.st-row.st-job:has-text("engine") .st-fold')).toHaveAttribute('aria-expanded', 'true');
  await expect(browser.locator('.st-row[data-key="job:j1@1/checkout"]')).toBeVisible();
  // So does the keyboard: the fold button that had focus has it again after the redraw.
  await browser.locator('.st-row.st-job:has-text("prototypes") .st-fold').focus();
  await browser.evaluate(() => { document.querySelector('.st-waterfall')?.setAttribute('data-probe', 'old'); });
  await expect(browser.locator('.st-waterfall[data-probe]')).toHaveCount(0);
  expect(await browser.evaluate(() => document.activeElement?.closest('.st-row')?.getAttribute('data-key') ?? null)).toBe('job:j2@1');

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
  // 9.1 min in: the 8 min tick (88 %) would crowd "now", so the axis stops at 6 min.
  expect(await axis()).toEqual(['0', '2 min', '4 min', '6 min', 'now']);
  expect(await legend()).toEqual(['Waiting', 'Ran', 'Still running', 'Failed']);

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
  // A wait still running keeps the hatch the legend gives "Waiting" (not the solid running look).
  const runningWait = await browser.evaluate(() => {
    const bar = document.querySelector('.st-row[data-key="wait@2"] .st-bar');
    return bar ? getComputedStyle(bar).backgroundImage : null;
  });
  expect(runningWait).toContain('repeating-linear-gradient');
  expect(await legend()).toContain('Waiting');
  expect(await browser.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(390);
  expect(await axeViolations(browser)).toEqual([]);
});

test('a short run’s axis names each tick once; a requeued job keeps each wait, and a lone wait keeps its row', async ({ app, browser, screen }) => {
  const axis = () => browser.evaluate(() => [...document.querySelectorAll('.st-waterfall .st-tick')].map((t) => t.textContent));
  await app.open(mock('/s/studio/ci?run=ci_s3'));
  await expect(screen.getByRole('region', 'Stages of this run')).toContainText('quality');
  expect(await axis()).toEqual(['0', '30 s', '1 min', '1 min 30 s']);
  // Requeued after its process host was lost: each attempt with its own wait, in waiting order.
  const rows = await jobRows(browser);
  expect(rows.map((r) => [r.name, r.took?.replace(/\s+/g, ' ')])).toEqual([['quality', '18 swaited 6 s'], ['security', '6 swaited 3 s'], ['quality', '1 minwaited 18 s']]);
  await expect(browser.locator('.st-row.st-job[data-key="job:j0@1"] [aria-label="error"]')).toBeVisible();
  // A skipped job (ADR-0133) says so with its mark and its own bar look, named in the legend.
  await expect(browser.locator('.st-row.st-job[data-key="job:j5@1"] [aria-label="skipped"]')).toBeVisible();
  await expect(browser.locator('.st-row.st-job[data-key="job:j5@1"] .st-bar-skipped')).toHaveCount(1);
  expect(await browser.evaluate(() => [...document.querySelectorAll('.st-bars-legend li')].map((t) => t.textContent))).toEqual(['Waiting', 'Ran', 'Skipped', 'Failed']);
  // A wait with no job span still shows, on its own row, under Run.
  await expect(browser.locator('.st-row:has-text("Waiting for a job slot")')).toHaveCount(1);
  // On a phone the CI run's stages fit the width.
  await browser.setViewport({ width: 390, height: 844 });
  await app.open(mock('/s/studio/ci?run=ci_s4'));
  await expect(screen.getByRole('region', 'Stages of this run')).toContainText('engine');
  expect(await browser.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(390);
  expect(await browser.evaluate(() => { const w = document.querySelector('.st-waterfall'); return w ? w.scrollWidth <= w.clientWidth : false; })).toBe(true);
  // No two axis labels touch, "now" included.
  expect(await browser.evaluate(() => {
    const boxes = [...document.querySelectorAll('.st-waterfall .st-tick')].map((t) => t.getBoundingClientRect());
    return boxes.every((b, i) => i === 0 || boxes[i - 1].right + 4 <= b.left);
  })).toBe(true);
});

test('a running job whose stage failed opens, and the legend names every bar look, folded or not', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/ci?run=ci_s4'));
  await expect(screen.getByRole('region', 'Stages of this run')).toContainText('engine');
  // "package (deployer)" still runs, but its "lint" stage failed: it opens on it, and the legend
  // says what the red bar means.
  await expect(browser.locator('.st-row.st-job:has-text("package (deployer)") .st-fold')).toHaveAttribute('aria-expanded', 'true');
  await expect(browser.locator('.st-row:has-text("lint") [aria-label="failed"]')).toBeVisible();
  expect(await browser.evaluate(() => [...document.querySelectorAll('.st-bars-legend li')].map((t) => t.textContent))).toEqual(['Waiting', 'Ran', 'Still running', 'Failed']);
  // Closed again, the legend still names the look of what it hides.
  await browser.locator('.st-row.st-job:has-text("package (deployer)") .st-fold').click();
  await expect(browser.locator('.st-row:has-text("lint")')).toHaveCount(0);
  expect(await browser.evaluate(() => [...document.querySelectorAll('.st-bars-legend li')].map((t) => t.textContent))).toEqual(['Waiting', 'Ran', 'Still running', 'Failed']);
  // The Table shows every span; the legend, which is about bars, goes with the plot.
  await screen.getByRole('region', 'Stages of this run').getByRole('button', 'Table').click();
  await expect(browser.locator('.st-bars-legend')).toBeHidden();
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
