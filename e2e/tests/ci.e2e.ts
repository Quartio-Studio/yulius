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

// SPEC-259 (#245): the summary is a card of its own colour over the textured page, so its state,
// its buttons and its facts keep a padding from its edges, also on a phone, where the head wraps,
// and while it loads, when its shape wraps as the loaded head does.
const summaryBox = (browser: { evaluate<T>(fn: () => T): Promise<T> }) => browser.evaluate(() => {
  const summary = document.querySelector('.run-view .run-summary');
  const items = [...(summary?.querySelector('.run-head')?.children || [])].filter((item) => !item.classList.contains('spacer'));
  const fact = summary?.querySelector('.run-facts')?.firstElementChild;
  if (!summary || items.length === 0 || !fact) return null;
  // Measured inside its border.
  const box = summary.getBoundingClientRect();
  const left = box.left + summary.clientLeft;
  const right = left + summary.clientWidth;
  const rects = items.map((item) => item.getBoundingClientRect());
  return {
    left: Math.round(Math.min(...rects.map((r) => r.left)) - left),
    right: Math.round(right - Math.max(...rects.map((r) => r.right))),
    facts: Math.round(fact.getBoundingClientRect().left - left),
    height: Math.round(box.height),
  };
});

test('a run’s summary keeps its content off the edges of its card', async ({ app, browser }) => {
  const shape = '.run-view .run-content > .skeleton[role="status"]';
  // A writer's ended run (Run again and In Pipeline) and a reader's (In Pipeline alone, sandbox-3).
  const runs = [{ path: '/s/studio/ci?run=ci_s2', read: '/api/sandboxes/sb_studio/ci/runs/ci_s2', buttons: 2 },
    { path: '/s/sandbox-3/ci?run=ci_t1', read: '/api/sandboxes/sb_three/ci/runs/ci_t1', buttons: 1 }];
  for (const run of runs) for (const width of [1280, 390]) {
    await browser.setViewport({ width, height: 800 });
    // A page load of its own for each width (`w=`), so the run's read is held again.
    await app.open(mock(run.path, `&w=${width}&hold=${run.read}`));
    await expect(browser.locator(shape)).toHaveText('Loading the CI run…');
    await expect(browser.locator(`${shape} .run-head .skeleton-button`)).toHaveCount(run.buttons);
    const loading = (await summaryBox(browser))!;
    expect({ ...loading, height: 0 }).toEqual({ left: 12, right: 12, facts: 12, height: 0 });
    await browser.evaluate(() => (window as unknown as { __yuliusMock: { release: () => void } }).__yuliusMock.release());
    await expect(browser.locator(shape)).toHaveCount(0);
    await expect(browser.locator('.run-view .run-head')).toContainText('In Pipeline');
    await expect(browser.locator('.run-view .run-head .btn')).toHaveCount(run.buttons);
    await expect(browser.locator('.run-view .run-time')).not.toHaveText('');
    // Loaded, its content keeps the padding and the card keeps the loading shape's height.
    await expect.poll(() => summaryBox(browser)).toEqual({ left: 12, right: 12, facts: 12, height: loading.height });
  }
});

// SPEC-269: Yulius removes the safe leftovers itself every 6 hours; Clean up does it now, shows
// at once that it runs, and the card then says what was removed and what was kept.
type HeldCleanup = Window & { __yuliusMock: { holdHygieneCleanup?: Promise<void>; failHygieneCleanup?: string }; __releaseCleanup?: () => void };

test('repository hygiene says Yulius cleans up itself and that unmerged work stays', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/ci'));
  const card = screen.getByRole('region', 'Repository hygiene');
  await expect(card.getByText('Branches and worktrees no task needs any more. Yulius removes the safe ones by itself (next in 4 h); Clean up removes them now.')).toBeVisible();
  await expect(browser.locator('.ci-hygiene li:has-text("spike/websocket-pool")')).toContainText('Kept');
  await expect(browser.locator('.ci-hygiene li:has-text("spike/websocket-pool")')).toContainText('unmerged work, which Yulius never deletes by itself');
  expect(await axeViolations(browser)).toEqual([]);
});

test('a cleanup shows at once that it runs, then what it removed and kept', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/ci'));
  const card = screen.getByRole('region', 'Repository hygiene');
  await expect(card.getByText('spike/websocket-pool')).toBeVisible();
  await browser.evaluate(() => {
    const w = window as unknown as HeldCleanup;
    w.__yuliusMock.holdHygieneCleanup = new Promise((resolve) => { w.__releaseCleanup = resolve; });
  });
  await card.getByRole('button', 'Clean up').click();
  const confirm = screen.getByRole('dialog', 'Remove 3 stale items?');
  await confirm.getByRole('button', 'Clean up').click();
  // Before the server answers: the card is busy and says what it is doing.
  await expect(card.getByRole('button', 'Cleaning up…')).toBeDisabled();
  await expect(card.getByRole('button', 'Check now')).toBeDisabled();
  await expect(card.getByRole('status')).toHaveText('Removing 3 items…');
  // The seconds so far count beside the live region, not in it (read once, not every second).
  await expect(browser.locator('.ci-hygiene-status')).toContainText('so far');
  await expect(card.getByRole('status')).not.toContainText('so far');
  // The pressed button is disabled: the keyboard stays on the card.
  await expect.poll(() => browser.evaluate(() => document.activeElement?.getAttribute('aria-label'))).toBe('Repository hygiene');
  expect(await axeViolations(browser)).toEqual([]);
  await browser.evaluate(() => { (window as unknown as HeldCleanup).__releaseCleanup?.(); });
  // The answer: the result stays on the card, no toast repeats it.
  const result = card.getByRole('status');
  await expect(result).toContainText('Cleaned up just now by mkarp: 3 removed.');
  await expect(browser.locator('.ci-hygiene-result li:has-text("worktrees/tsk_studio_9")')).toContainText('Removed');
  await expect(browser.locator('.ci-hygiene-result li:has-text("feat/12-old-landing")')).toHaveCount(2);
  await expect(card.getByRole('button', 'Clean up')).toBeDisabled();
  await expect(card.getByRole('button', 'Check now')).toBeEnabled();
  // Unmerged work without a pull request stays listed.
  await expect(browser.locator('.ci-hygiene:not(.ci-hygiene-result) li')).toHaveCount(1);
  await expect(browser.locator('.ci-hygiene:not(.ci-hygiene-result) li')).toContainText('spike/websocket-pool');
  await expect(browser.locator('.toast:has-text("removed")')).toHaveCount(0);
  expect(await axeViolations(browser)).toEqual([]);
  // A reload shows the same result: it is the report's, not the page's.
  await app.open(mock('/s/studio/ci'));
  await expect(screen.getByRole('region', 'Repository hygiene').getByRole('status')).toContainText('3 removed');
});

