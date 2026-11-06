import { test } from '@e2e-dev/web';
import { expect } from 'e2e';
import type { Browser, Screen } from '@e2e-dev/web';
import { axeViolations, DRAG_GRIP, DRAG_HINT, dragTab, failOnPageErrors, firstShown, mock, openSuperagent, tabGrips, tabMenu, tabNames, tabsInPlace } from './support.ts';

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
  // Its person types into it as into the sandbox Superagent: no other box (SPEC-430 GKY-003).
  await expect(browser.locator(`${shownAgent} .xterm-helper-textarea`)).toBeFocused();
  await browser.keyboard.type('what runs where?');
  await browser.keyboard.press('Enter');
  await expect(terminal).toHaveText(/Good question/);
  expect(await browser.locator('.agent textarea:not(.xterm-helper-textarea)').count()).toBe(0);
  expect(await browser.locator(`${shownAgent} .term-banner-readonly`).count()).toBe(0);
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
  // The example of what it is for (SAT-007), in its note now that it has no box (SPEC-430).
  await expect(screen.getByRole('note', 'About the Global Superagent'))
    .toHaveText(/For example: check what my sandboxes need from each other and solve it\./);
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
  await browser.keyboard.press('Home');
  await expect(here).toHaveAttribute('aria-selected', 'true');
  await expect(here).toBeFocused();
  await browser.keyboard.press('ArrowRight');
  await expect(all).toHaveAttribute('aria-selected', 'true');
  await expect(all).toBeFocused();
});

test('outside a sandbox the keys skip the disabled This sandbox', async ({ app, browser, screen }) => {
  await app.open(mock('/superagent'));
  const all = tabs(screen).getByRole('tab', 'All sandboxes');
  await expect(all).toHaveAttribute('aria-selected', 'true');
  // Its terminal takes the keyboard once it shows, as This sandbox's does (SPEC-430).
  await expect(browser.locator(`${shownAgent} .xterm-helper-textarea`)).toBeFocused();
  await all.focus();
  await browser.keyboard.press('ArrowLeft');
  await expect(all).toHaveAttribute('aria-selected', 'true');
  await expect(all).toBeFocused();
  await expect(tabs(screen).getByRole('tab', 'This sandbox')).toHaveAttribute('aria-selected', 'false');
});

test('All sandboxes stays chosen in every sandbox and after a reload, with one Global session and no sandbox one', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/t/12'));
  await firstShown(screen.getByRole('heading', { level: 1 }), '#12 Checkout API');
  await openSuperagent(browser, screen);
  await tabs(screen).getByRole('tab', 'All sandboxes').click();
  await expect(browser.locator(`${shownAgent} .xterm-rows`)).toHaveText(/superagent\/work/);

  await screen.getByRole('navigation', 'Sandboxes').getByRole('link', /^product\b/).click();
  await expect(screen.getByRole('heading', { level: 1 })).toHaveText('New task');
  await expect(tabs(screen).getByRole('tab', 'All sandboxes')).toHaveAttribute('aria-selected', 'true');
  await expect(browser.locator(`${shownAgent} .xterm-rows`)).toHaveText(/superagent\/work/);
  // One Global view for the page (SAT-004).
  expect(await browser.locator('.superagent-pane [aria-label="About the Global Superagent"]').count()).toBe(1);
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
  // Out of the sandboxes it stays open too, whatever another browser tab picked meanwhile.
  await browser.evaluate(() => window.localStorage.setItem('yulius.superagentTab', '"sandbox"'));
  await app.open(mock('/settings'));
  await expect(screen.getByRole('heading', { level: 1 })).toHaveText('Settings');
  await expect(tabs(screen).getByRole('tab', 'All sandboxes')).toHaveAttribute('aria-selected', 'true');
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
  // Shown for the new page, not by the person: the keyboard stays with the page, and nothing was
  // picked, so nothing is remembered (review round 1).
  await expect(screen.getByRole('textbox', 'Task prompt')).toBeFocused();
  expect(await browser.evaluate(() => window.localStorage.getItem('yulius.superagentTab'))).toBeNull();
  // A choice another browser tab stored meanwhile changes nothing here: a new page, another sandbox.
  await browser.evaluate(() => window.localStorage.setItem('yulius.superagentTab', '"sandbox"'));
  await app.open(mock('/s/studio/t/12'));
  await firstShown(screen.getByRole('heading', { level: 1 }), '#12 Checkout API');
  await expect(tabs(screen).getByRole('tab', 'All sandboxes')).toHaveAttribute('aria-selected', 'true');
  await screen.getByRole('navigation', 'Sandboxes').getByRole('link', /^product\b/).click();
  await expect(screen.getByRole('heading', { level: 1 })).toHaveText('New task');
  await expect(tabs(screen).getByRole('tab', 'All sandboxes')).toHaveAttribute('aria-selected', 'true');
  // Closed and opened again from the rail, it shows the tab it had.
  await openSuperagent(browser, screen);
  await expect(screen.getByRole('region', 'Tool panel')).toBeHidden();
  await openSuperagent(browser, screen);
  await expect(tabs(screen).getByRole('tab', 'All sandboxes')).toHaveAttribute('aria-selected', 'true');
  expect(await startsOf(browser)).toEqual({ sandbox: 0, global: 1 });
  // /s/<slug>/superagent asks for the sandbox's own.
  await app.open(mock('/s/studio/superagent'));
  await expect(tabs(screen).getByRole('tab', 'This sandbox')).toHaveAttribute('aria-selected', 'true');
  await expect(browser.locator(`${shownAgent} .xterm-rows`)).toHaveText(/sandboxes\/studio\/repo/);
});

