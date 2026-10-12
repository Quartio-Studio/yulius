import { test } from '@e2e-dev/web';
import { expect } from 'e2e';
import { axeViolations, failOnPageErrors, mock } from './support.ts';

failOnPageErrors();

// The Cockpit (SPEC-021 CKP-008): every user's own plain Claude Code session, first in the sandbox
// navigation, for quick questions and work that should not become a task.

const cockpitTerminal = '.cockpit-term .xterm-rows';

test('Cockpit is first in the sandbox navigation and opens its session', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/t/12'));
  const nav = screen.getByRole('navigation', 'Sandbox');
  await expect(browser.locator('nav.nav .nav-item').first()).toHaveText('Cockpit');

  await nav.getByRole('link', 'Cockpit').click();
  await expect(browser).toHaveURL(/#\/s\/studio\/cockpit$/);
  await expect(screen.getByRole('heading', { level: 1 })).toHaveText('Cockpit');
  await expect(nav.getByRole('link', 'Cockpit')).toHaveAttribute('aria-current', 'page');
  const terminal = browser.locator(cockpitTerminal);
  await expect(terminal).toHaveText(/Welcome to Claude Code/);

  // The keyboard is in the session: a quick question gets an answer.
  await expect(browser.locator('.cockpit-term .xterm-helper-textarea')).toBeFocused();
  await browser.keyboard.type('why?');
  await browser.keyboard.press('Enter');
  await expect(terminal).toHaveText(/Good question/);
  expect(await axeViolations(browser)).toEqual([]);

  // It is not a terminal of the Shell panel, and leaving and coming back finds the same session.
  await nav.getByRole('link', 'Tasks').click();
  await nav.getByRole('link', 'Cockpit').click();
  await expect(terminal).toHaveText(/Good question/);
});

test('a stopped Cockpit starts again with its conversation', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/cockpit'));
  const terminal = browser.locator(cockpitTerminal);
  await expect(terminal).toHaveText(/Welcome to Claude Code/);
  await browser.keyboard.type('/exit');
  await browser.keyboard.press('Enter');
  await expect(screen.getByText('The Cockpit stopped. Its conversation is kept.')).toBeVisible();

  await screen.getByRole('button', 'Start again').click();
  await expect(terminal).toHaveText(/resumed your last conversation/);
});

test('a Cockpit whose process is gone is started again by itself', async ({ app, browser }) => {
  // The page got a Cockpit that no longer runs (the machine rebooted): it does not stay stuck on it.
  await app.open(mock('/s/studio/cockpit', '&cockpit=gone'));
  await expect(browser.locator(cockpitTerminal)).toHaveText(/Welcome to Claude Code/);
});

test('a Cockpit that cannot start says why and offers to try again', async ({ app, screen }) => {
  await app.open(mock('/s/studio/cockpit', '&cockpit=fail'));
  await expect(screen.getByRole('alert')).toHaveText(/Couldn’t start the Cockpit: no subscription has capacity right now/);
  await expect(screen.getByRole('button', 'Try again')).toBeVisible();
});

test('read access explains why the Cockpit is not available', async ({ app, browser, screen }) => {
  await app.open(mock('/s/sandbox-3/cockpit'));
  await expect(screen.getByText('The Cockpit needs write access to this sandbox.')).toBeVisible();
  await expect(browser.locator(cockpitTerminal)).toBeHidden();
});
