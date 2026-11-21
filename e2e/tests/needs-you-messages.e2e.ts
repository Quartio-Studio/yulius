import { test, type Browser } from '@e2e-dev/web';
import { expect } from 'e2e';
import { axeViolations, failOnPageErrors, firstShown, mock, terminalShown, touchTask } from './support.ts';

failOnPageErrors();

// SPEC-578: every task entering Needs You immediately shows an actionable message beside its
// status (even when stopped or parked), in its attention bar, in the Needs You list, in the
// notification center, and in its timestamped wait history; clearing a browser handoff removes it.

const bar = (browser: Browser) => browser.locator('.attention-bar');
const stripStatus = (browser: Browser) => browser.locator('.strip-status');

test('stopped and parked open-step tasks show the actionable reason beside status, in Needs You list, and in history', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/t/13', '&ambient=0'));
  await expect(screen.getByRole('heading', { level: 1 })).toHaveText('#13 Improve the code');
  const origin = await browser.evaluate(() => location.origin);
  const at = new Date().toISOString();
  const notice = {
    source: 'system',
    episode: 'plan_open:5:1700000000000',
    happened: 'The task is parked with unfinished steps in its plan (5 open, next: "Verify parked PR checks").',
    action: 'Open the task and resume it to continue the remaining plan steps, or close it if done.',
    link: `${origin}/s/studio/t/13`,
    decision: false,
    at,
    at_ms: 1700000000000,
  };
  await touchTask(browser, 'sb_studio', 13, {
    status: 'parked',
    activity: 'waiting',
    needs_you_notice: notice,
    wait_history: [{ ...notice, resolved_at: null }],
  });

  // Visible beside the status even though the agent is stopped/parked.
  await expect(stripStatus(browser)).toHaveText(/5 open, next: "Verify parked PR checks"/);
  // Actionable explanation + Resume/Reply in the attention bar.
  await expect(bar(browser)).toHaveText(/Needs you[\s\S]*What happened[\s\S]*5 open, next: "Verify parked PR checks"[\s\S]*To do[\s\S]*resume it to continue/);
  await expect(bar(browser).getByRole('button', { name: 'Resume' })).toBeVisible();
  await expect(bar(browser).getByRole('button', { name: 'Reply' })).toBeVisible();
  // Reachable from the Needs You sidebar list with its specific explanation.
  await expect(browser.locator('.sidebar .task-link[data-route="/s/studio/t/13"] .sr-only:has-text("5 open, next:")')).toHaveCount(1);
  expect(await axeViolations(browser)).toEqual([]);

  // Timestamped wait history appears in the task overview page.
  await app.open(mock('/s/studio/tasks/13', '&ambient=0'));
  await touchTask(browser, 'sb_studio', 13, {
    status: 'parked',
    activity: 'waiting',
    needs_you_notice: notice,
    wait_history: [{ ...notice, resolved_at: null }],
  });
  await firstShown(browser.locator('#ti-wait-history'));
  await expect(browser.locator('.ti-wait-history')).toHaveText(/5 open, next: "Verify parked PR checks"/);
  await expect(browser.locator('.ti-wait-history')).toHaveText(/Open/);
  expect(await axeViolations(browser)).toEqual([]);
});

