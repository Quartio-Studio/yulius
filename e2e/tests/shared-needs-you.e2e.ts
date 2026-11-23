// SPEC-494 DFJ-011: the tasks of a sandbox whose Needs you name the same sandbox-level cause
// (its Docker) are grouped under one shared card in the sidebar's Needs you group: it names them
// all, each a link, and every task keeps its own row (its link, menu and count). Once the
// requests end the card goes.
import { test, type Browser } from '@e2e-dev/web';
import { expect } from 'e2e';
import { failOnPageErrors, mock, touchTask } from './support.ts';

failOnPageErrors();

/** The notification center's entries about tasks and shared problems, by key. */
const entries = (browser: Browser) => browser.evaluate(() => {
  const keys: string[] = [];
  for (let i = 0; i < localStorage.length; i += 1) {
    const name = localStorage.key(i) || '';
    if (!name.startsWith('yulius.notifications.') || name === 'yulius.notifications.filter') continue;
    for (const entry of JSON.parse(localStorage.getItem(name) || '[]')) {
      const key = String(entry.key || '');
      if (key.startsWith('needs-you:') || key.startsWith('task:')) keys.push(key);
    }
  }
  return keys;
});

test('tasks asking about the same sandbox problem share one Needs you card (SPEC-494)', async ({ app, browser }) => {
  await app.open(mock('/s/studio', '&ambient=0'));
  // The server names the episode by its oldest open request.
  const docker = (at: number) => ({ action: 'Restart the sandbox’s Docker', problem: 'Docker calls fail', decision: false, at: new Date(at).toISOString(), cause: 'docker', episode: new Date(1_000).toISOString() });
  // #12 asked first, #4 later.
  await touchTask(browser, 'sb_studio', 12, { needs_you: docker(1_000) });
  await touchTask(browser, 'sb_studio', 4, { status: 'running', needs_you: docker(2_000) });
  const CARD = '.sidebar .task-group.group-needs .task-shared:has-text("This project’s Docker")';
  const card = browser.locator(CARD);
  const links = browser.locator(`${CARD} a`);
  await expect(card).toHaveCount(1);
  // It names both, first asker first, each a link to its task.
  await expect(links).toHaveCount(2);
  expect(await links.first().textContent()).toBe('#12');
  expect(await links.nth(1).getAttribute('data-route')).toBe('/s/studio/t/4');
  // The notification center holds one entry for the shared problem, none per task.
  await expect.poll(() => entries(browser)).toEqual(['needs-you:sb_studio:docker']);
  // Each task keeps its own row, right under the card.
  await expect(browser.locator('.sidebar .task-group.group-needs .task-link[data-route="/s/studio/t/12"]')).toHaveCount(1);
  await expect(browser.locator('.sidebar .task-group.group-needs .task-link[data-route="/s/studio/t/4"]')).toHaveCount(1);
  // A change of one task's activity does not move the card or drop the keyboard from it.
  await links.first().focus();
  await touchTask(browser, 'sb_studio', 4, { activity: 'working' });
  await expect(card).toHaveCount(1);
  expect(await browser.evaluate(() => document.activeElement?.textContent ?? null)).toBe('#12');
  // Answered (or repaired): the card goes; the tasks stay.
  await touchTask(browser, 'sb_studio', 12, { needs_you: null });
  await touchTask(browser, 'sb_studio', 4, { needs_you: null });
  await expect(card).toHaveCount(0);
  await expect(browser.locator('.sidebar .task-link[data-route="/s/studio/t/12"]')).toHaveCount(1);
  // Its entry waited for a decision and never expired; answered, it goes a day later (SPEC-624).
  await expect.poll(() => browser.evaluate(() => {
    const name = Object.keys(localStorage).find((k) => k.startsWith('yulius.notifications.') && k !== 'yulius.notifications.filter') || '';
    const entry = JSON.parse(localStorage.getItem(name) || '[]').find((e: { key: string }) => e.key === 'needs-you:sb_studio:docker');
    return entry ? Math.round((Date.parse(entry.expires) - Date.now()) / 3600000) : null;
  })).toBe(24);
});

test('one notification per shared problem, also seen from another project (SPEC-494)', async ({ app, browser }) => {
  await app.open(mock('/s/product', '&ambient=0'));
  const docker = (at: number) => ({ action: 'Restart the sandbox’s Docker', decision: false, at: new Date(at).toISOString(), cause: 'docker', episode: new Date(1_000).toISOString() });
  await touchTask(browser, 'sb_studio', 12, { needs_you: docker(1_000) });
  await touchTask(browser, 'sb_studio', 4, { status: 'running', needs_you: docker(2_000) });
  await expect.poll(() => entries(browser)).toEqual(['needs-you:sb_studio:docker']);
  // Still asked, though this page has not loaded studio's list: another studio task's news does
  // not settle it (review of #1711).
  await touchTask(browser, 'sb_studio', 7, { activity: 'working' });
  await touchTask(browser, 'sb_three', 1, { activity: 'working' });
  const state = () => browser.evaluate(() => {
    const name = Object.keys(localStorage).find((k) => k.startsWith('yulius.notifications.') && k !== 'yulius.notifications.filter') || '';
    const entry = JSON.parse(localStorage.getItem(name) || '[]').find((e: { key: string }) => e.key === 'needs-you:sb_studio:docker');
    return entry ? [entry.read, entry.expires] : null;
  });
  expect(await state()).toEqual([false, null]);
});
