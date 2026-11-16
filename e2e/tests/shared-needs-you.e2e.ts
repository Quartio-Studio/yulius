// SPEC-494 DFJ-011: the tasks of a sandbox whose Needs you name the same sandbox-level cause
// (its Docker) show as one card in the sidebar's Needs you group, naming them all; once the
// requests end, each task is back on its own row.
import { test } from '@e2e-dev/web';
import { expect } from 'e2e';
import { failOnPageErrors, mock, touchTask } from './support.ts';

failOnPageErrors();

test('tasks asking about the same sandbox problem share one Needs you card (SPEC-494)', async ({ app, browser }) => {
  await app.open(mock('/s/studio', '&ambient=0'));
  const rows = browser.locator('.sidebar .task-group.group-needs .task-row');
  const docker = (at: number) => ({ action: 'Restart the sandbox’s Docker', problem: 'Docker calls fail', decision: false, at: new Date(at).toISOString(), cause: 'docker' });
  await touchTask(browser, 'sb_studio', 12, { needs_you: docker(1_000) });
  await touchTask(browser, 'sb_studio', 4, { status: 'running', needs_you: docker(2_000) });
  const card = browser.locator('.sidebar .task-group.group-needs .task-row:has-text("This project’s Docker")');
  await expect(card).toHaveCount(1);
  const text = await card.textContent();
  expect(text).toContain('#4');
  expect(text).toContain('#12');
  // One row for both: the other's own row is gone from the group.
  const theirs = browser.locator('.sidebar .task-group.group-needs .task-link[data-route="/s/studio/t/12"], .sidebar .task-group.group-needs .task-link[data-route="/s/studio/t/4"]');
  await expect(theirs).toHaveCount(1);
  // Answered (or repaired): each task is on its own again.
  await touchTask(browser, 'sb_studio', 12, { needs_you: null });
  await touchTask(browser, 'sb_studio', 4, { needs_you: null });
  await expect(card).toHaveCount(0);
  await expect(browser.locator('.sidebar .task-link[data-route="/s/studio/t/12"]')).toHaveCount(1);
  await expect(browser.locator('.sidebar .task-link[data-route="/s/studio/t/4"]')).toHaveCount(1);
});
