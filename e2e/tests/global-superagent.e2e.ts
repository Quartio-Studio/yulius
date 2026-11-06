import { test } from '@e2e-dev/web';
import { expect } from 'e2e';
import type { Browser, Screen } from '@e2e-dev/web';
import { axeViolations, failOnPageErrors, firstShown, mock, openSuperagent } from './support.ts';

failOnPageErrors();

// The Global Superagent (SPEC-427 GSA-006): outside a sandbox the tools rail has one tool, the
// person's Global Superagent, which sees every sandbox of theirs. The Superagent panel shows both
// Superagents as tabs, This sandbox and All sandboxes, as Notes shows its notepads (SPEC-428).

const shownAgent = '.panel-view:not([hidden]) .superagent-pane:not([hidden]) > .agent';
const startsOf = (browser: Browser) => browser.evaluate(() => {
  const db = (window as unknown as { __yuliusMock: { cockpitStarts?: number; globalOpens?: number } }).__yuliusMock;
  return { sandbox: db.cockpitStarts ?? 0, global: db.globalOpens ?? 0 };
});
const tabs = (screen: Screen) => screen.getByRole('region', 'Tool panel').getByRole('tablist', 'Superagents');

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
  await expect(browser.locator('.panel .panel-title')).toHaveText('Superagent');
  // Its tab, All sandboxes, is shown; This sandbox needs a sandbox (SPEC-428 SAT-003).
  await expect(tabs(screen).getByRole('tab', 'All sandboxes')).toHaveAttribute('aria-selected', 'true');
  await expect(tabs(screen).getByRole('tab', 'This sandbox')).toHaveAttribute('aria-disabled', 'true');
  const note = screen.getByRole('note', 'About the Global Superagent');
  await expect(note).toHaveText(/Sees all your sandboxes.*in sync/);
  await expect(note).toHaveText(/Code work still belongs in a task/);
  const terminal = browser.locator(`${shownAgent} .xterm-rows`);
  await expect(terminal).toHaveText(/Welcome to Claude Code/);
  await expect(terminal).toHaveText(/superagent\/work/);
  // It takes messages, not keys (GSA-007): the box has the keyboard.
  const box = screen.getByRole('textbox', 'Message to the Global Superagent');
  await expect(box).toBeFocused();
  await browser.keyboard.type('what runs where?');
  await browser.keyboard.press('Enter');
  await expect(terminal).toHaveText(/Good question/);
  await expect(box).toHaveValue('');
  await browser.keyboard.type('!cat /etc/passwd');
  await browser.keyboard.press('Enter');
  await expect(screen.getByRole('alert')).toHaveText(/runs no commands/);
  await expect(box).toHaveValue('!cat /etc/passwd');
  expect(await axeViolations(browser)).toEqual([]);

  // Settings is outside a sandbox too: the same session stays beside it.
  await app.open(mock('/settings'));
  await expect(screen.getByRole('heading', { level: 1 })).toHaveText('Settings');
  await expect(tabs(screen).getByRole('tab', 'All sandboxes')).toHaveAttribute('aria-selected', 'true');
  await expect(terminal).toHaveText(/Good question/);
});

test('in a sandbox the Superagent panel shows This sandbox and All sandboxes as tabs', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/t/12'));
  await firstShown(screen.getByRole('heading', { level: 1 }), '#12 Checkout API');
  await openSuperagent(browser, screen);
  await expect(browser.locator('.panel .panel-title')).toHaveText('Superagent');
  // This sandbox first, until the person picks the other (SAT-002).
  const here = tabs(screen).getByRole('tab', 'This sandbox');
  const all = tabs(screen).getByRole('tab', 'All sandboxes');
  await expect(here).toHaveAttribute('aria-selected', 'true');
  await expect(all).toHaveAttribute('aria-selected', 'false');
  await expect(screen.getByRole('note', 'About the Superagent')).toHaveText(/quick work/i);
  await expect(browser.locator(`${shownAgent} .xterm-rows`)).toHaveText(/sandboxes\/studio\/repo/);
  expect(await startsOf(browser)).toEqual({ sandbox: 1, global: 0 });

  await all.click();
  await expect(all).toHaveAttribute('aria-selected', 'true');
  await expect(screen.getByRole('note', 'About the Global Superagent')).toHaveText(/Sees all your sandboxes/);
  await expect(browser.locator(`${shownAgent} .xterm-rows`)).toHaveText(/superagent\/work/);
  // The example of what it is for (SAT-007).
  await expect(screen.getByRole('textbox', 'Message to the Global Superagent'))
    .toHaveAttribute('placeholder', 'For example: check what my sandboxes need from each other and solve it');
  expect(await axeViolations(browser)).toEqual([]);

  await here.click();
  await expect(here).toHaveAttribute('aria-selected', 'true');
  await expect(browser.locator(`${shownAgent} .xterm-rows`)).toHaveText(/sandboxes\/studio\/repo/);
  expect(await startsOf(browser)).toEqual({ sandbox: 1, global: 1 });
});

test('the sandbox note\'s All sandboxes selects that tab, and the arrow keys move between the tabs', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/t/12'));
  await firstShown(screen.getByRole('heading', { level: 1 }), '#12 Checkout API');
  await openSuperagent(browser, screen);
  const note = screen.getByRole('note', 'About the Superagent');
  await expect(note).toHaveText(/Work across sandboxes: the Global Superagent, in All sandboxes\./);
  await note.getByRole('button', 'All sandboxes').click();
  const all = tabs(screen).getByRole('tab', 'All sandboxes');
  await expect(all).toHaveAttribute('aria-selected', 'true');
  await expect(browser.locator(`${shownAgent} .xterm-rows`)).toHaveText(/superagent\/work/);
  // Still the sandbox's page.
  await expect(screen.getByRole('heading', { level: 1 })).toHaveText(/#12 Checkout API/);

  await all.focus();
  await browser.keyboard.press('ArrowLeft');
  const here = tabs(screen).getByRole('tab', 'This sandbox');
  await expect(here).toHaveAttribute('aria-selected', 'true');
  await expect(here).toBeFocused();
  await browser.keyboard.press('End');
  await expect(all).toHaveAttribute('aria-selected', 'true');
  await expect(all).toBeFocused();
});

