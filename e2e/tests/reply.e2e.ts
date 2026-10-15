import { test, type Browser } from '@e2e-dev/web';
import { expect } from 'e2e';
import { axeViolations, failOnPageErrors, mock, terminalShown, touchTask } from './support.ts';

failOnPageErrors();

// SPEC-156 (#516, task #147: "when i click reply, nothing happens"): Reply in the attention bar
// opens a field whose text reaches the agent as a typed prompt.

/** The task terminal's text. */
const taskTerminal = (browser: Browser) => browser.locator('.taskview-term .xterm-rows');

/** What task #12's mock session has received as typed input so far (before Enter). */
const typedIntoTask = (browser: Browser) =>
  browser.evaluate(() => (window as unknown as { __yuliusMock: { sessions: Map<string, { input: string }> } }).__yuliusMock.sessions.get('tsk_studio_12')?.input ?? '');

/** The focused element's accessible name and the caret, JSON-safe. */
const focused = (browser: Browser) => browser.evaluate(() => {
  const active = document.activeElement as HTMLInputElement | null;
  return { label: active?.getAttribute('aria-label') ?? null, caret: active?.selectionStart ?? null };
});

const askRuleset = (browser: Browser) => touchTask(browser, 'sb_studio', 12, {
  needs_you: { action: 'Add a ruleset', why: 'agents can push to main', at: new Date().toISOString() },
});

test('Reply on Needs you opens a field whose text the agent gets', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/t/12'));
  await terminalShown(browser);
  await askRuleset(browser);
  const bar = browser.locator('.attention-bar');
  await expect(bar).toHaveText(/Needs you: Add a ruleset/);

  await screen.getByRole('button', 'Reply').click();
  const field = screen.getByRole('textbox', 'Reply to the agent');
  await expect(field).toBeVisible();
  await expect.poll(() => focused(browser)).toEqual({ label: 'Reply to the agent', caret: 0 });
  expect(await axeViolations(browser)).toEqual([]);

  // The bar is rebuilt on every task update: the text, the keyboard and the caret stay.
  await browser.keyboard.type('Ruleset added');
  await touchTask(browser, 'sb_studio', 12, {
    needs_you: { action: 'Add a ruleset on main', why: 'agents can push to main', at: new Date().toISOString() },
  });
  await expect(browser.locator('.attention-bar')).toHaveText(/Needs you: Add a ruleset on main/);
  await expect.poll(() => focused(browser)).toEqual({ label: 'Reply to the agent', caret: 13 });
  await expect(field).toHaveValue('Ruleset added');

  await browser.keyboard.press('Enter');
  await expect(taskTerminal(browser)).toHaveText(/> Ruleset added/);
  await expect(field).toBeHidden();
});

test('Escape closes the reply field and sends nothing', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/t/12'));
  await terminalShown(browser);
  await askRuleset(browser);

  await screen.getByRole('button', 'Reply').click();
  const field = screen.getByRole('textbox', 'Reply to the agent');
  await expect.poll(() => focused(browser)).toEqual({ label: 'Reply to the agent', caret: 0 });
  await browser.keyboard.type('Not yet');
  await browser.keyboard.press('Escape');
  await expect(field).toBeHidden();
  await expect(screen.getByRole('button', 'Reply')).toBeVisible();
  expect(await typedIntoTask(browser)).toBe('');

  // The words wait for the next Reply; Cancel closes it too.
  await screen.getByRole('button', 'Reply').click();
  await expect(field).toHaveValue('Not yet');
  await screen.getByRole('button', 'Cancel').click();
  await expect(field).toBeHidden();
  expect(await typedIntoTask(browser)).toBe('');
});

test('Reply on a task waiting for your reply opens the same field; Send sends', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/t/12'));
  await terminalShown(browser);
  await touchTask(browser, 'sb_studio', 12, { activity: 'waiting', background_shells: 0 });
  await expect(browser.locator('.attention-bar')).toHaveText(/waiting for your reply/);

  await screen.getByRole('button', 'Reply').click();
  const field = screen.getByRole('textbox', 'Reply to the agent');
  await expect(field).toBeVisible();
  // Empty text sends nothing and keeps the field open.
  await screen.getByRole('button', 'Send').click();
  await expect(field).toBeVisible();
  expect(await typedIntoTask(browser)).toBe('');
  await field.fill('Go on');
  await screen.getByRole('button', 'Send').click();
  await expect(taskTerminal(browser)).toHaveText(/> Go on/);
  await expect(field).toBeHidden();
});

// Review of SPEC-156: a Needs you request stays while the agent asks something in the terminal (a
// permission menu). Text typed into a field there would pick the menu's options unseen, so Reply
// goes to the terminal, where the menu is.
test('Reply on Needs you while the agent asks in the terminal focuses the terminal', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/t/12'));
  await terminalShown(browser);
  await askRuleset(browser);
  await touchTask(browser, 'sb_studio', 12, { activity: 'needs_attention' });
  await expect(browser.locator('.attention-bar')).toHaveText(/Needs you: Add a ruleset/);
  await screen.getByRole('button', 'Reply').click();
  await expect.poll(() => focused(browser)).toEqual({ label: 'Terminal input', caret: 0 });
  await expect(screen.getByRole('textbox', 'Reply to the agent')).toBeHidden();
});

// Review round 2: the keys typed after it closes must not reach the terminal, where a permission
// menu may have opened (Enter approves, a digit picks an option).
test('a reply field that closes while typing keeps the keys out of the terminal', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/t/12'));
  await terminalShown(browser);
  await askRuleset(browser);
  await screen.getByRole('button', 'Reply').click();
  await expect.poll(() => focused(browser)).toEqual({ label: 'Reply to the agent', caret: 0 });
  await browser.keyboard.type('Hal');
  await touchTask(browser, 'sb_studio', 12, { activity: 'needs_attention' });
  const field = screen.getByRole('textbox', 'Reply to the agent');
  await expect(field).toBeHidden();
  // The keyboard rests on the bar, which does nothing with keys.
  await expect.poll(() => browser.evaluate(() => document.activeElement?.classList.contains('attention-bar') ?? false)).toBe(true);
  await browser.keyboard.type('lo 2');
  expect(await typedIntoTask(browser)).toBe('');
  // The words wait, hidden, for the next Reply.
  expect(await browser.evaluate(() => document.querySelector<HTMLInputElement>('.attention-reply input')?.value ?? null)).toBe('Hal');
});

test('Reply on a phone focuses the quick input; no field opens', async ({ app, browser, screen }) => {
  await browser.setViewport({ width: 390, height: 844 });
  await app.open(mock('/s/studio/t/12'));
  await terminalShown(browser);
  await askRuleset(browser);
  await screen.getByRole('button', 'Reply').click();
  await expect.poll(() => focused(browser)).toEqual({ label: 'Quick input', caret: 0 });
  await expect(screen.getByRole('textbox', 'Reply to the agent')).toBeHidden();
});
