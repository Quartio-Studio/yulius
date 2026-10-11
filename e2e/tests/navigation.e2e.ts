import { test } from '@e2e-dev/web';
import { expect } from 'e2e';
import { failOnPageErrors, mock } from './support.ts';

failOnPageErrors();

test('a task opens from the sidebar', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/t/12'));
  await expect(screen.getByRole('heading', { level: 1 })).toHaveText('#12 Checkout API');

  await screen.getByRole('link', /3D model viewer/).click();

  await expect(browser).toHaveURL(/#\/s\/studio\/t\/11$/);
  await expect(screen.getByRole('heading', { level: 1 })).toHaveText('#11 3D model viewer');
});

test('the sandbox pages open from the sandbox navigation', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/t/12'));
  const nav = screen.getByRole('navigation', 'Sandbox');

  for (const [page, path] of [
    ['Tasks', 'tasks'],
    ['Sessions', 'sessions'],
    ['Git', 'git'],
    ['CI', 'ci'],
    ['Workflows', 'workflows'],
    ['Skills', 'skills'],
    ['Automations', 'automations'],
    ['MCP', 'mcp'],
    ['Secrets', 'secrets'],
    ['Dashboard', 'dashboard'],
  ]) {
    await nav.getByRole('link', page).click();
    await expect(browser).toHaveURL(new RegExp(`#/s/studio/${path}$`));
    await expect(screen.getByRole('heading', { level: 1 })).toHaveText(page);
  }
});

test('another sandbox opens from the sandbox tabs', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/t/12'));

  await screen.getByRole('navigation', 'Sandboxes').getByRole('link', /^product\b/).click();

  await expect(browser).toHaveURL(/#\/s\/product$/);
  await expect(screen.getByRole('heading', { level: 1 })).toHaveText('New task');
});
