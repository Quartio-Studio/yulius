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
  needs_you: { action: 'Add a ruleset', problem: 'agents can push to main', decision: false, at: new Date().toISOString() },
});

test('Reply on Needs you opens a field whose text the agent gets', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/t/12'));
  await terminalShown(browser);
  await askRuleset(browser);
  const bar = browser.locator('.attention-bar');
  await expect(bar).toHaveText(/Needs you[\s\S]*To do[\s\S]*Add a ruleset/);

  await screen.getByRole('button', 'Reply').click();
  const field = screen.getByRole('textbox', 'Reply to the agent');
  await expect(field).toBeVisible();
  await expect.poll(() => focused(browser)).toEqual({ label: 'Reply to the agent', caret: 0 });
  expect(await axeViolations(browser)).toEqual([]);

  // The bar is rebuilt on every task update: the text, the keyboard and the caret stay.
  await browser.keyboard.type('Ruleset added');
  await touchTask(browser, 'sb_studio', 12, {
    needs_you: { action: 'Add a ruleset on main', problem: 'agents can push to main', decision: false, at: new Date().toISOString() },
  });
  await expect(browser.locator('.attention-bar')).toHaveText(/Needs you[\s\S]*To do[\s\S]*Add a ruleset on main/);
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
  await expect(browser.locator('.attention-bar')).toHaveText(/Needs you[\s\S]*To do[\s\S]*Add a ruleset/);
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

// Task #406 ("sometimes i cant click reply"): the bar is rebuilt on every task update, and a click
// is lost when its button is replaced between the press and the release.

/** Presses the mouse on the attention bar's Reply button and holds it. */
const pressReply = async (browser: Browser) => {
  const box = await browser.evaluate(() => {
    const button = [...document.querySelectorAll('.attention-bar button')].find((b) => b.textContent === 'Reply');
    const rect = button?.getBoundingClientRect();
    return rect ? { x: rect.x + rect.width / 2, y: rect.y + rect.height / 2 } : null;
  });
  if (!box) throw new Error('the attention bar has no Reply button');
  await browser.mouse.move(box.x, box.y);
  await browser.mouse.down();
};

/** Touches task #12 with a new title and waits until the view's strip shows it. */
const updateTitle = async (browser: Browser, title: string) => {
  await touchTask(browser, 'sb_studio', 12, { title });
  await expect(browser.locator('.taskview .step-task')).toContainText(title);
};

test('Reply pressed across a task update still focuses the terminal (#406)', async ({ app, browser }) => {
  await app.open(mock('/s/studio/t/12'));
  await terminalShown(browser);
  await touchTask(browser, 'sb_studio', 12, { activity: 'needs_attention' });
  await expect(browser.locator('.attention-bar .attention-text')).toHaveText('The agent needs your attention. Its question is in the terminal below.');
  // Away from the terminal first, so focusing it is the click's doing.
  await browser.evaluate(() => (document.activeElement as HTMLElement | null)?.blur());

  await pressReply(browser);
  await updateTitle(browser, 'Checkout API v2');
  await browser.mouse.up();
  await expect.poll(() => focused(browser)).toEqual({ label: 'Terminal input', caret: 0 });
});

test('Reply pressed across a task update opens the field; the update shows after (#406)', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/t/12'));
  await terminalShown(browser);
  await askRuleset(browser);
  const bar = browser.locator('.attention-bar');
  await expect(bar).toHaveText(/Needs you[\s\S]*To do[\s\S]*Add a ruleset/);

  await pressReply(browser);
  await touchTask(browser, 'sb_studio', 12, {
    needs_you: { action: 'Add a ruleset on main', problem: 'agents can push to main', decision: false, at: new Date().toISOString() },
  });
  await updateTitle(browser, 'Checkout API v2');
  await browser.mouse.up();
  const field = screen.getByRole('textbox', 'Reply to the agent');
  await expect(field).toBeVisible();
  await expect.poll(() => focused(browser)).toEqual({ label: 'Reply to the agent', caret: 0 });
  // The update held back during the press is shown once it ends.
  await expect(bar).toHaveText(/Add a ruleset on main/);
});

test('the keyboard stays on the terminal Reply across a task update (#406)', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/t/12'));
  await terminalShown(browser);
  await touchTask(browser, 'sb_studio', 12, { activity: 'needs_attention' });
  await expect(browser.locator('.attention-bar .attention-text')).toHaveText(/needs your attention/);
  await browser.evaluate(() => ([...document.querySelectorAll('.attention-bar button')].find((b) => b.textContent === 'Reply') as HTMLElement).focus());

  await updateTitle(browser, 'Checkout API v2');
  await expect.poll(() => browser.evaluate(() => (document.activeElement?.tagName === 'BUTTON' ? document.activeElement.textContent : document.activeElement?.tagName) ?? null)).toBe('Reply');
  await browser.keyboard.press('Enter');
  await expect.poll(() => focused(browser)).toEqual({ label: 'Terminal input', caret: 0 });
  await expect(screen.getByRole('textbox', 'Reply to the agent')).toBeHidden();
});