test('a failed cleanup says why in the card and can be tried again', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/ci'));
  const card = screen.getByRole('region', 'Repository hygiene');
  await expect(card.getByText('spike/websocket-pool')).toBeVisible();
  await browser.evaluate(() => { (window as unknown as HeldCleanup).__yuliusMock.failHygieneCleanup = 'GitHub is not available'; });
  await card.getByRole('button', 'Clean up').click();
  await screen.getByRole('dialog', 'Remove 3 stale items?').getByRole('button', 'Clean up').click();
  await expect(card.getByRole('status')).toContainText('Couldn’t clean up: GitHub is not available');
  await expect(card.getByRole('button', 'Clean up')).toBeEnabled();
  await expect(card.getByRole('button', 'Check now')).toBeEnabled();
  await expect(card.getByText('worktrees/tsk_studio_9')).toBeVisible();
  // Tried again once GitHub answers: it goes through.
  await browser.evaluate(() => { (window as unknown as HeldCleanup).__yuliusMock.failHygieneCleanup = undefined; });
  await card.getByRole('button', 'Clean up').click();
  await screen.getByRole('dialog', 'Remove 3 stale items?').getByRole('button', 'Clean up').click();
  await expect(card.getByRole('status')).toHaveText('Cleaned up just now by mkarp: 3 removed.');
});

test('a cleanup still running after the person left the tab shows as running when they come back', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/ci'));
  const card = screen.getByRole('region', 'Repository hygiene');
  await expect(card.getByText('spike/websocket-pool')).toBeVisible();
  await browser.evaluate(() => {
    const w = window as unknown as HeldCleanup;
    w.__yuliusMock.holdHygieneCleanup = new Promise((resolve) => { w.__releaseCleanup = resolve; });
  });
  await card.getByRole('button', 'Clean up').click();
  await screen.getByRole('dialog', 'Remove 3 stale items?').getByRole('button', 'Clean up').click();
  await expect(card.getByRole('button', 'Cleaning up…')).toBeDisabled();
  await screen.getByRole('radio', 'CD (deployment)').click();
  await screen.getByRole('radio', 'CI (tests)').click();
  const back = screen.getByRole('region', 'Repository hygiene');
  await expect(back.getByRole('button', 'Cleaning up…')).toBeDisabled();
  await expect(back.getByRole('status')).toHaveText('Removing 3 items…');
  await browser.evaluate(() => { (window as unknown as HeldCleanup).__releaseCleanup?.(); });
  await expect(back.getByRole('status')).toHaveText('Cleaned up just now by mkarp: 3 removed.');
  await expect(back.getByRole('button', 'Check now')).toBeEnabled();
});

test('a cleanup that fails after the person left the tab says why when they come back', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/ci'));
  const card = screen.getByRole('region', 'Repository hygiene');
  await expect(card.getByText('spike/websocket-pool')).toBeVisible();
  await browser.evaluate(() => {
    const w = window as unknown as HeldCleanup;
    w.__yuliusMock.holdHygieneCleanup = new Promise((resolve) => { w.__releaseCleanup = resolve; });
    w.__yuliusMock.failHygieneCleanup = 'GitHub is not available';
  });
  await card.getByRole('button', 'Clean up').click();
  await screen.getByRole('dialog', 'Remove 3 stale items?').getByRole('button', 'Clean up').click();
  await expect(card.getByRole('button', 'Cleaning up…')).toBeDisabled();
  await screen.getByRole('radio', 'CD (deployment)').click();
  await screen.getByRole('radio', 'CI (tests)').click();
  await browser.evaluate(() => { (window as unknown as HeldCleanup).__releaseCleanup?.(); });
  const back = screen.getByRole('region', 'Repository hygiene');
  await expect(back.getByRole('status')).toHaveText('Couldn’t clean up: GitHub is not available');
  await expect(back.getByRole('button', 'Clean up')).toBeEnabled();
});

test('a cleanup round that runs late reads as shortly, not in the past', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/ci'));
  await browser.evaluate(() => {
    const db = (window as unknown as { __yuliusMock: { ci: Record<string, { hygiene: { next_cleanup_at: string } }> } }).__yuliusMock;
    db.ci.sb_studio.hygiene.next_cleanup_at = new Date(Date.now() - 5 * 60000).toISOString();
  });
  const card = screen.getByRole('region', 'Repository hygiene');
  await card.getByRole('button', 'Check now').click();
  await expect(card.getByText(/removes the safe ones by itself \(next shortly\)/)).toBeVisible();
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
