import { test } from '@e2e-dev/web';
import { expect } from 'e2e';
import { axeViolations, failOnPageErrors, mock } from './support.ts';

failOnPageErrors();

// F 2026-10-10 (#138): Yulius is the projects' only CI. The mock's studio sandbox has three jobs
// (lint and a two-way test matrix) and four runs: dev running, PR #221 passed (its "test (2/2)"
// skipped), PR #218 failed in "test (2/2)", and an older PR #218 head cancelled when superseded
// (assets/mock.js, seedCi).

test('the CI page shows the jobs in force and opens a failed run at the failing job', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/ci'));
  await expect(screen.getByRole('heading', { level: 1 })).toHaveText('CI/CD');
  // SPEC-151: CI is the first tab of the CI/CD page, the one its plain address opens.
  await expect(screen.getByRole('radio', 'CI (tests)')).toHaveAttribute('aria-checked', 'true');
  await expect(screen.getByRole('radio', 'CD (deployment)')).toHaveAttribute('aria-checked', 'false');
  await expect(screen.getByRole('heading', '3 jobs from yulius.toml on the integration branch')).toBeVisible();
  await expect(screen.getByText('cargo clippy --all-targets -- -D warnings')).toBeVisible();
  await expect(screen.getByText(/superseded by 0d7e5f3/)).toBeVisible();

  // SPEC-238 CRV-001: a run opens inside the CI tab at its own address, not in a dialog, and the
  // keyboard goes with it (review round 1: the card it was on is gone).
  await screen.getByRole('button', 'Open the run of PR #218 at 0d7e5f3').press('Enter');
  const run = screen.getByRole('region', 'CI run');
  await expect(run).toBeVisible();
  await expect.poll(() => browser.evaluate(() => document.activeElement?.getAttribute('aria-label'))).toBe('CI run');
  await expect(run.getByRole('tab', /test \(2\/2\)/)).toHaveAttribute('aria-selected', 'true');
  await expect(browser.locator('.run-output')).toHaveText(/1 failed/);
  await expect(browser).toHaveURL(/#\/s\/studio\/ci\?run=ci_s2$/);
  expect(await browser.evaluate(() => document.querySelectorAll('dialog[open]').length)).toBe(0);
  await expect(screen.getByRole('heading', 'Runs')).toBeHidden();
  expect(await axeViolations(browser)).toEqual([]);

  await run.getByRole('tab', /lint/).click();
  await expect(browser.locator('.run-output')).toHaveText(/no warnings/);
  // All runs goes back to the list; Back and Forward move between the two.
  await run.getByRole('link', 'All runs').click();
  await expect(run).toBeHidden();
  await expect(browser).toHaveURL(/#\/s\/studio\/ci$/);
  // Back in the list, the keyboard is on the run it came from.
  await expect(screen.getByRole('button', 'Open the run of PR #218 at 0d7e5f3')).toBeFocused();
  await browser.evaluate(() => history.back());
  await expect(screen.getByRole('region', 'CI run')).toBeVisible();
  await expect(browser).toHaveURL(/#\/s\/studio\/ci\?run=ci_s2$/);
  await browser.evaluate(() => history.back());
  await expect(screen.getByRole('region', 'CI run')).toBeHidden();
  await browser.evaluate(() => history.forward());
  await expect(screen.getByRole('region', 'CI run')).toBeVisible();
  // The sidebar's CI/CD link, from another page, comes back to the run (SPEC-113 places).
  await screen.getByRole('link', 'Tasks').click();
  await expect(screen.getByRole('region', 'CI run')).toBeHidden();
  await screen.getByRole('link', 'CI/CD').click();
  await expect(screen.getByRole('region', 'CI run')).toBeVisible();
  await expect(browser).toHaveURL(/#\/s\/studio\/ci\?run=ci_s2$/);
  await screen.getByRole('region', 'CI run').getByRole('link', 'All runs').click();
});

// SPEC-258 (#774): "throw out run again lol what is even that". The list repeats no Run again;
// the running card keeps Cancel, and a run is tested again from its own view.
test('Run again lives in the run’s view, not on every card', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/ci'));
  await expect(screen.getByRole('heading', 'Runs')).toBeVisible();
  await expect(browser.locator('.item-list button:has-text("Run again")')).toHaveCount(0);
  await expect(browser.locator('.item-list button:has-text("Cancel")')).toHaveCount(1);

  await screen.getByRole('button', 'Open the run of PR #218 at 0d7e5f3').click();
  const run = screen.getByRole('region', 'CI run');
  await expect(run.getByRole('button', 'Run again: PR #218 at 0d7e5f3')).toBeVisible();
  await expect(run.getByRole('button', /^Cancel/)).toBeHidden();
  expect(await axeViolations(browser)).toEqual([]);
  await run.getByRole('button', 'Run again: PR #218 at 0d7e5f3').click();
  await expect(screen.getByText('PR #218 queued again.')).toBeVisible();
  // The new run opens in place of the old one, at its own address; Back returns to the old one.
  await expect(browser).toHaveURL(/#\/s\/studio\/ci\?run=ci_\d+$/);
  await expect(screen.getByRole('region', 'CI run').getByText('Running')).toBeVisible();
  await expect(screen.getByRole('region', 'CI run').getByRole('button', 'Cancel: PR #218 at 0d7e5f3')).toBeVisible();
  await browser.evaluate(() => history.back());
  await expect(browser).toHaveURL(/#\/s\/studio\/ci\?run=ci_s2$/);
});

// Review round 1: another job's tab keeps the head's Run again and In Pipeline; a click while the
// request is out sends no second one.
test('Run again stays across job tabs and is off while it is being sent', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/ci?run=ci_s2', '&holdpost=/api/sandboxes/sb_studio/ci/runs/ci_s2/rerun'));
  const run = screen.getByRole('region', 'CI run');
  await run.getByRole('tab', /lint/).click();
  await expect(browser.locator('.run-output')).toHaveText(/no warnings/);
  await expect(run.getByRole('link', 'In Pipeline')).toBeVisible();
  const again = run.getByRole('button', 'Run again: PR #218 at 0d7e5f3');
  await again.click();
  await expect(again).toBeDisabled();
  await browser.evaluate(() => (window as unknown as { __yuliusMock: { release: () => void } }).__yuliusMock.release());
  await expect(screen.getByText('PR #218 queued again.')).toBeVisible();
  await expect(browser).toHaveURL(/#\/s\/studio\/ci\?run=ci_\d+$/);
  await expect(screen.getByText(/Couldn’t run it again/)).toBeHidden();
});

test('a running run is cancelled from its view and can then run again', async ({ app, screen }) => {
  await app.open(mock('/s/studio/ci?run=ci_s4'));
  const run = screen.getByRole('region', 'CI run');
  await expect(run.getByText('Running')).toBeVisible();
  await expect(run.getByRole('button', /^Run again/)).toBeHidden();
  await run.getByRole('button', /^Cancel: /).click();
  const confirm = screen.getByRole('dialog', /^Cancel the run of /);
  await confirm.getByRole('button', 'Cancel run').click();
  await expect(run.getByText('Cancelled')).toBeVisible();
  await expect(run.getByRole('button', /^Cancel: /)).toBeHidden();
  // Review round 1: the keyboard goes from the Cancel it used to the Run again that replaced it.
  await expect(run.getByRole('button', /^Run again: /)).toBeFocused();
});

test('a run opens from the link of its commit status', async ({ app, screen }) => {
  await app.open(mock('/s/studio/ci?run=ci_s3'));
  const run = screen.getByRole('region', 'CI run');
  await expect(run).toBeVisible();
  await expect(run.getByText('PR #221')).toBeVisible();
  await expect(run.getByRole('tab', /lint/)).toHaveAttribute('aria-selected', 'true');
  // Review round 1: the header's Check for commits works in the run's view too.
  await screen.getByRole('button', 'Check for commits').click();
  await expect(screen.getByText('Looked for new commits.')).toBeVisible();
  await expect(screen.getByText(/Couldn’t check for commits/)).toBeHidden();
  await expect(run).toBeVisible();
});

// SPEC-162 SKP-003: PR #221 passed with "test (2/2)" skipped (it exited with the skip code): the
// run says so, and the job is Skipped, not Passed.
test('a skipped job reads Skipped, not Passed', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/ci'));
  await expect(screen.getByText('3 jobs · 2 passed · 1 skipped')).toBeVisible();
  await screen.getByRole('button', 'Open the run of PR #221 at 4be81c0').click();
  const run = screen.getByRole('region', 'CI run');
  await expect(browser.locator('.ci-tab:has-text("test (2/2)") .ci-skipped [aria-label="Skipped"]')).toBeVisible();
  await run.getByRole('tab', /test \(2\/2\)/).click();
  await expect(browser.locator('.run-output')).toHaveText(/scan not run[\s\S]*\[exit code 78\]/);
  expect(await axeViolations(browser)).toEqual([]);
});

// SPEC-238 CRV-003: "we see some dynamic summary all the time". A running run's view starts with
// a summary that stays on top while the page scrolls: its state, its jobs, a time that grows by
// itself and the stages running now (the mock's dev run ci_s4: four jobs still running).
const secondsShown = (browser: { evaluate<T>(fn: () => T): Promise<T> }) => browser.evaluate(() => {
  const text = document.querySelector('.run-view .run-time')?.textContent || '';
  const m = /^(?:(\d+) min )?(\d+) s so far$/.exec(text);
  return m ? Number(m[1] || 0) * 60 + Number(m[2]) : null;
});

test('a running run keeps a live summary on top: its time, its jobs and the stages running now', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/ci?run=ci_s4'));
  const run = screen.getByRole('region', 'CI run');
  const summary = browser.locator('.run-view .run-summary');
  await expect(run.getByText('Running')).toBeVisible();
  await expect(summary).toContainText('3 jobs · 1 passed · 2 running');
  await expect(summary).toContainText('Now: engine › build, package (gateway) and 2 more');
  await expect.poll(() => secondsShown(browser)).not.toBeNull();
  const first = (await secondsShown(browser))!;
  await expect.poll(() => secondsShown(browser), { timeout: 5000 }).toBeGreaterThan(first);
  expect(await axeViolations(browser)).toEqual([]);

  // Scrolled to the log in a short window, the summary is still at the top of the page.
  await browser.setViewport({ width: 1440, height: 520 });
  await browser.evaluate(() => { document.querySelector('.page')!.scrollTop = 100000; });
  await expect.poll(() => browser.evaluate(() => {
    const page = document.querySelector('.page')!;
    const top = document.querySelector('.run-view .run-summary')!.getBoundingClientRect().top;
    return page.scrollTop > 200 && Math.abs(top - page.getBoundingClientRect().top) <= 2;
  })).toBe(true);
  await browser.setViewport({ width: 1440, height: 900 });

  // An ended run says how long it took and runs nothing now, before its stages have loaded (review
  // round 1: the summary does not grow under them).
  await app.open(mock('/s/studio/ci?run=ci_s2', '&hold=/api/sandboxes/sb_studio/stages/timeline'));
  await expect(browser.locator('.run-view .run-time')).toHaveText(/^took /);
  await expect(browser.locator('.run-view .run-stages .st-row')).toHaveCount(0);
  await browser.evaluate(() => (window as unknown as { __yuliusMock: { release: () => void } }).__yuliusMock.release());
  await expect(browser.locator('.run-view .run-stages .st-row').first()).toBeVisible();
  await expect(browser.locator('.run-view .run-summary')).not.toContainText('Now:');
});

// SPEC-259 (#245): the summary is a band of its own colour over the textured page, so its state,
// its "In Pipeline" button and its facts keep a padding from the band's edges, also on a phone
// and while it loads.
const summaryInsets = (browser: { evaluate<T>(fn: () => T): Promise<T> }) => browser.evaluate(() => {
  const summary = document.querySelector('.run-view .run-summary');
  const head = summary?.querySelector('.run-head');
  const facts = summary?.querySelector('.run-facts');
  if (!summary || !head?.firstElementChild || !head.lastElementChild || !facts?.firstElementChild) return null;
  // Measured inside its border, if it has one.
  const box = summary.getBoundingClientRect();
  const left = box.left + summary.clientLeft;
  const right = left + summary.clientWidth;
  return {
    left: Math.round(head.firstElementChild.getBoundingClientRect().left - left),
    right: Math.round(right - head.lastElementChild.getBoundingClientRect().right),
    facts: Math.round(facts.firstElementChild.getBoundingClientRect().left - left),
  };
});

test('a run’s summary keeps its content off the edges of its band', async ({ app, browser }) => {
  for (const width of [1280, 390]) {
    await browser.setViewport({ width, height: 800 });
    await app.open(mock('/s/studio/ci?run=ci_s2', '&hold=/api/sandboxes/sb_studio/ci/runs/ci_s2'));
    // While it loads, its shape (SPEC-120) keeps the same padding.
    await expect.poll(() => summaryInsets(browser)).toEqual({ left: 12, right: 12, facts: 12 });
    await browser.evaluate(() => (window as unknown as { __yuliusMock: { release: () => void } }).__yuliusMock.release());
    await expect(browser.locator('.run-view .run-head')).toContainText('In Pipeline');
    await expect.poll(() => summaryInsets(browser)).toEqual({ left: 12, right: 12, facts: 12 });
  }
});

test('repository hygiene removes only what is safe', async ({ app, screen }) => {
  await app.open(mock('/s/studio/ci'));
  await expect(screen.getByRole('heading', 'Repository hygiene')).toBeVisible();
  await expect(screen.getByText('spike/websocket-pool')).toBeVisible();
  await screen.getByRole('button', 'Clean up').click();
  const confirm = screen.getByRole('dialog', 'Remove 3 stale items?');
  await expect(confirm).toBeVisible();
  await confirm.getByRole('button', 'Clean up').click();
  await expect(screen.getByText('3 items removed.')).toBeVisible();
  // Unmerged work without a pull request stays.
  await expect(screen.getByText('spike/websocket-pool')).toBeVisible();
  await expect(screen.getByText('worktrees/tsk_studio_9')).toBeHidden();
});

test('a sandbox without CI says how to add it and names leftover GitHub Actions', async ({ app, browser, screen }) => {
  await app.open(mock('/s/product/ci'));
  await expect(screen.getByRole('heading', 'No CI configured')).toBeVisible();
  await expect(screen.getByText(/GitHub Actions workflows are still on the integration branch \(ci\.yml, deploy\.yml\)/)).toBeVisible();
  await expect(screen.getByText('No runs yet')).toBeVisible();
  expect(await axeViolations(browser)).toEqual([]);
});

test('people with read access watch but cannot run or clean up', async ({ app, screen }) => {
  await app.open(mock('/s/sandbox-3/ci'));
  await expect(screen.getByRole('heading', '1 job from the sandbox’s delivery profile')).toBeVisible();
  await expect(screen.getByRole('button', 'Check for commits')).toBeDisabled();
  await expect(screen.getByRole('button', 'Clean up')).toBeDisabled();
  // SPEC-258 RRA-003: their run's view offers neither Run again nor Cancel.
  await screen.getByRole('button', 'Open the run of dev at 77aa01b').click();
  const run = screen.getByRole('region', 'CI run');
  await expect(run.getByRole('link', 'In Pipeline')).toBeVisible();
  await expect(run.getByRole('button', /^(Run again|Cancel)/)).toBeHidden();
});
