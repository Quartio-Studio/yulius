import { test } from '@e2e-dev/web';
import { expect } from 'e2e';
import { axeViolations, failOnPageErrors, mock } from './support.ts';

failOnPageErrors();

// The browser panel (SPEC-002 ADR-14): the start page lists the sandbox's listening ports, a
// port opens live in a tab of your own (in mock mode the "live page" is a small stand-in), and
// the sandbox Chrome's tabs — the agent's — share the strip and are watched remotely.
test('a sandbox port opens live and the sandbox Chrome tabs share the strip', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/t/12'));
  await screen.getByRole('button', /^Show tools/).click();
  await screen.getByRole('button', 'Browser').click();

  const start = screen.getByRole('region', 'New tab');
  await expect(start.getByRole('heading', /^Browser of /)).toBeVisible();
  await start.getByRole('button', /^localhost:\d+/).click();

  const live = screen.getByRole('tab', /^localhost:\d+\/$/);
  await expect(live).toHaveAttribute('aria-selected', 'true');
  await expect(browser.locator('iframe.bp-live')).toBeAttached();
  await expect(screen.getByRole('textbox', 'Address')).toHaveValue(/^localhost:\d+\/$/);
  expect(await axeViolations(browser)).toEqual([]);

  // The agent's sign-in tab in the sandbox Chrome asks for the user's action.
  const aws = screen.getByRole('tab', /Amazon Web Services Sign-In/);
  await aws.click();
  await expect(aws).toHaveAttribute('aria-selected', 'true');
  await expect(screen.getByText(/An agent asks for your action here/)).toBeVisible();
  // Back to the live tab: the page is still there.
  await live.click();
  await expect(live).toHaveAttribute('aria-selected', 'true');
  await expect(browser.locator('iframe.bp-live')).toBeVisible();
});
