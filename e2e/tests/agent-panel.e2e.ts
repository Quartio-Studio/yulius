import { test } from '@e2e-dev/web';
import { expect } from 'e2e';
import { axeViolations, failOnPageErrors, mock } from './support.ts';

failOnPageErrors();

// The Agent (SPEC-035, formerly the Cockpit of SPEC-021): every user's own plain Claude Code session
// per sandbox, in the right tool panel beside the page, for quick work that should not become a task.

const agentTerminal = '.agent-term .xterm-rows';

test('the Agent opens on the right beside the page, says what it is for and keeps its session', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/t/12'));
  await expect(screen.getByRole('heading', { level: 1 })).toHaveText('#12 Checkout API');

  await screen.getByRole('button', 'Show Agent').click();
  const panel = screen.getByRole('region', 'Tool panel');
  await expect(panel).toBeVisible();
  // The task stays in the centre: the Agent is beside it, not instead of it.
  await expect(screen.getByRole('heading', { level: 1 })).toHaveText('#12 Checkout API');
  await expect(screen.getByRole('navigation', 'Tools').getByRole('button', 'Agent')).toHaveAttribute('aria-pressed', 'true');

  // The notice: quick work here, anything bigger (writing code) in a task.
  const note = screen.getByRole('note', 'About the Agent');
  await expect(note).toHaveText(/quick work.*automations.*quick question/i);
  await expect(note).toHaveText(/writing code.*task/i);

  const terminal = browser.locator(agentTerminal);
  await expect(terminal).toHaveText(/Welcome to Claude Code/);
  // The keyboard is in the session: a quick question gets an answer.
  await expect(browser.locator('.agent-term .xterm-helper-textarea')).toBeFocused();
  await browser.keyboard.type('why?');
  await browser.keyboard.press('Enter');
  await expect(terminal).toHaveText(/Good question/);
  expect(await axeViolations(browser)).toEqual([]);

  // Nothing calls it the Cockpit any more.
  expect(await browser.evaluate(() => document.body.innerText)).not.toMatch(/Cockpit/);

  // Hidden and shown again, and after another page, it is the same session.
  await screen.getByRole('button', 'Hide Agent').click();
  await expect(panel).toBeHidden();
  await screen.getByRole('navigation', 'Sandbox').getByRole('link', 'Tasks').click();
  await screen.getByRole('button', 'Show Agent').click();
  await expect(terminal).toHaveText(/Good question/);
});

test('the Agent notice opens a new task for bigger work', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/tasks'));
  await screen.getByRole('button', 'Show Agent').click();
  await screen.getByRole('note', 'About the Agent').getByRole('link', 'New task').click();
  await expect(browser).toHaveURL(/#\/s\/studio$/);
  await expect(screen.getByRole('heading', { level: 1 })).toHaveText('New task');
});

test('the Agent is not in the left navigation', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/t/12'));
  await expect(screen.getByRole('navigation', 'Sandbox').getByRole('link', 'Tasks')).toBeVisible();
  const items = await browser.evaluate(() => [...document.querySelectorAll('nav.nav a')].map((a) => a.textContent));
  expect(items.filter((text) => /Cockpit|Agent/.test(text ?? ''))).toEqual([]);
});

for (const path of ['agent', 'cockpit']) {
  test(`/s/<slug>/${path} opens the sandbox with the Agent`, async ({ app, browser, screen }) => {
    await app.open(mock(`/s/studio/${path}`));
    await expect(browser.locator(agentTerminal)).toHaveText(/Welcome to Claude Code/);
    await expect(browser).toHaveURL(/#\/s\/studio$/);
    await expect(screen.getByRole('button', 'Hide Agent')).toBeVisible();
  });
}

test('a stopped Agent starts again with its conversation', async ({ app, browser, screen }) => {
  // Wide enough that the panel's terminal does not wrap the welcome line.
  await browser.setViewport({ width: 1920, height: 1000 });
  await app.open(mock('/s/studio/agent'));
  const terminal = browser.locator(agentTerminal);
  await expect(terminal).toHaveText(/Welcome to Claude Code/);
  await browser.locator('.agent-term .xterm-helper-textarea').focus();
  await browser.keyboard.type('/exit');
  await browser.keyboard.press('Enter');
  await expect(screen.getByText('The Agent stopped. Its conversation is kept.')).toBeVisible();

  await screen.getByRole('button', 'Start again').click();
  await expect(terminal).toHaveText(/resumed your last conversation/);
});

test('an Agent whose process is gone is started again by itself', async ({ app, browser }) => {
  // The panel got an Agent that no longer runs (the machine rebooted): it does not stay stuck on it.
  await app.open(mock('/s/studio/agent', '&cockpit=gone'));
  await expect(browser.locator(agentTerminal)).toHaveText(/Welcome to Claude Code/);
});

test('an Agent that cannot start says why and offers to try again', async ({ app, screen }) => {
  await app.open(mock('/s/studio/agent', '&cockpit=fail'));
  await expect(screen.getByRole('alert')).toHaveText(/Couldn’t start the Agent: no subscription has capacity right now/);
  await expect(screen.getByRole('button', 'Try again')).toBeVisible();
});

test('read access explains why the Agent is not available', async ({ app, browser, screen }) => {
  await app.open(mock('/s/sandbox-3/agent'));
  await expect(screen.getByText('The Agent needs write access to this sandbox.')).toBeVisible();
  await expect(browser.locator(agentTerminal)).toBeHidden();
});

// Review round 1: New task from a task page hands the keyboard to the composer, not the Agent.
test('New task from the Agent beside a task focuses the composer', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/t/12'));
  await screen.getByRole('button', 'Show Agent').click();
  await expect(browser.locator(agentTerminal)).toHaveText(/Welcome to Claude Code/);
  await screen.getByRole('note', 'About the Agent').getByRole('link', 'New task').click();
  await expect(screen.getByRole('heading', { level: 1 })).toHaveText('New task');
  // Typed text must become a task, never a message to the Agent.
  await expect.poll(() => browser.evaluate(() => Boolean(document.activeElement?.closest('.agent-term')))).toBe(false);
  await expect(screen.getByRole('textbox', 'Task prompt')).toBeFocused();
});