test('a press the window loses focus during does not hold the bar back (#406)', async ({ app, browser }) => {
  await app.open(mock('/s/studio/t/12'));
  await terminalShown(browser);
  await askRuleset(browser);
  const bar = browser.locator('.attention-bar');
  await expect(bar).toHaveText(/Needs you[\s\S]*To do[\s\S]*Add a ruleset/);

  await pressReply(browser);
  await touchTask(browser, 'sb_studio', 12, {
    needs_you: { action: 'Add a ruleset on main', problem: 'agents can push to main', decision: false, at: new Date().toISOString() },
  });
  await updateTitle(browser, 'Checkout API v2');
  // Held while pressed; the window losing focus (the release lands outside the page) ends the press.
  await expect(bar).not.toHaveText(/Add a ruleset on main/);
  await browser.evaluate(() => { window.dispatchEvent(new Event('blur')); });
  await expect(bar).toHaveText(/Add a ruleset on main/);
  await browser.mouse.up();
});

// Review of #406: a press whose release never comes (a context menu swallows it, a cancelled
// pointer, a release the page missed) must not hold the bar back.

/** Dispatches a synthetic pointer event of `type` at the bar's text (`init` merged in). */
const pointerAtBar = (browser: Browser, type: string, init: Record<string, unknown> = {}) =>
  browser.evaluate(([t, i]) => {
    const text = document.querySelector('.attention-bar .attention-text, .attention-bar .needs-you-text');
    if (!text) throw new Error('the attention bar has no text');
    text.dispatchEvent(new PointerEvent(t, { bubbles: true, cancelable: true, composed: true, pointerType: 'mouse', isPrimary: true, ...i }));
  }, [type, init] as const);

/** Asks the ruleset with `action` and waits until the view has had the update (strip retitled). */
const askWhilePressed = async (browser: Browser, action: string, title: string) => {
  await touchTask(browser, 'sb_studio', 12, {
    needs_you: { action, problem: 'agents can push to main', decision: false, at: new Date().toISOString() },
  });
  await updateTitle(browser, title);
};

test('a press ended by a context menu, a cancel or a buttonless move shows the held update (#406 review)', async ({ app, browser }) => {
  await app.open(mock('/s/studio/t/12'));
  await terminalShown(browser);
  await askRuleset(browser);
  const bar = browser.locator('.attention-bar');
  await expect(bar).toHaveText(/Add a ruleset/);
  const endings: [string, string, Record<string, unknown>][] = [
    ['contextmenu', 'Add a ruleset on main', {}],
    ['pointercancel', 'Add a ruleset on dev', {}],
    ['pointermove', 'Add a ruleset on both', { buttons: 0 }],
  ];
  for (const [index, [ending, action, init]] of endings.entries()) {
    await pointerAtBar(browser, 'pointerdown', { button: 0, buttons: 1 });
    await askWhilePressed(browser, action, `Checkout API v${index + 2}`);
    await expect(bar).not.toHaveText(new RegExp(`${action}$`));
    if (ending === 'contextmenu') {
      await browser.evaluate(() => { document.querySelector('.attention-bar')!.dispatchEvent(new MouseEvent('contextmenu', { bubbles: true, cancelable: true, button: 2 })); });
    } else {
      await pointerAtBar(browser, ending, init);
    }
    await expect(bar).toHaveText(new RegExp(action));
  }
});

test('a right press holds nothing back (#406 review)', async ({ app, browser }) => {
  await app.open(mock('/s/studio/t/12'));
  await terminalShown(browser);
  await askRuleset(browser);
  const bar = browser.locator('.attention-bar');
  await expect(bar).toHaveText(/Add a ruleset/);
  await pointerAtBar(browser, 'pointerdown', { button: 2, buttons: 2 });
  await askWhilePressed(browser, 'Add a ruleset on main', 'Checkout API v2');
  await expect(bar).toHaveText(/Add a ruleset on main/);
});

test('a tap across a task update, its click after the release, opens the field (#406 review)', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/t/12'));
  await terminalShown(browser);
  await askRuleset(browser);
  const bar = browser.locator('.attention-bar');
  await expect(bar).toHaveText(/Add a ruleset/);
  // A touch press and release on Reply, the update between them, and the tap's click after (a
  // tap's click comes with its gesture, after the release's task).
  await browser.evaluate(() => {
    const reply = [...document.querySelectorAll('.attention-bar button')].find((b) => b.textContent === 'Reply')!;
    (window as unknown as { __tapped: Element }).__tapped = reply;
    reply.dispatchEvent(new PointerEvent('pointerdown', { bubbles: true, composed: true, pointerType: 'touch', isPrimary: true, button: 0, buttons: 1 }));
  });
  await askWhilePressed(browser, 'Add a ruleset on main', 'Checkout API v2');
  await browser.evaluate(() => {
    const reply = (window as unknown as { __tapped: Element }).__tapped;
    reply.dispatchEvent(new PointerEvent('pointerup', { bubbles: true, composed: true, pointerType: 'touch', isPrimary: true, button: 0, buttons: 0 }));
  });
  await browser.evaluate(() => { ((window as unknown as { __tapped: HTMLElement }).__tapped).click(); });
  await expect(screen.getByRole('textbox', 'Reply to the agent')).toBeVisible();
  await expect.poll(() => focused(browser)).toEqual({ label: 'Reply to the agent', caret: 0 });
  await expect(bar).toHaveText(/Add a ruleset on main/);
});
