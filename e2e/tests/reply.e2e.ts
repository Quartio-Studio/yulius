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

/** Sends `data` to task #12's session as typed input, through a terminal socket of the page. */
const sendBehind = (browser: Browser, data: string) => browser.evaluate((input) => {
  const session = (window as unknown as { __yuliusMock: { sessions: Map<string, { viewers: Set<{ send(data: string): void }> }> } }).__yuliusMock.sessions.get('tsk_studio_12');
  const [socket] = session ? [...session.viewers] : [];
  if (!socket) throw new Error('task #12 has no terminal socket');
  socket.send(JSON.stringify({ type: 'input', data: input }));
}, data);

/**
 * Types `text` into task #12's session behind the keys sent so far and waits until the session has
 * it. The page's sockets deliver in order (each a task later), so a key the page sent the terminal
 * before it (a leak) has arrived by then. Answers what the session holds as typed.
 */
const typeBehind = async (browser: Browser, text: string) => {
  await sendBehind(browser, text);
  await expect.poll(async () => (await typedIntoTask(browser)).endsWith(text)).toBe(true);
  return typedIntoTask(browser);
};

/**
 * No key reached task #12's session since it held `seed` (#524): with `marker` typed behind the
 * keys it holds exactly both, and its terminal shows no `> …` line of the seed (a leaked Enter
 * sends what the session holds and empties it) and no interruption (a leaked Escape).
 */
const nothingReached = async (browser: Browser, seed: string, marker: string) => {
  expect(await typeBehind(browser, marker)).toBe(`${seed}${marker}`);
  await expect(taskTerminal(browser)).toContainText(`❯ ${seed}${marker}`);
  await expect(taskTerminal(browser)).not.toContainText(`> ${seed}`);
  await expect(taskTerminal(browser)).not.toContainText('(esc) interrupted');
};

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
  expect(await typeBehind(browser, 'x')).toBe('x');
  await browser.keyboard.type('Not yet');
  await browser.keyboard.press('Escape');
  await expect(field).toBeHidden();
  await expect(screen.getByRole('button', 'Reply')).toBeVisible();
  await expect.poll(() => browser.evaluate(() => document.activeElement?.textContent ?? null)).toBe('Reply');
  await nothingReached(browser, 'x', 'y');
  // The bar is rebuilt on a task update (the mock's ambient activity touches #12 every 16 s): the
  // keyboard stays on Reply (#506, seen under load).
  await touchTask(browser, 'sb_studio', 12, { title: 'Checkout API v2' });
  await expect(browser.locator('.taskview .step-task')).toContainText('Checkout API v2');
  await expect.poll(() => browser.evaluate(() => document.activeElement?.textContent ?? null)).toBe('Reply');

  // The words wait for the next Reply; Cancel closes it too.
  await screen.getByRole('button', 'Reply').click();
  await expect(field).toHaveValue('Not yet');
  await screen.getByRole('button', 'Cancel').click();
  await expect(field).toBeHidden();
  await nothingReached(browser, 'xy', 'z');
});

test('Reply on a task waiting for your reply opens the same field; Send sends', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/t/12'));
  await terminalShown(browser);
  await touchTask(browser, 'sb_studio', 12, { activity: 'waiting', background_shells: 0 });
  await expect(browser.locator('.attention-bar')).toHaveText(/waiting for your reply/);

  await screen.getByRole('button', 'Reply').click();
  const field = screen.getByRole('textbox', 'Reply to the agent');
  await expect(field).toBeVisible();
  // Empty text sends nothing and keeps the field open, with the keyboard in it.
  expect(await typeBehind(browser, 'x')).toBe('x');
  await screen.getByRole('button', 'Send').click();
  await expect(field).toBeVisible();
  await expect.poll(() => focused(browser)).toEqual({ label: 'Reply to the agent', caret: 0 });
  await nothingReached(browser, 'x', 'y');
  await sendBehind(browser, '\x7f\x7f');
  await expect.poll(() => typedIntoTask(browser)).toBe('');
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
  // The keyboard rests on the bar, which does nothing with keys (#524: each check below waits on
  // what shows the keys were handled, and on a marker typed behind them, before it reads).
  const barHasKeyboard = () => browser.evaluate(() => document.activeElement?.classList.contains('attention-bar') ?? false);
  const hiddenWords = () => browser.evaluate(() => document.querySelector<HTMLInputElement>('.attention-reply input')?.value ?? null);
  await expect.poll(barHasKeyboard).toBe(true);
  await browser.keyboard.type('lo 2');
  await expect.poll(barHasKeyboard).toBe(true);
  await expect.poll(hiddenWords).toBe('Hal');
  // The session holds only the marker `x`, typed after the keys: none of them reached it.
  expect(await typeBehind(browser, 'x')).toBe('x');
  // Enter (which would approve a menu) sends nothing either: the session still holds `x` (an
  // Enter would have sent it as `> x`), and the words wait, hidden, for the next Reply.
  await browser.keyboard.press('Enter');
  await expect.poll(barHasKeyboard).toBe(true);
  await expect.poll(hiddenWords).toBe('Hal');
  await nothingReached(browser, 'x', 'y');
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
