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
  const failed = browser.locator('.ss-tui details.ss-tool.ss-tool-error > summary').filter({ hasText: 'npx playwright test login' });
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
  // A subagent's entries are indented and named for assistive technology.
  await expect(browser.locator('.ss-tui li.ss-sidechain')).toHaveCount(1);
  await expect(browser.locator('.ss-tui li.ss-sidechain .sr-only').first()).toHaveText('Claude (subagent)');

  // A failed edit says why, in the error colour, folded: nothing was updated (review round 1).
  const failedEdit = browser.locator('.ss-tui details.ss-tool.ss-tool-error').filter({ hasText: 'Update(e2e/login.spec.ts)' });
  await expect(failedEdit).toHaveCount(1);
  await expect(failedEdit).not.toHaveAttribute('open', '');
  await expect(failedEdit).toContainText(/⏺\s*Update\(e2e\/login\.spec\.ts\)\s*error/);
  await expect(failedEdit).toContainText(/⎿\s*<tool_use_error>String to replace not found in file\./);
  await expect(failedEdit).not.toContainText('Updated e2e/login.spec.ts');
  // A deletion counts no added line.
  await expect(browser.locator('.ss-tui details.ss-tool > summary').filter({ hasText: 'Update(e2e/cart.spec.ts)' })).toContainText('Updated e2e/cart.spec.ts with 0 additions and 1 removal');
  await expect(browser.locator('.ss-tui .ss-diff[aria-label="Changes to e2e/cart.spec.ts"] .ss-add')).toHaveCount(0);
  // A new file reads as the TUI's "Wrote N lines to path" (review round 2).
  await expect(browser.locator('.ss-tui details.ss-tool > summary').filter({ hasText: 'Write(e2e/wait.ts)' })).toContainText(/⎿\s*Wrote 3 lines to e2e\/wait\.ts/);
});

// Review round 2: an edit whose result is not in the transcript (the session ended, or the result
// is on the next page) claims nothing: it stays folded with no "Updated" line.
test('an edit without its result yet claims no update', async ({ app, browser }) => {
  await app.open(mock('/s/studio/sessions/ses_task_12'));
  const edit = browser.locator('.ss-tui details.ss-tool').filter({ hasText: 'Update(src/api/checkout.ts)' });
  await expect(edit).toHaveCount(1);
  await expect(edit).not.toHaveAttribute('open', '');
  await expect(edit).not.toContainText('Updated src/api/checkout.ts');
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
      // Review round 1: a tag mentioned in prose is prose; text around wrappers is kept as written.
      mention: metaLines('Use <bash-input> to run things, or <system-reminder> for notes.'),
      after: metaLines('<local-command-stdout>done</local-command-stdout>\nthen this'),
      two: metaLines('<command-name>/a</command-name><command-args>x</command-args><command-name>/b</command-name><command-args>y</command-args>'),
      cutClose: metaLines('<local-command-stdout>output</local-command-std'),
      ansi: metaLines('<local-command-stdout>\u001b[1mTotal cost:\u001b[22m $0.42</local-command-stdout>'),
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
  expect(parsed.mention).toEqual([{ kind: 'note', text: 'Use <bash-input> to run things, or <system-reminder> for notes.' }]);
  expect(parsed.after).toEqual([{ kind: 'out', text: 'done' }, { kind: 'note', text: 'then this' }]);
  expect(parsed.two).toEqual([{ kind: 'command', text: '/a x' }, { kind: 'command', text: '/b y' }]);
  expect(parsed.cutClose).toEqual([{ kind: 'out', text: 'output' }]);
  expect(parsed.ansi).toEqual([{ kind: 'out', text: 'Total cost: $0.42' }]);
});

