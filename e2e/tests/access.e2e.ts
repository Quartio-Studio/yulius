import { test } from '@e2e-dev/web';
import { expect } from 'e2e';
import { failOnPageErrors, mock } from './support.ts';

failOnPageErrors();

// The home page's notice for accounts without GitHub (crates/yulius-server/web/assets/pages/home.js).
const GITHUB_GATE = /Sandbox content needs a GitHub-linked account/;

// SPEC-001 ACC-012 (F 2026-10-09): the superadmin owns the instance and sees every sandbox
// without a GitHub link; everyone else still needs GitHub to see any.
test('the superadmin without a GitHub link sees every sandbox and no GitHub gate', async ({ app, screen }) => {
  await app.open(mock('/', '&github_linked=0'));

  // The home screen (UI-045) lists them, as do the sandbox tabs.
  await expect(screen.getByRole('heading', { level: 1 })).toHaveText('Sandboxes');
  await expect(screen.getByRole('article', 'studio')).toBeVisible();
  await expect(screen.getByRole('article', 'product')).toBeVisible();
  const sandboxes = screen.getByRole('navigation', 'Sandboxes');
  await expect(sandboxes.getByRole('link', /^studio\b/)).toBeVisible();
  await expect(sandboxes.getByRole('link', /^product\b/)).toBeVisible();
  await expect(screen.getByText(GITHUB_GATE)).toBeHidden();
});

test('an instance admin without a GitHub link sees no sandbox and is asked to sign in with GitHub', async ({ app, screen }) => {
  await app.open(mock('/', '&role=admin&github_linked=0'));

  await expect(screen.getByRole('heading', { level: 1 })).toHaveText('No sandboxes yet');
  await expect(screen.getByText(GITHUB_GATE)).toBeVisible();
  await expect(screen.getByRole('link', 'Sign in with GitHub')).toBeVisible();
});
