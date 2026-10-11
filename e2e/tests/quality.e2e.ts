import { test } from '@e2e-dev/web';
import { expect } from 'e2e';
import { axeViolations, failOnPageErrors, mock } from './support.ts';

failOnPageErrors();

// Every main page: no WCAG 2.x A/AA violations (and, as everywhere, no page errors).
const pages = [
  ['home', '/', 'Sandboxes'],
  ['task', '/s/studio/t/12', '#12 Checkout API'],
  ['new task', '/s/studio', 'New task'],
  ['tasks', '/s/studio/tasks', 'Tasks'],
  ['git', '/s/studio/git', 'Git'],
  ['workflows', '/s/studio/workflows', 'Workflows'],
  ['skills', '/s/studio/skills', 'Skills'],
  ['automations', '/s/studio/automations', 'Automations'],
  ['MCP', '/s/studio/mcp', 'MCP'],
  ['secrets', '/s/studio/secrets', 'Secrets'],
  ['agent secrets', '/s/studio/secrets?section=agents', 'Secrets'],
  ['dashboard', '/s/studio/dashboard', 'Dashboard'],
  ['settings', '/settings', 'Settings'],
] as const;

for (const [name, path, heading] of pages) {
  test(`the ${name} page has no errors and no WCAG A/AA violations`, async ({ app, browser, screen }) => {
    await app.open(mock(path));
    await expect(screen.getByRole('heading', { level: 1 })).toHaveText(heading);

    expect(await axeViolations(browser)).toEqual([]);
  });
}