test('a slash command, its output, a ! command and an interruption read as the TUI prints them', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/sessions/ses_task_4'));
  const transcript = screen.getByRole('list', 'Transcript');
  await expect(transcript).toContainText(/>\s*\/goal Payment webhook ships on green CI/);
  // A typed command is the person's prompt, with its time.
  const goal = browser.locator('.ss-tui .ss-entries > li').filter({ hasText: '/goal Payment webhook' });
  await expect(goal.getByText('Prompt')).toHaveCount(1);
  await expect(goal).toContainText(/ago/);
  await expect(browser.locator('.ss-tui li.ss-system .sr-only')).toHaveText('System');
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

// #347 (SPEC-090): the TUI does not print `isMeta` context. A long note (a skill's body) folds to one
// faint line that opens on demand; short notes people act on (the Stop hook's goal) stay as written.
test('a long injected note folds to one line, a short one stays readable (unit, in the page)', async ({ app, browser }) => {
  await app.open(mock('/s/studio/sessions'));
  const folds = await browser.evaluate(async () => {
    const url = '/assets/pages/sessions.js';
    const { foldsNote } = await import(url);
    const lines = (n: number) => Array.from({ length: n }, (_, i) => `line ${i}`).join('\n');
    return {
      goal: foldsNote('A session-scoped Stop hook is now active with condition: "Ship it". Briefly acknowledge the goal, then immediately start working toward it.'),
      feedback: foldsNote('Stop hook feedback:\nYou have 1 new Yulius notice.\nRead it.\nThen go on.'),
      eight: foldsNote(lines(8)),
      nine: foldsNote(lines(9)),
      atBound: foldsNote('x'.repeat(2000)),
      overBound: foldsNote('x'.repeat(2001)),
      trailing: foldsNote(`${lines(8)}\n\n\n`),
    };
  });
  expect(folds).toEqual({ goal: false, feedback: false, eight: false, nine: true, atBound: false, overBound: true, trailing: false });
});

test('a skill body shows as one folded line that opens on demand; the Stop hook goal stays shown', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/sessions/ses_task_4'));
  const transcript = screen.getByRole('list', 'Transcript');
  await expect(transcript).toContainText('A session-scoped Stop hook is now active with condition: "Payment webhook ships on green CI".');
  const note = browser.locator('.ss-tui details.ss-note');
  const line = browser.locator('.ss-tui details.ss-note > summary');
  const body = browser.locator('.ss-tui details.ss-note > .ss-note-text');
  await expect(note).toHaveCount(1);
  // Its size in UTF-8 bytes (1,621; the text is 1,539 UTF-16 units, which would read 1.5 KiB).
  await expect(line).toHaveText(/⎿\s*Context added \(1\.6 KiB\)$/);
  await expect(note).not.toHaveAttribute('open', '');
  await expect(body).toBeHidden();
  await expect(transcript).not.toContainText('Base directory for this skill');
  expect(await axeViolations(browser)).toEqual([]);

  await line.focus();
  await browser.keyboard.press('Enter');
  await expect(note).toHaveAttribute('open', '');
  await expect(body).toBeVisible();
  await expect(body).toContainText('Base directory for this skill: /home/dev/.claude/skills/retry-policy');
  await expect(body).toContainText('- Rule 36: back off before attempt 37.');
  await expect(body).toHaveAttribute('role', 'region');
  await expect(body).toHaveAttribute('aria-label', 'Added context');
  // Opened, it scrolls within a bounded height and the keyboard reaches it with a visible ring.
  await browser.keyboard.press('Tab');
  const ring = await browser.evaluate(() => {
    const text = document.activeElement as HTMLElement;
    return { reached: text.matches('.ss-note-text:focus-visible'), scrolls: text.scrollHeight > text.clientHeight, bounded: text.clientHeight <= 480, inset: getComputedStyle(text).boxShadow.includes('inset') };
  });
  expect(ring).toEqual({ reached: true, scrolls: true, bounded: true, inset: true });
  expect(await axeViolations(browser)).toEqual([]);
});

// "the contrast and the saturation should be lower to communicate that this session is old and this
// is a session not a task" — still at least WCAG AA for text, in both themes.
test('a session is faded in both themes: less contrast and saturation than a live terminal, still AA', async ({ app, browser }) => {
  await app.open(mock('/s/studio/sessions/ses_imp_flaky'));
  await expect(browser.locator('.ss-tui')).toBeVisible();
  for (const theme of ['dark', 'light']) {
    const m = await browser.evaluate((name) => {
      document.documentElement.setAttribute('data-theme', name);
      const well = document.querySelector('.ss-tui')!;
      const rgb = (token: string) => {
        const probe = well.appendChild(document.createElement('span'));
        probe.style.color = `var(${token})`;
        const [r, g, b] = getComputedStyle(probe).color.match(/\d+(\.\d+)?/g)!.map(Number);
        probe.remove();
        return [r, g, b];
      };
      const lum = (c: number[]) => {
        const [r, g, b] = c.map((v) => { const s = v / 255; return s <= 0.03928 ? s / 12.92 : ((s + 0.055) / 1.055) ** 2.4; });
        return 0.2126 * r + 0.7152 * g + 0.0722 * b;
      };
      const contrast = (a: string, b: string) => { const [x, y] = [lum(rgb(a)), lum(rgb(b))].sort((p, q) => q - p); return (x + 0.05) / (y + 0.05); };
      const saturation = (token: string) => { const c = rgb(token).map((v) => v / 255); const max = Math.max(...c); const min = Math.min(...c); const l = (max + min) / 2; return max === min ? 0 : (max - min) / (1 - Math.abs(2 * l - 1)); };
      return {
        live: contrast('--term-fg', '--term-bg'),
        fg: contrast('--session-fg', '--session-bg'),
        dim: contrast('--session-dim', '--session-bg'),
        dimOnPrompt: contrast('--session-dim', '--session-prompt-bg'),
        fgOnPrompt: contrast('--session-fg', '--session-prompt-bg'),
        ok: contrast('--session-ok', '--session-bg'),
        err: contrast('--session-err', '--session-bg'),
        added: contrast('--session-fg', '--session-add-bg'),
        removed: contrast('--session-fg', '--session-del-bg'),
        sat: { ok: saturation('--session-ok'), green: saturation('--ansi-green'), err: saturation('--session-err'), red: saturation('--ansi-red') },
      };
    }, theme);
    expect(m.fg, theme).toBeLessThan(m.live * 0.75);
    for (const value of [m.fg, m.dim, m.dimOnPrompt, m.fgOnPrompt, m.ok, m.err, m.added, m.removed]) expect(value, theme).toBeGreaterThanOrEqual(4.5);
    expect(m.sat.ok, theme).toBeLessThan(m.sat.green * 0.7);
    expect(m.sat.err, theme).toBeLessThan(m.sat.red * 0.7);
  }
  expect(await axeViolations(browser)).toEqual([]);
});

