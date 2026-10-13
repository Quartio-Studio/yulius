import { test } from '@e2e-dev/web';
import { expect } from 'e2e';
import { axeViolations, failOnPageErrors, mock } from './support.ts';

failOnPageErrors();

// Ready-made automations (SPEC-042 AUT-009, AC-AUT-8): the Automations page offers the
// automations Yulius ships, each enabled with one click, after which it is an ordinary automation.

test('a ready-made automation is enabled in one click and joins the list', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/automations'));
  await expect(screen.getByRole('heading', { level: 1 })).toHaveText('Automations');
  await expect(screen.getByRole('heading', /^Ready-made/)).toBeVisible();
  await expect(screen.getByText('Keep the docs up to date')).toBeVisible();
  await expect(screen.getByText('Work through open issues')).toBeVisible();
  expect(await axeViolations(browser)).toEqual([]);

  await screen.getByRole('button', 'Enable Keep the docs up to date').click();
  await expect(screen.getByText('Keep the docs up to date enabled.')).toBeVisible();
  // Now an ordinary automation: its schedule switch is on and it is no longer suggested.
  await expect(screen.getByRole('switch', 'Keep the docs up to date runs on schedule')).toBeChecked();
  await expect(screen.getByRole('button', 'Enable Keep the docs up to date')).toBeHidden();
  await expect(screen.getByRole('button', 'Enable Work through open issues')).toBeVisible();

  await screen.getByRole('button', 'Enable Work through open issues').click();
  await expect(screen.getByRole('switch', 'Work through open issues runs on schedule')).toBeChecked();
  // Nothing left to suggest: the section goes away.
  await expect(screen.getByRole('heading', /^Ready-made/)).toBeHidden();
});
