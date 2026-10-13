import { test } from '@e2e-dev/web';
import { expect } from 'e2e';
import { axeViolations, failOnPageErrors, mock } from './support.ts';

failOnPageErrors();

// F 2026-10-10: "Those Claude sessions should go on server as well. I want to be able to see them
// in read-only mode as any other session. It should be accessible from the panel." Sessions lists
// task sessions and imported ones together, with filters and search; one opens read-only.
test('Sessions lists task and imported sessions, filters and searches them', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/t/12'));
  // Sessions is under More in the sandbox navigation (SPEC-029).
  await screen.getByRole('navigation', 'Sandbox').getByRole('button', 'More').click();
  await screen.getByRole('navigation', 'Sandbox').getByRole('link', 'Sessions').click();
  await expect(browser).toHaveURL(/#\/s\/studio\/sessions$/);
  await expect(screen.getByRole('heading', { level: 1 })).toHaveText('Sessions');

  const list = screen.getByRole('list', 'Sessions');
  await expect(list.getByRole('link', /Checkout API/)).toBeVisible();
  await expect(list.getByRole('link', /Add Stripe checkout to the cart page/)).toContainText('Imported by @mkarp');
  await expect(list.getByRole('link', /Add Stripe checkout/)).toContainText('3 secrets redacted');
  expect(await axeViolations(browser)).toEqual([]);

  // Only the uploaded ones.
  await screen.getByRole('radiogroup', 'Kind of session').getByRole('radio', /^Imported/).click();
  await expect(list.getByRole('link')).toHaveCount(2);
  await expect(list.getByRole('link', /Checkout API/)).toHaveCount(0);
  await expect(browser).toHaveURL(/kind=imported/);

  // Full-text search marks what matched.
  await screen.getByRole('radiogroup', 'Kind of session').getByRole('radio', /^All/).click();
  await screen.getByRole('searchbox', 'Search sessions').fill('stripe webhook');
  await expect(list.getByRole('link')).toHaveCount(2);
  await expect(browser.locator('.ss-rows mark').first()).toBeVisible();
  await expect(screen.getByText('2 sessions match')).toBeVisible();
  await screen.getByRole('searchbox', 'Search sessions').fill('no such words anywhere');
  await expect(screen.getByText('Nothing found')).toBeVisible();
});