// "it should be read-only … No way to write to them": the TUI's prompt box is locked, and a spider
// web in the corner says nobody has typed here for a while.
test('a session ends with a locked prompt box and has a spider web in its corner', async ({ app, browser }) => {
  await app.open(mock('/s/studio/sessions/ses_imp_checkout'));
  const locked = browser.locator('.ss-tui .ss-locked');
  await expect(locked).toHaveText(/>\s*Read-only session · nothing can be sent/);
  expect(await browser.evaluate(() => {
    const box = document.querySelector('.ss-tui .ss-locked')!;
    return [box.matches(':last-child'), box.closest('button, a, [tabindex], [contenteditable]') === null];
  })).toEqual([true, true]);
  expect(await browser.evaluate(() => document.querySelectorAll('#main textarea, #main input:not([type=search]), #main [contenteditable], #main .xterm').length)).toBe(0);

  const web = await browser.evaluate(() => {
    const well = document.querySelector('.ss-tui')!.getBoundingClientRect();
    const svg = document.querySelector('.ss-tui > svg.ss-web')!;
    const box = svg.getBoundingClientRect();
    return {
      hidden: svg.getAttribute('aria-hidden'),
      pointer: getComputedStyle(svg).pointerEvents,
      crisp: svg.getAttribute('shape-rendering'),
      pixels: svg.querySelectorAll('rect').length,
      top: Math.round(box.top - well.top),
      right: Math.round(well.right - box.right),
      // The first lines make room for it instead of running under it.
      clear: [...document.querySelectorAll('.ss-tui .ss-entries > li')].filter((li) => li.getBoundingClientRect().top < box.bottom).every((li) => li.getBoundingClientRect().right <= box.left + 1),
    };
  });
  expect(web.hidden).toBe('true');
  expect(web.pointer).toBe('none');
  expect(web.crisp).toBe('crispEdges');
  expect(web.pixels).toBeGreaterThan(20);
  expect(web.top).toBeLessThanOrEqual(2);
  expect(web.right).toBeLessThanOrEqual(2);
  expect(web.clear).toBe(true);
  expect(await axeViolations(browser)).toEqual([]);
});

