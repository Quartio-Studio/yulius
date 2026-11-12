import { test, type Browser } from '@e2e-dev/web';
import { expect } from 'e2e';
import { axeViolations, failOnPageErrors, firstShown, mock, terminalShown, touchTask } from './support.ts';

failOnPageErrors();

// SPEC-472 (#1221, task #455: "I can't click reply and don't see any question"): a task flagged
// "needs attention" says why in its bar. Yulius raises that flag itself for reasons that are not
// a question in the terminal; only a flag of Claude Code's own question says it is there. Reply
// shows that the terminal took the keyboard.

const bar = (browser: Browser) => browser.locator('.attention-bar');
const notice = (browser: Browser) => browser.locator('.taskview .term-notice');

/** The focused element's accessible name. */
const focusedLabel = (browser: Browser) => browser.evaluate(() => document.activeElement?.getAttribute('aria-label') ?? null);

const GATE = {
  kind: 'yulius',
  problem: 'The agent stopped 3 times without fixing: Code changed without a valid spec. Write `docs/specs/checkout.md` first.',
  action: 'Tell it how to go on, in the terminal.',
  link: null,
};

test('a flag Yulius raised says why and what to do, not that a question is in the terminal', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/t/12', '&ambient=0'));
  await terminalShown(browser);
  await touchTask(browser, 'sb_studio', 12, { activity: 'needs_attention', attention: GATE });

  await expect(bar(browser)).toHaveText(/Needs you[\s\S]*Problem[\s\S]*The agent stopped 3 times without fixing: Code changed without a valid spec[\s\S]*To do[\s\S]*Tell it how to go on, in the terminal\./);
  await expect(bar(browser)).not.toHaveText(/question is in the terminal/);
  // Backticked names read as code, as in a request (SPEC-327).
  await expect(browser.locator('.attention-bar .needs-you-parts code')).toHaveText('docs/specs/checkout.md');
  await expect(screen.getByRole('button', 'Reply')).toBeVisible();
  await expect(browser.locator('.attention-bar a')).toHaveCount(0);
  expect(await axeViolations(browser)).toEqual([]);

  // The reason goes with its flag.
  await touchTask(browser, 'sb_studio', 12, { activity: 'working' });
  await expect(bar(browser)).toBeHidden();
});

test('a secret request says where a person answers it, with the link', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/t/12', '&ambient=0'));
  await terminalShown(browser);
  const link = `${await browser.evaluate(() => location.origin)}/s/studio/secrets?section=agents`;
  await touchTask(browser, 'sb_studio', 12, {
    activity: 'needs_attention',
    attention: { kind: 'yulius', problem: 'The agent asked for a secret, and a person has to answer its request.', action: 'Answer it on the Secrets page.', link },
  });

  await expect(bar(browser)).toHaveText(/Needs you[\s\S]*Problem[\s\S]*The agent asked for a secret[\s\S]*To do[\s\S]*Answer it on the Secrets page\./);
  const open = browser.locator('.attention-bar a:has-text("Open")');
  await expect(open).toBeVisible();
  expect(await open.getAttribute('href')).toBe(link);
  // A page of this Yulius opens in place.
  expect(await open.getAttribute('target')).toBeNull();
  await expect(screen.getByRole('button', 'Reply')).toBeVisible();
  expect(await axeViolations(browser)).toEqual([]);
});

test("a question in the terminal reads so, in Claude Code's words when it gave any", async ({ app, browser }) => {
  await app.open(mock('/s/studio/t/12', '&ambient=0'));
  await terminalShown(browser);
  const text = browser.locator('.attention-bar .attention-text');

  await touchTask(browser, 'sb_studio', 12, { activity: 'needs_attention', attention: { kind: 'terminal', problem: 'Claude needs your permission to use Bash', action: null, link: null } });
  await expect(text).toHaveText('The agent needs your attention. Claude needs your permission to use Bash. Its question is in the terminal below.');
  expect(await axeViolations(browser)).toEqual([]);

  // Words that end a sentence themselves get no second full stop.
  await touchTask(browser, 'sb_studio', 12, { attention: { kind: 'terminal', problem: 'Allow the plan?', action: null, link: null } });
  await expect(text).toHaveText('The agent needs your attention. Allow the plan? Its question is in the terminal below.');

  await touchTask(browser, 'sb_studio', 12, { attention: { kind: 'terminal', problem: null, action: null, link: null } });
  await expect(text).toHaveText('The agent needs your attention. Its question is in the terminal below.');
});