test('the Global Superagent\'s session follows its panel\'s size', async ({ app, browser, screen }) => {
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

test('the Global Superagent in front stays in front in a sandbox that had another tool open', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/t/12'));
  await firstShown(screen.getByRole('heading', { level: 1 }), '#12 Checkout API');
  if (await browser.locator('.rail:not([hidden])').count() === 0) await screen.getByRole('button', /^Show tools/).click();
  await screen.getByRole('navigation', 'Tools').getByRole('button', 'Notes').click();
  await expect(browser.locator('.panel .panel-title')).toHaveText('Notes');
  await app.open(mock('/superagent'));
  await expect(browser.locator(`${shownAgent} .xterm-rows`)).toHaveText(/superagent\/work/);
  // studio remembered Notes (SPEC-113 KST-005); the Global Superagent in front wins (SAT-005).
  await screen.getByRole('navigation', 'Sandboxes').getByRole('link', /^studio\b/).click();
  await expect(tabs(screen).getByRole('tab', 'All sandboxes')).toHaveAttribute('aria-selected', 'true');
  await expect(browser.locator('.panel .panel-title')).toHaveText('Superagent');
  expect(await startsOf(browser)).toEqual({ sandbox: 0, global: 1 });
});

// Review round 2: the tab picked last shows in every sandbox, also in a panel that was not shown when
// it was picked.
test('a sandbox\'s panel shown again shows the tab picked last elsewhere', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/t/12'));
  await firstShown(screen.getByRole('heading', { level: 1 }), '#12 Checkout API');
  await openSuperagent(browser, screen);
  await expect(tabs(screen).getByRole('tab', 'This sandbox')).toHaveAttribute('aria-selected', 'true');
  const sandboxes = screen.getByRole('navigation', 'Sandboxes');
  await sandboxes.getByRole('link', /^product\b/).click();
  await expect(screen.getByRole('heading', { level: 1 })).toHaveText('New task');
  await expect(tabs(screen).getByRole('tab', 'This sandbox')).toHaveAttribute('aria-selected', 'true');
  await sandboxes.getByRole('link', /^studio\b/).click();
  await tabs(screen).getByRole('tab', 'All sandboxes').click();
  await expect(browser.locator(`${shownAgent} .xterm-rows`)).toHaveText(/superagent\/work/);
  await openSuperagent(browser, screen);
  await expect(screen.getByRole('region', 'Tool panel')).toBeHidden();
  // product's remembered tool (SPEC-113 KST-005) comes back on All sandboxes.
  await sandboxes.getByRole('link', /^product\b/).click();
  await expect(tabs(screen).getByRole('tab', 'All sandboxes')).toHaveAttribute('aria-selected', 'true');
  // Back to This sandbox in product, closed; studio's panel opened from the rail follows.
  await tabs(screen).getByRole('tab', 'This sandbox').click();
  await openSuperagent(browser, screen);
  await expect(screen.getByRole('region', 'Tool panel')).toBeHidden();
  await sandboxes.getByRole('link', /^studio\b/).click();
  await expect(screen.getByRole('region', 'Tool panel')).toBeHidden();
  await openSuperagent(browser, screen);
  await expect(tabs(screen).getByRole('tab', 'This sandbox')).toHaveAttribute('aria-selected', 'true');
});

test('the tab keys with a modifier do not switch the Superagent tabs', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/t/12'));
  await firstShown(screen.getByRole('heading', { level: 1 }), '#12 Checkout API');
  await openSuperagent(browser, screen);
  const here = tabs(screen).getByRole('tab', 'This sandbox');
  await expect(here).toHaveAttribute('aria-selected', 'true');
  await here.focus();
  await browser.keyboard.press('Alt+ArrowRight');
  await browser.keyboard.press('Control+End');
  await expect(here).toHaveAttribute('aria-selected', 'true');
});

// SPEC-434 MTB-001/004: the two tabs move as the sandbox and Notes tabs do, and say so.
const strip = '.panel-view:not([hidden]) [role="tablist"][aria-label="Superagents"]';