// Task #86 (SPEC-101): "the web should be sticky (should stick to the top while scrolling)". Scrolled
// down, the web stays in the top-right corner of what is visible of the well.
test('the spider web sticks to the top of the view while the session scrolls', async ({ app, browser }) => {
  await browser.setViewport({ width: 1280, height: 420 });
  await app.open(mock('/s/studio/sessions/ses_imp_checkout'));
  await expect(browser.locator('.ss-tui .ss-locked')).toBeVisible();
  const place = () => browser.evaluate(() => {
    const page = document.querySelector<HTMLElement>('.ss-page')!;
    const well = document.querySelector('.ss-tui')!.getBoundingClientRect();
    const box = document.querySelector('.ss-tui > svg.ss-web')!.getBoundingClientRect();
    const view = page.getBoundingClientRect().top + page.clientTop;
    return { scrollable: page.scrollHeight - page.clientHeight, top: Math.round(box.top - Math.max(view, well.top)), right: Math.round(well.right - box.right), inWell: box.bottom <= well.bottom + 1 };
  });
  const before = await place();
  expect(before.scrollable).toBeGreaterThan(200);
  expect(before.top).toBeLessThanOrEqual(2);
  await browser.evaluate(() => { document.querySelector<HTMLElement>('.ss-page')!.scrollTop = 200; });
  await expect.poll(async () => (await place()).top).toBeLessThanOrEqual(2);
  const after = await place();
  expect(after.top).toBeGreaterThanOrEqual(-2);
  expect(after.right).toBeLessThanOrEqual(2);
  expect(after.inWell).toBe(true);

  // Stuck, the web lies under the text, so it fades to a colour every text of the well reads on at
  // AA, spider included; at the top, beside the first lines, it keeps its own colours. Everything
  // after it in the well paints over it.
  const colours = (theme: string) => browser.evaluate((name) => {
    document.documentElement.setAttribute('data-theme', name);
    const well = document.querySelector('.ss-tui')!;
    const parse = (c: string) => c.match(/\d+(\.\d+)?/g)!.slice(0, 3).map(Number);
    const token = (t: string) => {
      const probe = well.appendChild(document.createElement('span'));
      probe.style.color = `var(${t})`;
      const c = getComputedStyle(probe).color;
      probe.remove();
      return parse(c);
    };
    const lum = (c: number[]) => {
      const [r, g, b] = c.map((v) => { const x = v / 255; return x <= 0.03928 ? x / 12.92 : ((x + 0.055) / 1.055) ** 2.4; });
      return 0.2126 * r + 0.7152 * g + 0.0722 * b;
    };
    const contrast = (a: number[], b: number[]) => { const [x, y] = [lum(a), lum(b)].sort((p, q) => q - p); return (x + 0.05) / (y + 0.05); };
    const fills = ['.ss-web-thread', '.ss-web-silk', '.ss-web-body'].map((sel) => parse(getComputedStyle(well.querySelector(sel)!).fill));
    const texts = ['--session-fg', '--session-dim', '--session-ok', '--session-err'].map(token);
    return {
      web: String(fills[0]) === String(token('--session-web')),
      lowest: Math.min(...fills.flatMap((f) => texts.map((t) => contrast(t, f)))),
      over: ['.ss-entries > li', '.ss-more', '.ss-locked'].every((sel) => [...well.querySelectorAll(sel)].every((el) => getComputedStyle(el).position !== 'static')),
    };
  }, theme);
  // It fades as soon as it sticks: the well's top just past the view's.
  await browser.evaluate(() => {
    const page = document.querySelector<HTMLElement>('.ss-page')!;
    page.scrollTop = 0;
    page.scrollTop = document.querySelector('.ss-tui')!.getBoundingClientRect().top - page.getBoundingClientRect().top - page.clientTop + 3;
  });
  await expect.poll(async () => (await colours('dark')).web).toBe(false);
  await browser.evaluate(() => { document.querySelector<HTMLElement>('.ss-page')!.scrollTop = 200; });
  for (const theme of ['dark', 'light']) {
    await expect.poll(async () => (await colours(theme)).web, theme).toBe(false);
    const stuck = await colours(theme);
    expect(stuck.lowest, theme).toBeGreaterThanOrEqual(4.5);
    expect(stuck.over, theme).toBe(true);
  }
  await browser.evaluate(() => { document.querySelector<HTMLElement>('.ss-page')!.scrollTop = 0; });
  await expect.poll(async () => (await colours('dark')).web).toBe(true);
  expect(await axeViolations(browser)).toEqual([]);
});

