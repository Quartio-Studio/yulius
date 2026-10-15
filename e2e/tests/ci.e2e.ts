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

  await screen.getByRole('button', 'Open the run of PR #218 at 0d7e5f3').click();
  const dialog = screen.getByRole('dialog', 'CI run');
  await expect(dialog).toBeVisible();
  await expect(dialog.getByRole('tab', /test \(2\/2\)/)).toHaveAttribute('aria-selected', 'true');
  await expect(browser.locator('.run-output')).toHaveText(/1 failed/);
  await expect(browser).toHaveURL(/#\/s\/studio\/ci\?run=ci_s2$/);
  expect(await axeViolations(browser)).toEqual([]);

  await dialog.getByRole('tab', /lint/).click();
  await expect(browser.locator('.run-output')).toHaveText(/no warnings/);
  await browser.keyboard.press('Escape');
  await expect(dialog).toBeHidden();
  await expect(browser).toHaveURL(/#\/s\/studio\/ci$/);

  await screen.getByRole('button', 'Run again: PR #218 at 0d7e5f3').click();
  await expect(screen.getByText('PR #218 queued again.')).toBeVisible();
});

test('a run opens from the link of its commit status', async ({ app, screen }) => {
  await app.open(mock('/s/studio/ci?run=ci_s3'));
  const dialog = screen.getByRole('dialog', 'CI run');
  await expect(dialog).toBeVisible();
  await expect(dialog.getByText('PR #221')).toBeVisible();
  await expect(dialog.getByRole('tab', /lint/)).toHaveAttribute('aria-selected', 'true');
});

// SPEC-162 SKP-003: PR #221 passed with "test (2/2)" skipped (it exited with the skip code): the
// run says so, and the job is Skipped, not Passed.
test('a skipped job reads Skipped, not Passed', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/ci'));
  await expect(screen.getByText('3 jobs · 2 passed · 1 skipped')).toBeVisible();
  await screen.getByRole('button', 'Open the run of PR #221 at 4be81c0').click();
  const dialog = screen.getByRole('dialog', 'CI run');
  await expect(browser.locator('.ci-tab:has-text("test (2/2)") .ci-skipped [aria-label="Skipped"]')).toBeVisible();
  await dialog.getByRole('tab', /test \(2\/2\)/).click();
  await expect(browser.locator('.run-output')).toHaveText(/scan not run[\s\S]*\[exit code 78\]/);
  expect(await axeViolations(browser)).toEqual([]);
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
  await expect(screen.getByRole('button', 'Run again: dev at 77aa01b')).toBeHidden();
  await expect(screen.getByRole('button', 'Clean up')).toBeDisabled();
});
