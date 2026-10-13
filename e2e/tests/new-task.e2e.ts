import { test } from '@e2e-dev/web';
import { expect } from 'e2e';
import { axeViolations, failOnPageErrors, mock } from './support.ts';

failOnPageErrors();

type Holding = Window & { __yuliusMock: { holdCreate?: Promise<void> }; __release?: () => void };

// The new-task composer (SPEC-001 UI-021, TSK-015; F 2026-10-10): Enter adds a line and continues
// Markdown lists, ⌘/Ctrl+Enter or the ↑ button starts, and mode and workflow default to Auto.

test('the new-task shortcut opens the composer; Enter adds a line and ⌘/Ctrl+Enter starts', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/t/12'));
  await expect(screen.getByRole('heading', { level: 1 })).toHaveText('#12 Checkout API');

  await browser.keyboard.press('ControlOrMeta+Shift+O');

  await expect(browser).toHaveURL(/#\/s\/studio$/);
  const prompt = screen.getByRole('textbox', 'Task prompt');
  await expect(prompt).toBeFocused();

  await prompt.fill('Add a health check endpoint');
  await prompt.press('Enter');
  // Enter is a new line now, never a start.
  await expect(prompt).toHaveValue('Add a health check endpoint\n');
  await expect(browser).toHaveURL(/#\/s\/studio$/);
  // The hint shows the keyboard way to start, not Shift+Enter.
  await expect(browser.locator('.composer-hint')).toHaveText(/to start/);
  await expect(browser.locator('.composer-hint')).not.toHaveText(/Shift/);

  await prompt.press('ControlOrMeta+Enter');

  await expect(browser).toHaveURL(/#\/s\/studio\/t\/\d+$/);
  await expect(screen.getByRole('heading', { level: 1 })).toHaveText(/^#\d+ Add a health check endpoint$/);
  // Auto chose the mode and the workflow; the task view says so.
  await expect(browser.locator('.auto-chip')).toHaveText('Auto → Goal · Feature');
});

test('lists continue on Enter, end on an empty item, indent with Tab; ⌘/Ctrl+B and I wrap the selection', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio'));
  const prompt = screen.getByRole('textbox', 'Task prompt');
  await prompt.click();
  await browser.keyboard.type('Plan:');
  await browser.keyboard.press('Enter');
  await browser.keyboard.type('- one');
  await browser.keyboard.press('Enter');
  await browser.keyboard.type('two');
  await browser.keyboard.press('Enter');
  await browser.keyboard.press('Tab');
  await browser.keyboard.type('nested');
  await browser.keyboard.press('Enter');
  // An empty nested item moves out a level, an empty top-level item ends the list.
  await browser.keyboard.press('Enter');
  await browser.keyboard.press('Enter');
  await browser.keyboard.type('1. first');
  await browser.keyboard.press('Enter');
  await browser.keyboard.type('second');
  await browser.keyboard.press('Enter');
  await browser.keyboard.press('Enter');
  await browser.keyboard.type('- [ ] a task');
  await browser.keyboard.press('Enter');
  await browser.keyboard.type('next');
  await expect(prompt).toHaveValue('Plan:\n- one\n- two\n  - nested\n1. first\n2. second\n- [ ] a task\n- [ ] next');
  // Shift+Tab outdents; on a top-level item it leaves the text box as usual.
  await browser.keyboard.press('Tab');
  await expect(prompt).toHaveValue('Plan:\n- one\n- two\n  - nested\n1. first\n2. second\n- [ ] a task\n  - [ ] next');
  await browser.keyboard.press('Shift+Tab');
  await expect(prompt).toHaveValue('Plan:\n- one\n- two\n  - nested\n1. first\n2. second\n- [ ] a task\n- [ ] next');
  await expect(browser).toHaveURL(/#\/s\/studio$/);

  await prompt.fill('make it bold and calm');
  await browser.evaluate(() => {
    const box = document.querySelector('textarea.composer-prompt') as HTMLTextAreaElement;
    box.setSelectionRange(8, 12);
  });
  await browser.keyboard.press('ControlOrMeta+b');
  await expect(prompt).toHaveValue('make it **bold** and calm');
  await browser.keyboard.press('ControlOrMeta+b');
  await expect(prompt).toHaveValue('make it bold and calm');
  await browser.keyboard.press('ControlOrMeta+i');
  await expect(prompt).toHaveValue('make it *bold* and calm');

  // A code fence closes below the caret.
  await prompt.fill('');
  await browser.keyboard.type('```sh');
  await browser.keyboard.press('Enter');
  await browser.keyboard.type('cargo test');
  await expect(prompt).toHaveValue('```sh\ncargo test\n```');

  // Preview shows the prompt formatted, read-only; Edit goes back.
  await prompt.fill('- **one**\n- two');
  await screen.getByRole('button', 'Preview').click();
  await expect(browser.locator('.composer-preview strong')).toHaveText('one');
  await expect(prompt).toBeHidden();
  await screen.getByRole('button', 'Preview').click();
  await expect(prompt).toBeFocused();
  expect(await axeViolations(browser)).toEqual([]);
});

test('Auto is the default; a question runs as a prompt and picked values win', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio'));
  await expect(screen.getByRole('button', 'Mode: Auto mode')).toBeVisible();
  await expect(screen.getByRole('button', 'Workflow: Auto workflow')).toBeVisible();
  const prompt = screen.getByRole('textbox', 'Task prompt');
  await prompt.fill('How does the checkout compute the cart total?');
  // The ↑ button starts too; while Jev decides, a calm status says so. The creation is held until
  // the status was seen: it goes when the answer comes, which a busy runner can reach first (#320).
  await browser.evaluate(() => {
    const w = window as unknown as Holding;
    w.__yuliusMock.holdCreate = new Promise<void>((resolve) => { w.__release = resolve; });
  });
  await screen.getByRole('button', 'Start task').click();
  await expect(browser.locator('.intake-status')).toHaveText('Choosing mode and workflow…');
  await browser.evaluate(() => (window as unknown as Holding).__release!());
  await expect(browser).toHaveURL(/#\/s\/studio\/t\/\d+$/);
  // A question is a prompt with the Research workflow: no spec first.
  await expect(browser.locator('.auto-chip')).toHaveText('Auto → Prompt · Research');

  // Picked mode and workflow are sent as they are: no Auto label.
  await browser.keyboard.press('ControlOrMeta+Shift+O');
  await screen.getByRole('button', 'Mode: Auto mode').click();
  await screen.getByRole('menuitemradio', /^Goal/).click();
  await screen.getByRole('button', 'Workflow: Auto workflow').click();
  await screen.getByRole('menuitemradio', /^Fix \(default\)/).click();
  await expect(screen.getByRole('button', 'Workflow: Fix')).toBeVisible();
  await screen.getByRole('textbox', 'Task prompt').fill('Bump the lockfile');
  await screen.getByRole('textbox', 'Task prompt').press('ControlOrMeta+Enter');
  await expect(screen.getByRole('heading', { level: 1 })).toHaveText(/^#\d+ Bump the lockfile$/);
  await expect(browser.locator('.auto-chip')).toBeHidden();
  await expect(browser.locator('.strip-extras')).toHaveText(/Goal/);
});