// Task #86 (SPEC-101): "spider should be animated". On its loop the spider climbs its thread in
// pixel steps, the thread shortening with it, its legs switching frames, and drops back; it rests
// where SPEC-075 drew it, and keeps still for people who asked for reduced motion.
test('the spider climbs its thread and drops back, and keeps still under reduced motion', async ({ app, browser }) => {
  await app.open(mock('/s/studio/sessions/ses_imp_checkout'));
  await expect(browser.locator('.ss-tui .ss-locked')).toBeVisible();
  // Seek every animation of the spider to a fraction of its loop and measure it in pixels.
  const at = (fraction: number) => browser.evaluate((f) => {
    const svg = document.querySelector('.ss-tui > svg.ss-web')!;
    const spider = svg.querySelector('.ss-web-spider')!;
    const silk = svg.querySelector('.ss-web-silk')!;
    const animations = [spider, silk, ...svg.querySelectorAll('.ss-web-legs')].flatMap((el) => el.getAnimations());
    for (const a of animations) {
      a.pause();
      a.currentTime = f * Number(a.effect!.getComputedTiming().duration);
    }
    const frame = svg.getBoundingClientRect();
    const pixel = frame.height / 34;
    const s = spider.getBoundingClientRect();
    const t = silk.getBoundingClientRect();
    const shown = (selector: string) => getComputedStyle(svg.querySelector(selector)!).visibility === 'visible';
    return {
      animations: animations.length,
      spiderTop: Math.round((s.top - frame.top) / pixel),
      silk: Math.round(t.height / pixel),
      silkEnd: Math.round((t.top - frame.top) / pixel),
      legs: shown('.ss-web-legs-a') && !shown('.ss-web-legs-b') ? 'a' : shown('.ss-web-legs-b') && !shown('.ss-web-legs-a') ? 'b' : 'both or none',
    };
  }, fraction);

  const rest = await at(0);
  expect(rest.animations).toBe(4);
  expect(rest).toMatchObject({ spiderTop: 29, silk: 17, silkEnd: 12, legs: 'a' });
  expect(await at(0.53)).toMatchObject({ legs: 'b' });
  expect(await at(0.55)).toMatchObject({ legs: 'a' });
  expect(await at(0.7)).toMatchObject({ spiderTop: 23, silk: 11, silkEnd: 12, legs: 'a' });
  expect(await at(0.83)).toMatchObject({ legs: 'b' });
  expect(await at(0.99)).toMatchObject({ spiderTop: 29, silk: 17, legs: 'a' });

  // The reduced-motion rule stops each of them (the harness cannot emulate the media query).
  const still = await browser.evaluate(() => {
    const stopped: string[] = [];
    const visit = (rules: CSSRuleList, reduced: boolean) => {
      for (const rule of rules) {
        if (rule instanceof CSSMediaRule) visit(rule.cssRules, reduced || rule.conditionText.includes('prefers-reduced-motion: reduce'));
        else if (reduced && rule instanceof CSSStyleRule && rule.style.animationName === 'none') stopped.push(rule.selectorText);
      }
    };
    for (const sheet of document.styleSheets) visit(sheet.cssRules, false);
    return stopped.join(', ');
  });
  for (const part of ['.ss-web-spider', '.ss-web-silk', '.ss-web-legs']) expect(still).toContain(part);
});