test('failing PR check shows the failed job, failing test, and Inspect CI link in the bar and beside status', async ({ app, browser }) => {
  await app.open(mock('/s/studio/t/12', '&ambient=0'));
  await terminalShown(browser);
  const origin = await browser.evaluate(() => location.origin);
  const ciLink = `${origin}/s/studio/ci?run=run_1536`;
  const at = new Date().toISOString();
  const notice = {
    source: 'system',
    episode: 'pr_fail:1536:run_1536',
    happened: 'The agent stopped and pull request #1536 CI run run_1536 failed: `test` exited 1 (failing tests: browser::handoff).',
    action: 'Inspect CI run run_1536 and reply to the agent or rerun CI.',
    link: ciLink,
    decision: false,
    at,
    at_ms: 1700000001000,
  };
  await touchTask(browser, 'sb_studio', 12, {
    status: 'running',
    activity: 'waiting',
    needs_you_notice: notice,
    wait_history: [{ ...notice, resolved_at: null }],
  });

  await expect(stripStatus(browser)).toHaveText(/pull request #1536 CI run run_1536 failed/);
  await expect(bar(browser)).toHaveText(/Needs you[\s\S]*pull request #1536 CI run run_1536 failed[\s\S]*Inspect CI run run_1536/);
  const inspect = browser.locator('.attention-bar a:has-text("Inspect CI")');
  await expect(inspect).toBeVisible();
  expect(await inspect.getAttribute('href')).toBe(ciLink);
  expect(await axeViolations(browser)).toEqual([]);
});

test('browser handoff reaches notifications, stays visible after activity changes, and clears on browser.action_cleared', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio', '&ambient=0'));
  await firstShown(browser.locator('.notif-btn'));
  const origin = await browser.evaluate(() => location.origin);
  const link = `${origin}/s/studio/browser?tab=tab-cal`;
  const at = new Date().toISOString();
  const asked = {
    action: 'Open the browser tab: Sign in to Google Calendar for the test customer.',
    happened: 'The agent asked you to act in the sandbox browser: Sign in to Google Calendar for the test customer.',
    problem: null,
    decision: false,
    link,
    browser_tab: 'tab-cal',
    at,
    at_ms: 1700000002000,
  };
  const notice = {
    source: 'browser',
    episode: 'browser:tab-cal:1700000002000',
    happened: asked.happened,
    action: asked.action,
    link,
    decision: false,
    browser_tab: 'tab-cal',
    at,
    at_ms: 1700000002000,
  };

  // Task #12 gets a browser handoff while the user is on the sandbox home: reaches notifications.
  await touchTask(browser, 'sb_studio', 12, {
    status: 'running',
    activity: 'waiting',
    needs_you: asked,
    needs_you_notice: notice,
  });
  await browser.locator('.notif-btn').click();
  await expect(browser.locator('.notif-list')).toHaveText(/Sign in to Google Calendar for the test customer/);
  await browser.locator('.notif-btn').click();

  // Opening the task shows the handoff even though activity is `waiting` (not `needs_attention`).
  await browser.locator('.sidebar .task-link[data-route="/s/studio/t/12"]').click();
  await terminalShown(browser);
  await expect(bar(browser)).toHaveText(/Sign in to Google Calendar for the test customer/);
  await expect(screen.getByRole('button', 'Open browser tab')).toBeVisible();
  expect(await axeViolations(browser)).toEqual([]);

  // Clearing the handoff removes the bar and keeps it cleared.
  await browser.evaluate(() => (window as unknown as { __yuliusMock: { clearBrowserAction: (s: string, t: string, tid: string) => boolean } }).__yuliusMock.clearBrowserAction('sb_studio', 'tab-cal', 'tsk_studio_12'));
  await touchTask(browser, 'sb_studio', 12, {
    status: 'running',
    activity: 'done',
    needs_you: null,
    needs_you_notice: null,
  });
  await expect(bar(browser)).toBeHidden();
});

test('live browser handoff while needs_attention offers inline Reply with timestamp, and progressing CI stays Working', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/t/12', '&ambient=0'));
  await terminalShown(browser);
  const origin = await browser.evaluate(() => location.origin);
  const link = `${origin}/s/studio/browser?tab=tab-live`;
  const at = new Date(Date.now() - 120_000).toISOString();
  const asked = {
    action: 'Open the browser tab: Approve Google OAuth consent.',
    happened: 'The agent asked you to act in the sandbox browser: Approve Google OAuth consent.',
    problem: null,
    decision: false,
    link,
    browser_tab: 'tab-live',
    at,
    at_ms: Date.now() - 120_000,
  };
  await touchTask(browser, 'sb_studio', 12, {
    status: 'running',
    activity: 'needs_attention',
    attention: { kind: 'yulius', problem: asked.happened, action: asked.action, link },
    needs_you: asked,
    needs_you_notice: { ...asked, source: 'browser', episode: `browser:tab-live:${asked.at_ms}` },
  });

  // Finding #9: timestamp is shown beside the heading.
  await expect(browser.locator('.attention-bar .needs-you-time')).toBeVisible();
  // Finding #1: clicking Reply opens the inline text answer form (not terminal-only focus).
  await bar(browser).getByRole('button', { name: 'Reply' }).click();
  await expect(browser.locator('.attention-bar .attention-reply input')).toBeVisible();
  expect(await axeViolations(browser)).toEqual([]);

  // Finding #8: when a newer CI run is progressing (`yulius_wait: true`), even a failing cached PR
  // does not show the Needs You attention bar.
  await touchTask(browser, 'sb_studio', 12, {
    status: 'running',
    activity: 'waiting',
    attention: null,
    needs_you: null,
    needs_you_notice: null,
    yulius_wait: true,
  });
  await expect(stripStatus(browser)).toHaveText(/Waiting for checks or merge/);
  await expect(bar(browser)).toBeHidden();
});