test('the Superagent tabs move by drag, keyboard and menu and stay so', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/t/12', '&ambient=0'));
  await firstShown(screen.getByRole('heading', { level: 1 }), '#12 Checkout API');
  await openSuperagent(browser, screen);
  const here = tabs(screen).getByRole('tab', 'This sandbox');
  const all = tabs(screen).getByRole('tab', 'All sandboxes');
  await expect(here).toHaveAttribute('aria-selected', 'true');
  expect(await tabNames(browser, strip)).toEqual(['This sandbox', 'All sandboxes']);
  expect(await tabGrips(browser, strip)).toEqual([{ title: DRAG_HINT, grip: DRAG_GRIP }, { title: DRAG_HINT, grip: DRAG_GRIP }]);
  await expect(here).toHaveAttribute('aria-keyshortcuts', 'Alt+ArrowLeft Alt+ArrowRight');

  // A drag moves All sandboxes first and chooses nothing.
  await dragTab(browser, all, here);
  await expect.poll(() => tabNames(browser, strip)).toEqual(['All sandboxes', 'This sandbox']);
  await expect(here).toHaveAttribute('aria-selected', 'true');
  // Both settle side by side: the tab slid aside is not left shifted (review round 1).
  await expect.poll(() => tabsInPlace(browser, strip)).toBe(true);

  // The chosen tab, dragged, rides above with the dragged tabs' background (review round 1).
  const box = await here.boundingBox();
  if (!box) throw new Error('tab not laid out');
  await browser.mouse.move(box.x + 12, box.y + box.height / 2);
  await browser.mouse.down();
  await browser.mouse.move(box.x + 40, box.y + box.height / 2);
  expect(await browser.evaluate((selector) => {
    const tab = document.querySelector(`${selector} .term-tab.dragging`) as HTMLElement;
    const probe = document.body.appendChild(document.createElement('div'));
    probe.style.background = 'var(--surface-2)';
    const want = getComputedStyle(probe).backgroundColor;
    probe.remove();
    return { key: tab.dataset.key, same: getComputedStyle(tab).backgroundColor === want, cursor: getComputedStyle(tab).cursor };
  }, strip)).toEqual({ key: 'sandbox', same: true, cursor: 'grabbing' });
  await browser.keyboard.press('Escape');
  await browser.mouse.up();
  expect(await tabNames(browser, strip)).toEqual(['All sandboxes', 'This sandbox']);
  await expect.poll(() => tabsInPlace(browser, strip)).toBe(true);
  expect(await startsOf(browser)).toEqual({ sandbox: 1, global: 0 });

  // The arrows follow the order shown: Home is All sandboxes now.
  await here.focus();
  await browser.keyboard.press('Home');
  await expect(all).toHaveAttribute('aria-selected', 'true');
  await expect(all).toBeFocused();
  await browser.keyboard.press('ArrowRight');
  await expect(here).toHaveAttribute('aria-selected', 'true');

  // Alt+←/→ moves the focused tab and keeps the keyboard on it; an end goes no further.
  await browser.keyboard.press('Alt+ArrowLeft');
  expect(await tabNames(browser, strip)).toEqual(['This sandbox', 'All sandboxes']);
  await expect(here).toBeFocused();
  await expect(here).toHaveAttribute('aria-selected', 'true');
  await browser.keyboard.press('Alt+ArrowLeft');
  expect(await tabNames(browser, strip)).toEqual(['This sandbox', 'All sandboxes']);

  // The menu moves it without dragging (WCAG 2.5.7).
  await tabMenu(browser, `${strip} [data-key="sandbox"]`);
  await expect(screen.getByRole('menuitem', 'Move left')).toHaveAttribute('aria-disabled', 'true');
  await screen.getByRole('menuitem', 'Move right').click();
  expect(await tabNames(browser, strip)).toEqual(['All sandboxes', 'This sandbox']);
  expect(await axeViolations(browser)).toEqual([]);

  // Kept in another sandbox, after a reload and outside a sandbox, where the keys skip This sandbox.
  await screen.getByRole('navigation', 'Sandboxes').getByRole('link', /^product\b/).click();
  await expect(screen.getByRole('heading', { level: 1 })).toHaveText('New task');
  expect(await tabNames(browser, strip)).toEqual(['All sandboxes', 'This sandbox']);
  await app.open(mock('/superagent'));
  await expect(all).toHaveAttribute('aria-selected', 'true');
  expect(await tabNames(browser, strip)).toEqual(['All sandboxes', 'This sandbox']);
  await all.focus();
  await browser.keyboard.press('ArrowRight');
  await browser.keyboard.press('End');
  await expect(all).toHaveAttribute('aria-selected', 'true');
  await expect(all).toBeFocused();

  // A stored order that is not the two tabs is ignored.
  await browser.evaluate(() => localStorage.setItem('yulius.superagentOrder', JSON.stringify(['global', 'global'])));
  await browser.reload();
  await expect(all).toHaveAttribute('aria-selected', 'true');
  expect(await tabNames(browser, strip)).toEqual(['This sandbox', 'All sandboxes']);
});