// Task #86 (SPEC-101): "sometimes pixelart cockroach should run through the screen in this view".
test('a cockroach runs across the visible part of the session now and then (unit, in the page)', async ({ app, browser }) => {
  await browser.setViewport({ width: 1280, height: 420 });
  await app.open(mock('/s/studio/sessions/ses_imp_checkout'));
  await expect(browser.locator('.ss-tui .ss-locked')).toBeVisible();
  const result = await browser.evaluate(async () => {
    const url = '/assets/pages/sessions.js';
    const { cockroachDelay, runCockroach, scheduleCockroaches } = await import(url);
    const delays = {
      firstMin: cockroachDelay(true, () => 0),
      firstMax: cockroachDelay(true, () => 0.999999),
      laterMin: cockroachDelay(false, () => 0),
      laterMax: cockroachDelay(false, () => 0.999999),
    };
    const page = document.querySelector<HTMLElement>('.ss-page')!;
    const well = document.querySelector<HTMLElement>('.ss-tui')!;
    page.scrollTop = 150;
    const view = page.getBoundingClientRect();
    const roach = runCockroach(well, () => 0.5) as SVGSVGElement;
    const [run] = roach.getAnimations();
    const placed = (fraction: number) => {
      run.pause();
      run.currentTime = fraction * Number(run.effect!.getComputedTiming().duration);
      const box = roach.getBoundingClientRect();
      const w = well.getBoundingClientRect();
      return { left: Math.round(box.left - w.left), right: Math.round(w.right - box.right), top: box.top, bottom: box.bottom, overflow: page.scrollWidth - page.clientWidth };
    };
    const start = placed(0);
    const middle = placed(0.5);
    const end = placed(0.999);
    const shape = {
      inWell: roach.parentElement === well,
      hidden: roach.getAttribute('aria-hidden'),
      pointer: getComputedStyle(roach).pointerEvents,
      legs: roach.querySelectorAll('.ss-roach-legs').length,
      duration: Number(run.effect!.getComputedTiming().duration),
    };
    run.finish();
    await run.finished;
    await new Promise((resolve) => requestAnimationFrame(resolve));
    const removed = !roach.isConnected;

    // It also runs the other way, facing where it goes.
    const back = runCockroach(well, () => 0.2) as SVGSVGElement;
    const [backRun] = back.getAnimations();
    const backAt = (fraction: number) => {
      backRun.pause();
      backRun.currentTime = fraction * Number(backRun.effect!.getComputedTiming().duration);
      const box = back.getBoundingClientRect();
      const w = well.getBoundingClientRect();
      return { left: Math.round(box.left - w.left), right: Math.round(w.right - box.right), flipped: getComputedStyle(back).transform.startsWith('matrix(-1') };
    };
    const leftward = [backAt(0), backAt(0.5), backAt(0.999)];
    back.remove();
    // At the lowest place it can take it is still all in view.
    const low = runCockroach(well, () => 0.999999) as SVGSVGElement;
    const lowBottom = low.getBoundingClientRect().bottom;
    low.remove();
    // A hidden page sends none.
    Object.defineProperty(document, 'hidden', { configurable: true, get: () => true });
    const whileHidden = { roach: runCockroach(well, () => 0.5), shown: well.querySelectorAll('.ss-roach').length };
    delete (document as unknown as { hidden?: boolean }).hidden;

    // The scheduler on a fake timer: one cockroach per timer, then the next delay, until stopped.
    const timers: { fn: () => void; ms: number; cleared: boolean }[] = [];
    const timer = { set: (fn: () => void, ms: number) => timers.push({ fn, ms, cleared: false }) - 1, clear: (id: number) => { if (timers[id]) timers[id].cleared = true; } };
    const stop = scheduleCockroaches(well, { random: () => 0.5, timer });
    const scheduled = timers.map((t) => t.ms);
    timers[0].fn();
    const ran = well.querySelectorAll('.ss-roach').length;
    const next = timers.map((t) => t.ms);
    stop();
    const afterStop = { cleared: timers[1].cleared, left: well.querySelectorAll('.ss-roach').length };
    timers[1].fn();
    const stale = { timers: timers.length, roaches: well.querySelectorAll('.ss-roach').length };
    return { delays, view: { top: view.top, bottom: view.bottom }, start, middle, end, leftward, shape, removed, lowBottom, whileHidden, scheduled, ran, next, afterStop, stale };
  });
  expect(result.delays).toEqual({ firstMin: 20000, firstMax: 59999, laterMin: 45000, laterMax: 149999 });
  expect(result.shape).toMatchObject({ inWell: true, hidden: 'true', pointer: 'none', legs: 2 });
  expect(result.shape.duration).toBeGreaterThanOrEqual(2000);
  expect(result.shape.duration).toBeLessThanOrEqual(4000);
  // It enters at one side, crosses the well and leaves at the other, in the part one can see.
  expect(result.start.right).toBeGreaterThan(0);
  expect(result.start.left).toBeLessThan(0);
  expect(result.middle.left).toBeGreaterThan(0);
  expect(result.middle.right).toBeGreaterThan(0);
  expect(result.end.left).toBeGreaterThan(result.middle.left);
  expect(result.middle.top).toBeGreaterThanOrEqual(result.view.top);
  expect(result.middle.bottom).toBeLessThanOrEqual(result.view.bottom);
  for (const at of [result.start, result.middle, result.end]) expect(at.overflow).toBe(0);
  expect(result.removed).toBe(true);
  const [enter, cross, leave] = result.leftward;
  expect(enter.left).toBeGreaterThan(0);
  expect(enter.right).toBeLessThan(0);
  expect(cross.left).toBeGreaterThan(0);
  expect(cross.right).toBeGreaterThan(0);
  expect(leave.left).toBeLessThan(cross.left);
  expect(result.leftward.every((at) => at.flipped)).toBe(true);
  expect(result.lowBottom).toBeLessThanOrEqual(result.view.bottom);
  expect(result.whileHidden).toEqual({ roach: null, shown: 0 });
  expect(result.scheduled).toEqual([40000]);
  expect(result.ran).toBe(1);
  expect(result.next).toEqual([40000, 97500]);
  expect(result.afterStop).toEqual({ cleared: true, left: 0 });
  expect(result.stale).toEqual({ timers: 2, roaches: 0 });
});

// The session view schedules them itself and stops when it is left; nothing runs for people who
// asked for reduced motion (the harness cannot emulate the media query, so matchMedia is stubbed).
const recordTimers = (reduce: boolean) => {
  const w = window as unknown as { roachTimers: { ms: number; fn: () => void; cleared: boolean; id: number }[] };
  w.roachTimers = [];
  const set = window.setTimeout.bind(window);
  const clear = window.clearTimeout.bind(window);
  window.setTimeout = ((fn: () => void, ms?: number, ...rest: unknown[]) => {
    const id = set(fn, ms, ...rest);
    // The app keeps other long timers (a request's timeout); only those set in the session view count.
    if ((ms ?? 0) >= 20000 && new Error().stack?.split('\n')[2]?.includes('/pages/sessions.js')) w.roachTimers.push({ ms: ms ?? 0, fn, cleared: false, id: Number(id) });
    return id;
  }) as typeof window.setTimeout;
  window.clearTimeout = ((id?: number) => {
    for (const t of w.roachTimers) if (t.id === id) t.cleared = true;
    clear(id);
  }) as typeof window.clearTimeout;
  if (reduce) {
    const media = window.matchMedia.bind(window);
    window.matchMedia = (query: string) => (query.includes('prefers-reduced-motion') ? ({ ...media(query), matches: true, media: query } as MediaQueryList) : media(query));
  }
};

