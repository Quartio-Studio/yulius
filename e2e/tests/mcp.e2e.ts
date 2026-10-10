import { test } from '@e2e-dev/web';
import { expect } from 'e2e';
import { axeViolations, failOnPageErrors, mock } from './support.ts';

failOnPageErrors();

// MCP servers (SPEC-001 MCP-001..004, UI-038; F 2026-10-10, #136).

// "When I click Add mcp server, nothing happens." With no server attached, the empty state's
// Add server called a function the page no longer had (a ReferenceError, so nothing happened).
test('with no MCP server attached, the empty state’s Add server opens the add page', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/mcp'));
  await expect(screen.getByRole('heading', { level: 1 })).toHaveText('MCP');
  for (const name of ['quartio', 'apify']) {
    await screen.getByRole('button', `Remove ${name}`).click();
    await screen.getByRole('dialog', `Remove MCP server “${name}”?`).getByRole('button', 'Remove').click();
    await expect(screen.getByRole('button', `Remove ${name}`)).toBeHidden();
  }
  await expect(screen.getByText('No MCP servers yet')).toBeVisible();
  await browser.locator('.empty-state .btn-primary').click();
  await expect(browser).toHaveURL(/#\/s\/studio\/mcp\/new$/);
  await expect(screen.getByRole('heading', { level: 1 })).toHaveText('Add MCP server');
  expect(await axeViolations(browser)).toEqual([]);
});

test('a server is added from the catalog, tested, edited and removed', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/mcp'));
  await screen.getByRole('button', 'Add server').click();
  await expect(browser).toHaveURL(/#\/s\/studio\/mcp\/new$/);
  await screen.getByRole('radio', /Context7/).click();
  await expect(screen.getByRole('textbox', 'Server name')).toHaveValue('context7');
  await screen.getByRole('button', 'Add server').click();

  // After Add the server opens and is tested at once.
  await expect(browser).toHaveURL(/#\/s\/studio\/mcp\/mcp_\d+$/);
  await expect(screen.getByText(/Connected · 4 tools/)).toBeVisible();

  const command = screen.getByRole('textbox', 'Command line');
  await expect(command).toHaveValue('npx -y @upstash/context7-mcp');
  await command.fill('npx -y @upstash/context7-mcp --verbose');
  await screen.getByRole('button', 'Save').click();
  await expect(screen.getByText(/Saved context7/)).toBeVisible();

  // The list shows the change; the server reads it back.
  await browser.locator('a.gt-back').click();
  await expect(browser).toHaveURL(/#\/s\/studio\/mcp$/);
  await expect(screen.getByText('npx -y @upstash/context7-mcp --verbose')).toBeVisible();
  await screen.getByRole('button', 'Edit context7').click();
  await expect(screen.getByRole('textbox', 'Command line')).toHaveValue('npx -y @upstash/context7-mcp --verbose');

  await screen.getByRole('button', 'Remove').click();
  await screen.getByRole('dialog', 'Remove MCP server “context7”?').getByRole('button', 'Remove').click();
  await expect(browser).toHaveURL(/#\/s\/studio\/mcp$/);
  await expect(screen.getByRole('button', 'Edit context7')).toBeHidden();
  await expect(screen.getByRole('button', 'Edit apify')).toBeVisible();
});
