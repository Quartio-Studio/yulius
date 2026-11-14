import { test } from '@e2e-dev/web';
import { expect } from 'e2e';
import { axeViolations, failOnPageErrors, mock } from './support.ts';

failOnPageErrors();

// Default workflows (SPEC-001 AUT-006, UI-034; #136): they ship with the server, come first, are
// read-only and are customised by duplicating them.

test('the defaults come first and read-only; Duplicate starts a custom copy', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/workflows'));
  await expect(screen.getByRole('heading', { level: 1 })).toHaveText('Workflows');
  await expect(screen.getByRole('heading', /^Defaults/)).toBeVisible();
  await expect(screen.getByRole('heading', /^Custom/)).toBeVisible();
  for (const name of ['Feature', 'Fix', 'Research']) {
    await expect(screen.getByRole('button', `Open workflow ${name}`)).toBeVisible();
    await expect(screen.getByRole('button', `Delete ${name}`)).toBeHidden();
  }
  await expect(screen.getByText('Use when: The task adds or changes behaviour that users or other code rely on.')).toBeVisible();
  await expect(screen.getByRole('button', 'Delete Release notes')).toBeVisible();

  // A default opens read-only, offering only a copy.
  await screen.getByRole('button', 'Open workflow Feature').click();
  await expect(browser).toHaveURL(/#\/s\/studio\/workflows\/wf-feature$/);
  await expect(screen.getByText(/A default workflow: it ships with Yulius and stays current with each release\./)).toBeVisible();
  await expect(screen.getByRole('button', 'Save')).toBeHidden();
  await expect(screen.getByRole('button', 'Delete')).toBeHidden();
  expect(await axeViolations(browser)).toEqual([]);

  await screen.getByRole('button', 'Duplicate to customise').click();
  await expect(browser).toHaveURL(/#\/s\/studio\/workflows\/new\?from=wf-feature$/);
  await expect(screen.getByRole('textbox', 'Workflow name')).toHaveValue('Feature (custom)');
  await expect(screen.getByRole('button', 'Write the spec')).toBeVisible();
  await screen.getByRole('button', 'Create workflow').click();
  await expect(browser).toHaveURL(/#\/s\/studio\/workflows$/);
  await expect(screen.getByRole('button', 'Edit workflow Feature (custom)')).toBeVisible();
  await expect(screen.getByRole('button', 'Delete Feature (custom)')).toBeVisible();
});