test('a session opens read-only with messages, tool calls and diffs, and continues in a task', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/sessions'));
  await screen.getByRole('list', 'Sessions').getByRole('link', /Add Stripe checkout/).click();
  await expect(browser).toHaveURL(/#\/s\/studio\/sessions\/ses_imp_checkout/);
  await expect(screen.getByRole('heading', { level: 1 })).toHaveText('Add Stripe checkout to the cart page');
  await expect(screen.getByText('Read-only. Secrets are redacted.')).toBeVisible();

  const transcript = screen.getByRole('list', 'Transcript');
  await expect(transcript.getByText('Add Stripe checkout to the cart page. Use the test keys from .env.')).toBeVisible();
  // An edit shows as a diff, with its result inside the call.
  const diff = transcript.getByRole('group', 'Changes to src/api/checkout.ts');
  await expect(diff).toContainText('+ export async function checkout(cart, key) {');
  await expect(diff).toContainText('- export async function checkout(cart) {');
  await expect(transcript.getByText('The file src/api/checkout.ts has been updated.')).toBeVisible();
  // A redacted dotenv read stays redacted; folded calls open on demand.
  await transcript.getByText('/repo/.env').click();
  await expect(transcript.getByText(/STRIPE_SECRET_KEY=\[REDACTED:env-value\]/)).toBeVisible();
  await expect(screen.getByText('End of the session.')).toBeVisible();
  // No text box: nothing can be typed into a read-only session.
  expect(await browser.evaluate(() => document.querySelectorAll('#main textarea, #main input[type="text"]').length)).toBe(0);
  expect(await axeViolations(browser)).toEqual([]);

  // Continuing creates a task from the session and opens it.
  await screen.getByRole('button', 'Continue in a new task').click();
  await expect(browser).toHaveURL(/#\/s\/studio\/t\/\d+$/);
});

// Task #71 (SPEC-075): "Sessions should be printed as their default agents sessions (the default
// TUI)". The transcript reads as Claude Code printed it: `>` prompts, `⏺` messages and calls,
// `⎿` results folded to their first lines, edits unfolded with their changed lines.
test('a session reads as Claude Code’s TUI: prompts, messages, calls with results and diffs', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/sessions/ses_imp_checkout'));
  const transcript = screen.getByRole('list', 'Transcript');
  await expect(transcript.getByText('Add Stripe checkout to the cart page. Use the test keys from .env.')).toBeVisible();
  expect(await browser.evaluate(() => getComputedStyle(document.querySelector('.ss-tui')!).fontFamily)).toMatch(/IBM Plex Mono/);
  const line = (n: number) => browser.locator('.ss-tui .ss-entries > li').nth(n);
  await expect(line(0)).toContainText(/^\s*Prompt\s*>\s*Add Stripe checkout/);
  await expect(line(1)).toContainText(/⏺\s*I’ll look at the cart and the environment first\./);
  // Thinking is folded as the TUI's "✻ Thinking…".
  await expect(browser.locator('.ss-tui details.ss-thinking > summary').first()).toHaveText(/✻\s*Thinking…/);

  // A read is folded to "Read 2 lines" and unfolds to what it read.
  const summary = (text: string) => browser.locator('.ss-tui details.ss-tool > summary').filter({ hasText: text }).first();
  await expect(summary('/repo/.env')).toContainText(/⏺\s*Read\(\/repo\/\.env\)/);
  await expect(summary('/repo/.env')).toContainText(/⎿\s*Read 2 lines/);
  await expect(transcript.getByText(/STRIPE_SECRET_KEY=/)).toBeHidden();
  await expect(transcript.getByText('Read 2 lines')).toBeVisible();
  await summary('/repo/.env').click();
  await expect(transcript.getByText(/STRIPE_SECRET_KEY=\[REDACTED:env-value\]/)).toBeVisible();
  // Unfolded, the call shows all of it instead of the folded line.
  await expect(transcript.getByText('Read 2 lines')).toBeHidden();

  // A command shows its first output lines under ⎿ while folded.
  await expect(summary('npm test -- checkout')).toContainText(/⏺\s*Bash\(npm test -- checkout\)/);
  await expect(summary('npm test -- checkout')).toContainText(/⎿\s*PASS src\/api\/checkout\.test\.ts/);

  // An edit is "Update(path)", unfolded with its changed lines.
  await expect(summary('Update(')).toContainText(/⏺\s*Update\(src\/api\/checkout\.ts\)/);
  await expect(summary('Update(')).toContainText('Updated src/api/checkout.ts with 2 additions and 2 removals');
  await expect(browser.locator('.ss-tui details.ss-tool[open] .ss-diff')).toHaveCount(1);
  await expect(transcript.getByRole('group', 'Changes to src/api/checkout.ts')).toContainText('+ export async function checkout(cart, key) {');
  // Who said what is still named for assistive technology.
  await expect(browser.locator('.ss-tui .ss-entries > li .sr-only').first()).toHaveText('Prompt');
  expect(await axeViolations(browser)).toEqual([]);
});

test('a failed call is marked in the TUI’s error colour and says error', async ({ app, browser }) => {
  await app.open(mock('/s/studio/sessions/ses_imp_flaky'));
  const failed = browser.locator('.ss-tui details.ss-tool.ss-tool-error > summary');
  await expect(failed).toContainText(/⏺\s*Bash\(npx playwright test login\).*error/s);
  await expect(failed).toContainText(/⎿\s*Error: Timed out 5000ms/);
  // The marker and the output are in the error colour.
  const [mark, output, error] = await browser.evaluate(() => {
    const summary = document.querySelector('.ss-tui details.ss-tool.ss-tool-error > summary')!;
    const probe = summary.appendChild(document.createElement('span'));
    probe.style.color = 'var(--session-err)';
    const colors = [summary.querySelector('.ss-mark')!, summary.querySelector('.ss-out-text')!, probe].map((el) => getComputedStyle(el).color);
    probe.remove();
    return colors;
  });
  expect(mark).toBe(error);
  expect(output).toBe(error);
  // Many output lines fold to the first three and a count.
  await expect(browser.locator('.ss-tui details.ss-tool > summary').filter({ hasText: 'waitForTimeout in e2e' })).toContainText('… +2 lines');
});