test('the session view sends cockroaches now and then and stops when it is left', async ({ app, browser, screen }) => {
  await browser.addInitScript(recordTimers, false);
  await app.open(mock('/s/studio/sessions/ses_imp_checkout'));
  await expect(browser.locator('.ss-tui .ss-locked')).toBeVisible();
  const timers = () => browser.evaluate(() => (window as unknown as { roachTimers: { ms: number; cleared: boolean }[] }).roachTimers.map((t) => ({ ms: t.ms, cleared: t.cleared })));
  await expect.poll(async () => (await timers()).length).toBe(1);
  const [first] = await timers();
  expect(first.ms).toBeGreaterThanOrEqual(20000);
  expect(first.ms).toBeLessThan(60000);
  // Its time comes: a cockroach runs and the next one is scheduled further apart.
  // Run the first by hand, its real timer cleared so that it cannot fire as well.
  await browser.evaluate(() => {
    const [first] = (window as unknown as { roachTimers: { fn: () => void; id: number }[] }).roachTimers;
    window.clearTimeout(first.id);
    first.fn();
  });
  await expect(browser.locator('.ss-tui > svg.ss-roach')).toHaveCount(1);
  const [, second] = await timers();
  expect(second.ms).toBeGreaterThanOrEqual(45000);
  expect(second.ms).toBeLessThan(150000);
  expect(await axeViolations(browser)).toEqual([]);
  // Leaving the session stops the next one.
  await screen.getByRole('link', 'Sessions').first().click();
  await expect(browser).toHaveURL(/#\/s\/studio\/sessions$/);
  await expect.poll(async () => (await timers())[1].cleared).toBe(true);
});

test('no cockroach runs under reduced motion', async ({ app, browser }) => {
  await browser.addInitScript(recordTimers, true);
  await app.open(mock('/s/studio/sessions/ses_imp_checkout'));
  await expect(browser.locator('.ss-tui .ss-locked')).toBeVisible();
  await expect.poll(() => browser.evaluate(() => (window as unknown as { roachTimers: unknown[] }).roachTimers.length)).toBeGreaterThan(0);
  const ran = await browser.evaluate(async () => {
    const [first] = (window as unknown as { roachTimers: { fn: () => void; id: number }[] }).roachTimers;
    window.clearTimeout(first.id);
    first.fn();
    const url = '/assets/pages/sessions.js';
    const { runCockroach } = await import(url);
    return { direct: runCockroach(document.querySelector('.ss-tui')), shown: document.querySelectorAll('.ss-roach').length };
  });
  expect(ran).toEqual({ direct: null, shown: 0 });
});

// F 2026-10-13 (SPEC-086): "Sessions are beautiful but should be always 100% width and not only
// when minimised. Even left and right margin of the session window and border radius should be
// thrown out to make it look similar to the original session." The well spans the content area
// edge to edge and reaches its bottom, beside the Superagent too, in both themes and on a phone;
// the header keeps the page's padding.
test('a session fills the content area edge to edge, beside the Superagent and on a phone', async ({ app, browser, screen }) => {
  await browser.setViewport({ width: 1920, height: 1000 });
  await app.open(mock('/s/studio/sessions/ses_imp_checkout'));
  await expect(browser.locator('.ss-tui .ss-locked')).toBeVisible();
  const fit = () => browser.evaluate(() => {
    const page = document.querySelector<HTMLElement>('.ss-page')!;
    const well = document.querySelector('.ss-tui')!;
    const style = getComputedStyle(well);
    page.scrollTop = page.scrollHeight;
    const p = page.getBoundingClientRect();
    const w = well.getBoundingClientRect();
    const inner = { left: p.left + page.clientLeft, right: p.left + page.clientLeft + page.clientWidth, bottom: p.top + page.clientTop + page.clientHeight };
    return {
      left: Math.round(w.left - inner.left),
      right: Math.round(inner.right - w.right),
      bottom: Math.round(inner.bottom - w.bottom),
      radius: [style.borderTopLeftRadius, style.borderTopRightRadius, style.borderBottomRightRadius, style.borderBottomLeftRadius].join(' '),
      sides: `${style.borderLeftWidth} ${style.borderRightWidth}`,
      head: Math.round(document.querySelector('.ss-page .gt-back')!.getBoundingClientRect().left - inner.left),
      width: Math.round(inner.right - inner.left),
      sideways: document.documentElement.scrollWidth > window.innerWidth || page.scrollWidth > page.clientWidth,
      band: style.backgroundColor !== getComputedStyle(page).backgroundColor,
    };
  });
  const edgeToEdge = { left: 0, right: 0, bottom: 0, radius: '0px 0px 0px 0px', sides: '0px 0px', sideways: false, band: true };
  const wide = await fit();
  expect(wide).toMatchObject({ ...edgeToEdge, head: 32 });
  expect(wide.width).toBeGreaterThan(1200);

  // Beside the Superagent the content area is narrower; the well still fills it.
  await screen.getByRole('button', 'Show Superagent').click();
  await expect(screen.getByRole('region', 'Tool panel')).toBeVisible();
  const beside = await fit();
  expect(beside).toMatchObject(edgeToEdge);
  expect(beside.width).toBeLessThan(wide.width);
  await screen.getByRole('button', 'Hide Superagent').click();
  await expect(screen.getByRole('region', 'Tool panel')).toBeHidden();

  for (const theme of ['light', 'dark']) {
    await browser.evaluate((name) => document.documentElement.setAttribute('data-theme', name), theme);
    expect(await fit(), theme).toMatchObject(edgeToEdge);
    expect(await axeViolations(browser), theme).toEqual([]);
  }

  await browser.setViewport({ width: 390, height: 844 });
  await expect.poll(async () => (await fit()).head).toBe(12);
  expect(await fit()).toMatchObject(edgeToEdge);
  expect(await axeViolations(browser)).toEqual([]);
});

// Review round 3: an entry's content-visibility clips paint at its edge, so focus and the search
// target are drawn inside the box, where they stay visible (WCAG 2.4.7).
test('a folded call shows its keyboard focus inside the entry, and the search target too', async ({ app, browser }) => {
  await app.open(mock('/s/studio/sessions/ses_imp_checkout?line=5'));
  await expect(browser.locator('.ss-target')).toBeVisible();
  await browser.locator('.ss-tui details.ss-tool > summary').filter({ hasText: 'npm test -- checkout' }).focus();
  const ring = await browser.evaluate(() => {
    const summary = document.activeElement as HTMLElement;
    const style = getComputedStyle(summary);
    const target = getComputedStyle(document.querySelector('.ss-target')!);
    return {
      focused: summary.matches('.ss-tool > summary:focus-visible'),
      inset: style.boxShadow.includes('inset'),
      outside: style.outlineStyle !== 'none' && parseFloat(style.outlineOffset) > -parseFloat(style.outlineWidth),
      targetInset: target.boxShadow.includes('inset'),
    };
  });
  expect(ring).toEqual({ focused: true, inset: true, outside: false, targetInset: true });
});

// Review round 4: a long diff scrolls and takes the keyboard; its ring is drawn inside too.
test('a long diff that scrolls shows its keyboard focus', async ({ app, browser }) => {
  await app.open(mock('/s/studio/sessions/ses_task_4'));
  await expect(browser.locator('.ss-tui details.ss-tool > summary').filter({ hasText: 'Write(src/webhooks/fixtures.ts)' })).toContainText('Wrote 60 lines');
  await browser.locator('.ss-tui .ss-diff[aria-label="Changes to src/webhooks/fixtures.ts"] .ss-diff-hunk').focus();
  const ring = await browser.evaluate(() => {
    const hunk = document.activeElement as HTMLElement;
    const style = getComputedStyle(hunk);
    return { scrolls: hunk.scrollHeight > hunk.clientHeight, hunk: hunk.matches('.ss-diff-hunk:focus-visible'), inset: style.boxShadow.includes('inset'), clipped: getComputedStyle(hunk.closest('.ss-diff')!).overflow === 'hidden' };
  });
  expect(ring).toEqual({ scrolls: true, hunk: true, inset: true, clipped: false });
  // A scroller is a named region (as the Git page's diffs); short blocks are no tab stops (review round 5).
  await expect(browser.locator('.ss-tui .ss-diff-hunk[role="region"][aria-label="Changed lines of src/webhooks/fixtures.ts"]')).toHaveCount(1);
  // The other scroller is the folded skill body (SPEC-090).
  await expect(browser.locator('.ss-tui [tabindex="0"]')).toHaveCount(2);
  await expect(browser.locator('.ss-tui .ss-note-text[tabindex="0"]')).toHaveCount(1);
});

test('a search match opens the transcript at its line', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/sessions?q=fulfilling'));
  await screen.getByRole('list', 'Sessions').getByRole('link', /Add Stripe checkout/).click();
  await expect(browser).toHaveURL(/sessions\/ses_imp_checkout\?line=7/);
  await expect(browser.locator('.ss-target')).toContainText('verifies signatures before fulfilling orders');
  await expect(screen.getByRole('link', 'Show from the start')).toBeVisible();
});
