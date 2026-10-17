import { test } from '@e2e-dev/web';
import { expect } from 'e2e';
import { axeViolations, failOnPageErrors, mock } from './support.ts';

failOnPageErrors();

// Settings → Instance, "Sandbox Docker" (SPEC-203 HST-014, HST-015, HST-017, HST-012): whether
// sandboxes get Docker in `yulius doctor`'s words, the base's rebuild and when it is ready, a
// sandbox on its own Docker with the way back, the last update's re-check, and the smoke check
// run from the page with each check as it ends. The mock reports a check every second; the card
// asks every two seconds while one runs.

const bodyText = (browser: { evaluate: (fn: () => string) => Promise<string> }) => browser.evaluate(() => document.body.innerText);

test('sandboxes get Docker and the smoke check passes check by check', async ({ app, browser, screen }) => {
  await app.open(mock('/admin/instance'));
  await expect(screen.getByRole('heading', { level: 2, name: 'Sandbox Docker' })).toBeVisible();
  await expect(screen.getByText('sandboxes get Docker')).toBeVisible();
  await expect(screen.getByText(/each isolated sandbox gets its own engine/)).toBeVisible();
  await expect(screen.getByText('base base-9')).toBeVisible();
  expect(await axeViolations(browser)).toEqual([]);

  await screen.getByRole('button', 'Run smoke check').click();
  await expect(screen.getByText('Smoke check running')).toBeVisible();
  await expect(screen.getByRole('button', 'Run smoke check')).toBeDisabled();
  // Checks arrive while it still runs, not only at the end.
  await expect.poll(() => browser.evaluate(() => {
    const running = document.querySelector('.sandbox-docker-smoke-running');
    return running ? running.querySelectorAll('.sandbox-docker-checks li').length : -1;
  }), { timeout: 6000 }).toBeGreaterThan(0);
  await expect(screen.getByText('Smoke check passed')).toBeVisible({ timeout: 10000 });
  expect(await browser.locator('.sandbox-docker-checks li.ok').count()).toBe(5);
  expect(await bodyText(browser)).toMatch(/docker compose up \(two services\) in the old run/);
  expect(await bodyText(browser)).toMatch(/started by mkarp/);
  await expect(screen.getByRole('button', 'Run smoke check')).toBeEnabled();
  expect(await axeViolations(browser)).toEqual([]);
});

test('a failing smoke check stops at the check that failed and says why', async ({ app, browser, screen }) => {
  await app.open(mock('/admin/instance', '&smoke=fail'));
  await screen.getByRole('button', 'Run smoke check').click();
  await expect(screen.getByText('Smoke check failed')).toBeVisible({ timeout: 10000 });
  expect(await browser.locator('.sandbox-docker-checks li.failed').count()).toBe(1);
  await expect(screen.getByText(/pull rate limit/)).toBeVisible();
});

test('a base being rebuilt, a sandbox on its own Docker and a re-check problem are said plainly', async ({ app, browser, screen }) => {
  await app.open(mock('/admin/instance', '&docker=rebuild'));
  await expect(screen.getByText('no Docker in sandboxes')).toBeVisible();
  await expect(screen.getByText(/base-3\) has no `dockerd` though the host has one/)).toBeVisible();
  // The rebuild: running, with when it will be ready.
  await expect(screen.getByText(/Rebuilding the sandbox base .* ready around .* \(the last build took 3 min\)/)).toBeVisible();
  // The sandbox on its own docker.io, and the way back (never removing it).
  await expect(screen.getByText('quartio/product')).toBeVisible();
  await expect(screen.getByText(/apt-get install -y docker-ce docker-ce-cli containerd.io/)).toBeVisible();
  await expect(screen.getByText(/never `sudo apt-get remove docker.io`/)).toBeVisible();
  // The last update's re-check found a problem: in the card, with the reason.
  await expect(screen.getByText(/sandboxes may have lost Docker\./)).toBeVisible();
  await expect(screen.getByText(/kernel modules: cannot load overlay/)).toBeVisible();
  expect(await axeViolations(browser)).toEqual([]);
});

test('without isolation the card says why and offers no smoke check', async ({ app, screen }) => {
  await app.open(mock('/admin/instance', '&docker=legacy'));
  await expect(screen.getByText(/sandboxes are not isolated on this server/)).toBeVisible();
  await expect(screen.getByText(/sudo yulius host setup/)).toBeVisible();
  await expect(screen.getByRole('button', 'Run smoke check')).toBeDisabled();
});

test('an update whose re-check finds a regression says so in its outcome', async ({ app, browser, screen }) => {
  await app.open(mock('/admin/instance', '&update=1&docker=rebuild'));
  await screen.getByRole('button', 'Update now…').click();
  await screen.getByRole('dialog', /to 9c1e7d3\?$/).getByRole('button', 'Update to 9c1e7d3').click();
  await expect.poll(() => bodyText(browser), { timeout: 15000 }).toMatch(/Updated to 9c1e7d3/);
  await expect(screen.getByText('Sandboxes may have lost Docker with this update.')).toBeVisible();
  expect(await axeViolations(browser)).toEqual([]);
});

test('members never see it', async ({ app, screen }) => {
  await app.open(mock('/admin/instance', '&role=member'));
  await expect(screen.getByText(/Only the superadmin manages updates/)).toBeVisible();
  await expect(screen.getByRole('heading', { level: 2, name: 'Sandbox Docker' })).toBeHidden();
});
