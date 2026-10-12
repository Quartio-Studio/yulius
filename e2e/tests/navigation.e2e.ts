import { test } from '@e2e-dev/web';
import { expect } from 'e2e';
import { axeViolations, failOnPageErrors, mock } from './support.ts';

failOnPageErrors();

test('a task opens from the sidebar', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/t/12'));
  await expect(screen.getByRole('heading', { level: 1 })).toHaveText('#12 Checkout API');

  await screen.getByRole('link', /3D model viewer/).click();

  await expect(browser).toHaveURL(/#\/s\/studio\/t\/11$/);
  await expect(screen.getByRole('heading', { level: 1 })).toHaveText('#11 3D model viewer');
});

// Task #27 (SPEC-029): "Too much elements here. Only Cockpit, Tasks, Git, CI (and CD in the
// future), Secrets and Dashboard should be here. Other should be to be expanded".
const PRIMARY = [
  ['Cockpit', 'cockpit'],
  ['Tasks', 'tasks'],
  ['Git', 'git'],
  ['CI', 'ci'],
  ['Secrets', 'secrets'],
  ['Dashboard', 'dashboard'],
];
const MORE = [
  ['Sessions', 'sessions'],
  ['Workflows', 'workflows'],
  ['Skills', 'skills'],
  ['Automations', 'automations'],
  ['MCP', 'mcp'],
];

test('the sandbox navigation shows six pages and expands the rest under More', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/t/12'));
  const nav = screen.getByRole('navigation', 'Sandbox');
  const more = nav.getByRole('button', 'More');

  await expect(nav.getByRole('link')).toHaveCount(PRIMARY.length);
  await expect(browser.locator('nav.nav a.nav-item:visible')).toHaveText(PRIMARY.map(([page]) => page));
  await expect(more).toHaveAttribute('aria-expanded', 'false');
  for (const [page] of MORE) await expect(nav.getByRole('link', page)).toBeHidden();
  expect(await axeViolations(browser)).toEqual([]);

  await more.click();
  await expect(more).toHaveAttribute('aria-expanded', 'true');
  await expect(nav.getByRole('link')).toHaveCount(PRIMARY.length + MORE.length);
  await expect(browser.locator('nav.nav .nav-more .nav-item')).toHaveText(MORE.map(([page]) => page));
  expect(await axeViolations(browser)).toEqual([]);

  // Kept after a reload, then closed again.
  await browser.reload();
  await expect(more).toHaveAttribute('aria-expanded', 'true');
  await more.click();
  await expect(more).toHaveAttribute('aria-expanded', 'false');
  for (const [page] of MORE) await expect(nav.getByRole('link', page)).toBeHidden();
  await browser.reload();
  await expect(more).toHaveAttribute('aria-expanded', 'false');
});

test('the sandbox pages open from the sandbox navigation', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/t/12'));
  const nav = screen.getByRole('navigation', 'Sandbox');

  for (const [page, path] of PRIMARY) {
    await nav.getByRole('link', page).click();
    await expect(browser).toHaveURL(new RegExp(`#/s/studio/${path}$`));
    await expect(screen.getByRole('heading', { level: 1 })).toHaveText(page);
  }
  await nav.getByRole('button', 'More').click();
  for (const [page, path] of MORE) {
    await nav.getByRole('link', page).click();
    await expect(browser).toHaveURL(new RegExp(`#/s/studio/${path}$`));
    await expect(screen.getByRole('heading', { level: 1 })).toHaveText(page);
    await expect(nav.getByRole('link', page)).toHaveAttribute('aria-current', 'page');
  }
});

test('a page under More opens with the group expanded and the page current', async ({ app, screen }) => {
  await app.open(mock('/s/studio/mcp'));
  const nav = screen.getByRole('navigation', 'Sandbox');

  await expect(nav.getByRole('button', 'More')).toHaveAttribute('aria-expanded', 'true');
  await expect(nav.getByRole('link', 'MCP')).toBeVisible();
  await expect(nav.getByRole('link', 'MCP')).toHaveAttribute('aria-current', 'page');
});

test('another sandbox opens from the sandbox tabs', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/t/12'));

  await screen.getByRole('navigation', 'Sandboxes').getByRole('link', /^product\b/).click();

  await expect(browser).toHaveURL(/#\/s\/product$/);
  await expect(screen.getByRole('heading', { level: 1 })).toHaveText('New task');
});
