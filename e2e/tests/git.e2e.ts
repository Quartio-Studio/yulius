import { test } from '@e2e-dev/web';
import { expect } from 'e2e';
import { axeViolations, failOnPageErrors, mock } from './support.ts';

failOnPageErrors();

// The mock's studio sandbox: its GitHub App lacks "Commit statuses" (assets/mock.js, checksAccess).
const MISSING = /can't read commit statuses for Quartio-Studio\/studio: the GitHub App needs the 'Commit statuses: read' permission/;

// F 2026-10-10 (#111): "bruh" at a yellow "GitHub: checks cannot be read: GitHub: GitHub refused
// the request (HTTP 403)" banner. A permission the App lacks is one quiet line for administrators.
test('the Git page names a missing GitHub App permission to administrators until dismissed', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/git'));
  await expect(screen.getByRole('heading', { level: 1 })).toHaveText('Git');
  await expect(screen.getByText(MISSING)).toBeVisible();
  await expect(screen.getByRole('link', 'Open the App’s permissions')).toHaveAttribute(
    'href',
    'https://github.com/organizations/Quartio-Studio/settings/apps/yulius-quartio-studio/permissions',
  );
  // Not a warning banner.
  expect(await browser.evaluate(() => document.querySelectorAll('.banner-warn').length)).toBe(0);
  expect(await axeViolations(browser)).toEqual([]);

  // Dismissed, it stays hidden for the session, also after a reload.
  await screen.getByRole('button', 'Dismiss until the next session').click();
  await expect(screen.getByText(MISSING)).toBeHidden();
  await browser.reload();
  await expect(screen.getByRole('heading', { level: 1 })).toHaveText('Git');
  await expect(screen.getByRole('heading', /^Delivery needs you/)).toBeVisible();
  await expect(screen.getByText(MISSING)).toBeHidden();
});

test('members who cannot change the GitHub App do not see it', async ({ app, screen }) => {
  await app.open(mock('/s/studio/git', '&role=member'));
  await expect(screen.getByRole('heading', { level: 1 })).toHaveText('Git');
  await expect(screen.getByRole('heading', /^Delivery needs you/)).toBeVisible();
  await expect(screen.getByText(MISSING)).toBeHidden();
});