test('All sandboxes stays chosen in every sandbox and after a reload, with one Global session and no sandbox one', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/t/12'));
  await firstShown(screen.getByRole('heading', { level: 1 }), '#12 Checkout API');
  await openSuperagent(browser, screen);
  await tabs(screen).getByRole('tab', 'All sandboxes').click();
  await expect(browser.locator(`${shownAgent} .xterm-rows`)).toHaveText(/superagent\/work/);
  // What is typed stays in its box from one sandbox to the next (SAT-004).
  await screen.getByRole('textbox', 'Message to the Global Superagent').fill('half a thought');

  await screen.getByRole('navigation', 'Sandboxes').getByRole('link', /^product\b/).click();
  await expect(screen.getByRole('heading', { level: 1 })).toHaveText('New task');
  await expect(tabs(screen).getByRole('tab', 'All sandboxes')).toHaveAttribute('aria-selected', 'true');
  await expect(browser.locator(`${shownAgent} .xterm-rows`)).toHaveText(/superagent\/work/);
  await expect(screen.getByRole('textbox', 'Message to the Global Superagent')).toHaveValue('half a thought');
  expect(await browser.locator('.superagent-pane .agent-compose').count()).toBe(1);
  // studio's own Superagent started when its tab showed; product's never did.
  expect(await startsOf(browser)).toEqual({ sandbox: 1, global: 1 });

  // Back in studio, whose own Superagent is not shown again.
  await app.open(mock('/s/studio/t/12'));
  await firstShown(screen.getByRole('heading', { level: 1 }), '#12 Checkout API');
  await expect(tabs(screen).getByRole('tab', 'All sandboxes')).toHaveAttribute('aria-selected', 'true');
  expect(await startsOf(browser)).toEqual({ sandbox: 1, global: 1 });

  // After a reload the rail opens it on the tab chosen, starting no sandbox Superagent.
  await browser.reload();
  await firstShown(screen.getByRole('heading', { level: 1 }), '#12 Checkout API');
  await openSuperagent(browser, screen);
  await expect(tabs(screen).getByRole('tab', 'All sandboxes')).toHaveAttribute('aria-selected', 'true');
  await expect(browser.locator(`${shownAgent} .xterm-rows`)).toHaveText(/superagent\/work/);
  expect(await startsOf(browser)).toEqual({ sandbox: 0, global: 1 });
});

test('/superagent opens All sandboxes with the Global Superagent', async ({ app, browser, screen }) => {
  await app.open(mock('/superagent'));
  await expect(tabs(screen).getByRole('tab', 'All sandboxes')).toHaveAttribute('aria-selected', 'true');
  await expect(browser.locator(`${shownAgent} .xterm-rows`)).toHaveText(/Welcome to Claude Code/);
});

test('a sandbox entered while the Global Superagent is open keeps it open and starts no Superagent of its own', async ({ app, browser, screen }) => {
  await app.open(mock('/superagent'));
  await expect(browser.locator(`${shownAgent} .xterm-rows`)).toHaveText(/Welcome to Claude Code/);
  await screen.getByRole('navigation', 'Sandboxes').getByRole('link', /^studio\b/).click();
  await expect(screen.getByRole('heading', { level: 1 })).toHaveText('New task');
  // SAT-005: the panel stays on All sandboxes.
  await expect(screen.getByRole('region', 'Tool panel')).toBeVisible();
  await expect(tabs(screen).getByRole('tab', 'All sandboxes')).toHaveAttribute('aria-selected', 'true');
  await expect(tabs(screen).getByRole('tab', 'This sandbox')).not.toHaveAttribute('aria-disabled', 'true');
  await expect(browser.locator(`${shownAgent} .xterm-rows`)).toHaveText(/superagent\/work/);
  expect(await startsOf(browser)).toEqual({ sandbox: 0, global: 1 });
  // /s/<slug>/superagent asks for the sandbox's own.
  await app.open(mock('/s/studio/superagent'));
  await expect(tabs(screen).getByRole('tab', 'This sandbox')).toHaveAttribute('aria-selected', 'true');
  await expect(browser.locator(`${shownAgent} .xterm-rows`)).toHaveText(/sandboxes\/studio\/repo/);
});

test('the Global Superagent\'s session follows its panel\'s size though it takes no keys', async ({ app, browser, screen }) => {
  await browser.setViewport({ width: 1400, height: 900 });
  await app.open(mock('/superagent'));
  await expect(browser.locator(`${shownAgent} .xterm-rows`)).toHaveText(/Welcome to Claude Code/);
  const cols = () => browser.evaluate(() => {
    type Shells = { __yuliusMock: { shells: Map<string, { global?: boolean; session?: { cols: number; sized?: boolean } }> } };
    const shell = [...(window as unknown as Shells).__yuliusMock.shells.values()].find((s) => s.global && s.session);
    return shell && shell.session && shell.session.sized ? shell.session.cols : 0;
  });
  await expect.poll(cols).toBeGreaterThan(20);
  const before = await cols();
  await screen.getByRole('button', 'Maximize panel').click();
  await expect.poll(cols).toBeGreaterThan(before + 10);
});