test('on a phone, New task closes the Agent so the composer shows', async ({ app, browser, screen }) => {
  await browser.setViewport({ width: 390, height: 844 });
  await app.open(mock('/s/studio/agent'));
  await expect(browser.locator(agentTerminal)).toHaveText(/Welcome to Claude Code/);
  await screen.getByRole('note', 'About the Agent').getByRole('link', 'New task').click();
  await expect(screen.getByRole('region', 'Tool panel')).toBeHidden();
  await expect(screen.getByRole('heading', { level: 1 })).toHaveText('New task');
});

test('the Agent opens from the rail, keeps its session across tools and the top bar follows', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/tasks'));
  await screen.getByRole('button', /^Show tools/).click();
  const rail = screen.getByRole('navigation', 'Tools');
  await rail.getByRole('button', 'Agent').click();
  await expect(screen.getByRole('button', 'Hide Agent')).toHaveAttribute('aria-pressed', 'true');
  const terminal = browser.locator(agentTerminal);
  await expect(terminal).toHaveText(/Welcome to Claude Code/);
  await browser.keyboard.type('why?');
  await browser.keyboard.press('Enter');
  await expect(terminal).toHaveText(/Good question/);

  // Another tool in the panel: the button is released; back to the Agent, the same session.
  await rail.getByRole('button', 'Shell').click();
  await expect(screen.getByRole('button', 'Show Agent')).toHaveAttribute('aria-pressed', 'false');
  await rail.getByRole('button', 'Agent').click();
  await expect(terminal).toHaveText(/Good question/);
});

test('the top-bar Agent button does not leave the tools rail on for good', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/tasks'));
  await expect(screen.getByRole('navigation', 'Tools')).toBeHidden();
  await screen.getByRole('button', 'Show Agent').click();
  await expect(screen.getByRole('region', 'Tool panel')).toBeVisible();
  await screen.getByRole('button', 'Hide Agent').click();
  await expect(screen.getByRole('navigation', 'Tools')).toBeHidden();
  await browser.reload();
  await expect(screen.getByRole('button', 'Show Agent')).toBeVisible();
  await expect(screen.getByRole('navigation', 'Tools')).toBeHidden();
});

test('switching sandbox with the Agent open shows that sandbox’s Agent', async ({ app, screen }) => {
  await app.open(mock('/s/studio/agent'));
  await expect(screen.getByRole('button', 'Hide Agent')).toBeVisible();
  await screen.getByRole('navigation', 'Sandboxes').getByRole('link', /^sandbox-3\b/).click();
  await expect(screen.getByText('The Agent needs write access to this sandbox.')).toBeVisible();
  // Read users get no New task link they cannot use.
  await expect(screen.getByRole('note', 'About the Agent').getByRole('link', 'New task')).toBeHidden();
});

// Review round 2: the new sandbox's Agent starts beside the page without taking the keyboard.
test('switching to a sandbox whose Agent starts keeps the keyboard on the page', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/t/12'));
  await screen.getByRole('button', 'Show Agent').click();
  await expect(browser.locator(agentTerminal)).toHaveText(/Welcome to Claude Code/);
  await screen.getByRole('navigation', 'Sandboxes').getByRole('link', /^product\b/).click();
  await expect(screen.getByRole('heading', { level: 1 })).toHaveText('New task');
  await expect(browser.locator(agentTerminal)).toHaveText(/cwd: .*product/);
  await expect(screen.getByRole('textbox', 'Task prompt')).toBeFocused();
});