// The attached screenshot printed `<command-name>/goal</command-name>…` raw. The TUI prints a
// slash command as typed, its output under ⎿, and hides reminders and the caveat line.
test('recorded wrappers parse as the TUI prints them, also when cut short (unit, in the page)', async ({ app, browser }) => {
  await app.open(mock('/s/studio/sessions'));
  const parsed = await browser.evaluate(async () => {
    const url = '/assets/pages/sessions.js';
    const { metaLines } = await import(url);
    return {
      goal: metaLines('<command-name>/goal</command-name>\n  <command-message>goal</command-message>\n  <command-args>Ship it</command-args>'),
      bare: metaLines('<command-name>/clear</command-name><command-message>clear</command-message><command-args></command-args>'),
      stdout: metaLines('<local-command-stdout>Goal set: Ship it</local-command-stdout>'),
      stderr: metaLines('<local-command-stderr>Error: compaction</local-command-stderr>'),
      shell: metaLines('<bash-input>ls</bash-input>'),
      shellOut: metaLines('<bash-stdout>a\nb</bash-stdout><bash-stderr></bash-stderr>'),
      reminder: metaLines('<system-reminder>\nsecret plans\n</system-reminder>'),
      hook: metaLines('<user-prompt-submit-hook>noise</user-prompt-submit-hook>'),
      caveat: metaLines('Caveat: The messages below were generated by the user while running local commands.'),
      interrupted: metaLines('[Request interrupted by user for tool use]'),
      cut: metaLines('<local-command-stdout>a very long output that the server cut'),
      note: metaLines('A session-scoped Stop hook is now active.'),
    };
  });
  expect(parsed.goal).toEqual([{ kind: 'command', text: '/goal Ship it' }]);
  expect(parsed.bare).toEqual([{ kind: 'command', text: '/clear' }]);
  expect(parsed.stdout).toEqual([{ kind: 'out', text: 'Goal set: Ship it' }]);
  expect(parsed.stderr).toEqual([{ kind: 'err', text: 'Error: compaction' }]);
  expect(parsed.shell).toEqual([{ kind: 'shell', text: 'ls' }]);
  expect(parsed.shellOut).toEqual([{ kind: 'out', text: 'a\nb' }]);
  expect(parsed.reminder).toEqual([]);
  expect(parsed.hook).toEqual([]);
  expect(parsed.caveat).toEqual([]);
  expect(parsed.interrupted).toEqual([{ kind: 'err', text: 'Interrupted by user' }]);
  expect(parsed.cut).toEqual([{ kind: 'out', text: 'a very long output that the server cut' }]);
  expect(parsed.note).toEqual([{ kind: 'note', text: 'A session-scoped Stop hook is now active.' }]);
});

test('a slash command, its output, a ! command and an interruption read as the TUI prints them', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/sessions/ses_task_4'));
  const transcript = screen.getByRole('list', 'Transcript');
  await expect(transcript).toContainText(/>\s*\/goal Payment webhook ships on green CI/);
  await expect(transcript).toContainText(/⎿\s*Goal set: Payment webhook ships on green CI/);
  await expect(transcript).toContainText(/!\s*ls src\/webhooks/);
  await expect(transcript).toContainText(/⎿\s*stripe\.ts\s*retry\.ts/);
  await expect(transcript).toContainText(/⎿\s*Interrupted by user/);
  await expect(browser.locator('.ss-tui .ss-out.ss-out-err').filter({ hasText: 'Interrupted by user' })).toHaveCount(1);
  await expect(transcript).toContainText(/⎿\s*Error: Error during compaction/);
  await expect(transcript).toContainText('A session-scoped Stop hook is now active');
  await expect(transcript).not.toContainText('<command-');
  await expect(transcript).not.toContainText('<local-command-');
  await expect(transcript).not.toContainText('<bash-');
  await expect(transcript).not.toContainText('The date is 2026-10-13');
  await expect(transcript).not.toContainText('Caveat:');
  expect(await axeViolations(browser)).toEqual([]);
});

test('a search match opens the transcript at its line', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/sessions?q=fulfilling'));
  await screen.getByRole('list', 'Sessions').getByRole('link', /Add Stripe checkout/).click();
  await expect(browser).toHaveURL(/sessions\/ses_imp_checkout\?line=7/);
  await expect(browser.locator('.ss-target')).toContainText('verifies signatures before fulfilling orders');
  await expect(screen.getByRole('link', 'Show from the start')).toBeVisible();
});
