import { test } from '@e2e-dev/web';
import { expect } from 'e2e';
import { axeViolations, failOnPageErrors, firstShown, mock, openSuperagent } from './support.ts';

failOnPageErrors();

// The Global Superagent (SPEC-427 GSA-006): outside a sandbox the tools rail has one tool, the
// person's Global Superagent, which sees every sandbox of theirs; in a sandbox the rail's
// Superagent stays that sandbox's.

const shownAgent = '.panel-view:not([hidden]) > .agent';

test('outside a sandbox the rail opens the Global Superagent, which stays open across those pages', async ({ app, browser, screen }) => {
  await app.open(mock('/'));
  await firstShown(screen.getByRole('heading', { level: 1 }));
  await openSuperagent(browser, screen);
  const rail = screen.getByRole('navigation', 'Tools');
  // Its only tool here.
  expect(await browser.evaluate(() => [...document.querySelectorAll('nav.rail button')]
    .filter((b) => getComputedStyle(b).display !== 'none')
    .map((b) => b.getAttribute('aria-label')))).toEqual(['Superagent']);
  await expect(rail.getByRole('button', 'Superagent')).toHaveAttribute('aria-pressed', 'true');
  await expect(browser.locator('.panel .panel-title')).toHaveText('Global Superagent');
  const note = screen.getByRole('note', 'About the Global Superagent');
  await expect(note).toHaveText(/Sees all your sandboxes.*in sync/);
  await expect(note).toHaveText(/Code work still belongs in a task/);
  const terminal = browser.locator(`${shownAgent} .xterm-rows`);
  await expect(terminal).toHaveText(/Welcome to Claude Code/);
  await expect(terminal).toHaveText(/superagent\/work/);
  await browser.keyboard.type('what runs where?');
  await browser.keyboard.press('Enter');
  await expect(terminal).toHaveText(/Good question/);
  expect(await axeViolations(browser)).toEqual([]);

  // Settings is outside a sandbox too: the same session stays beside it.
  await app.open(mock('/settings'));
  await expect(screen.getByRole('heading', { level: 1 })).toHaveText('Settings');
  await expect(browser.locator('.panel .panel-title')).toHaveText('Global Superagent');
  await expect(terminal).toHaveText(/Good question/);
});

test('in a sandbox the rail opens that sandbox\'s Superagent, whose note links to the Global one', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/t/12'));
  await firstShown(screen.getByRole('heading', { level: 1 }), '#12 Checkout API');
  await openSuperagent(browser, screen);
  await expect(browser.locator('.panel .panel-title')).toHaveText('Superagent');
  const note = screen.getByRole('note', 'About the Superagent');
  await expect(note).toHaveText(/quick work/i);
  await expect(browser.locator(`${shownAgent} .xterm-rows`)).toHaveText(/sandboxes\/studio\/repo/);

  await note.getByRole('link', 'Global Superagent').click();
  await expect(browser).toHaveURL(/#\/superagent$/);
  await expect(browser.locator('.panel .panel-title')).toHaveText('Global Superagent');
  await expect(browser.locator(`${shownAgent} .xterm-rows`)).toHaveText(/superagent\/work/);
});

test('/superagent opens All sandboxes with the Global Superagent', async ({ app, browser, screen }) => {
  await app.open(mock('/superagent'));
  await expect(browser.locator('.panel .panel-title')).toHaveText('Global Superagent');
  await expect(browser.locator(`${shownAgent} .xterm-rows`)).toHaveText(/Welcome to Claude Code/);
});