test('a flag with no reason claims no question', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/t/12', '&ambient=0'));
  await terminalShown(browser);
  await touchTask(browser, 'sb_studio', 12, { activity: 'needs_attention' });

  await expect(browser.locator('.attention-bar .attention-text')).toHaveText('The agent needs your attention. Look at its terminal below.');
  await expect(screen.getByRole('button', 'Reply')).toBeVisible();
});

test('Reply says that the terminal took the keyboard', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/t/12', '&ambient=0'));
  await terminalShown(browser);
  await touchTask(browser, 'sb_studio', 12, { activity: 'needs_attention', attention: GATE });
  await expect(bar(browser)).toHaveText(/Needs you/);
  // Away from the terminal first, so focusing it is the click's doing.
  await browser.evaluate(() => (document.activeElement as HTMLElement | null)?.blur());
  await expect(notice(browser)).toBeHidden();

  await screen.getByRole('button', 'Reply').click();
  await expect(notice(browser)).toHaveText('The terminal has the keyboard: answer there.');
  await expect.poll(() => focusedLabel(browser)).toBe('Terminal input');
  // A flag never opens the text field: a menu may be open in the terminal (SPEC-156 RPL-006).
  await expect(screen.getByRole('textbox', 'Reply to the agent')).toBeHidden();
  expect(await axeViolations(browser)).toEqual([]);
});

test('Reply on a question in the terminal says the same', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/t/12', '&ambient=0'));
  await terminalShown(browser);
  await touchTask(browser, 'sb_studio', 12, { activity: 'needs_attention', attention: { kind: 'terminal', problem: null, action: null, link: null } });
  await expect(bar(browser)).toHaveText(/question is in the terminal below/);
  await browser.evaluate(() => (document.activeElement as HTMLElement | null)?.blur());

  await screen.getByRole('button', 'Reply').click();
  await expect(notice(browser)).toHaveText('The terminal has the keyboard: answer there.');
  await expect.poll(() => focusedLabel(browser)).toBe('Terminal input');
});

test('Reply on a terminal that takes no input says why instead', async ({ app, browser, screen }) => {
  await app.open(mock('/s/sandbox-3/t/1', '&ambient=0'));
  await firstShown(screen.getByText('Read-only (GitHub: read)'));
  await terminalShown(browser);
  await touchTask(browser, 'sb_three', 1, { activity: 'needs_attention', attention: GATE });
  await expect(bar(browser)).toHaveText(/Needs you/);

  await screen.getByRole('button', 'Reply').click();
  await expect(notice(browser)).toHaveText('Read-only (GitHub: read)');
});

test("a watcher reads the reason, and Reply stays the owner's", async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/t/12', '&ambient=0'));
  await terminalShown(browser);
  await touchTask(browser, 'sb_studio', 12, { owner: { login: 'anna-q' } });
  await expect(browser.locator('.taskview .watch .ss-tui')).toBeVisible();

  await touchTask(browser, 'sb_studio', 12, { activity: 'needs_attention', attention: GATE });
  await expect(bar(browser)).toHaveText(/Needs you[\s\S]*Problem[\s\S]*stopped 3 times[\s\S]*To do[\s\S]*Tell it how to go on/);
  await expect(screen.getByRole('button', 'Reply')).toBeHidden();
  expect(await axeViolations(browser)).toEqual([]);
});

test("the task's row says the reason's to-do, not to approve in the terminal", async ({ app, browser }) => {
  await app.open(mock('/s/studio/t/12', '&ambient=0'));
  await terminalShown(browser);
  const row = browser.locator('.sidebar .task-link[data-route="/s/studio/t/12"] .sr-only:has-text("Tell it how to go on")');

  await touchTask(browser, 'sb_studio', 12, { activity: 'needs_attention', attention: { kind: 'terminal', problem: null, action: null, link: null } });
  await expect(browser.locator('.sidebar .task-link[data-route="/s/studio/t/12"] .sr-only:has-text("Approve or answer in the terminal")')).toHaveCount(1);
  await touchTask(browser, 'sb_studio', 12, { attention: GATE });
  await expect(row).toHaveCount(1);
  await expect(browser.locator('.sidebar .task-link[data-route="/s/studio/t/12"] .sr-only:has-text("Approve or answer in the terminal")')).toHaveCount(0);
});
