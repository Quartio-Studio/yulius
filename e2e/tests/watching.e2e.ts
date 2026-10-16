import { test, type Browser } from '@e2e-dev/web';
import { expect } from 'e2e';
import { axeViolations, failOnPageErrors, mock, terminalShown, touchTask } from './support.ts';

failOnPageErrors();

// Someone else's task is watched as its live transcript (SPEC-179): the owner's terminal is drawn
// for the owner's window, so a watcher reads the session as the Sessions view prints it, as it
// grows. The mock signs in as mkarp; studio #12 is mkarp's running task with a session of four
// entries, and handing it to anna-q makes mkarp a watcher.

type Item = { role: string, text?: string, blocks?: unknown[] };
type Mocked = { __yuliusMock: { appendTranscript: (sid: string, n: number, items: Item[]) => void, transcriptReads?: number } };
const append = (browser: Browser, items: Item[]) => browser.evaluate((list) => (window as unknown as Mocked).__yuliusMock.appendTranscript('sb_studio', 12, list), items);
const reads = (browser: Browser) => browser.evaluate(() => (window as unknown as Mocked).__yuliusMock.transcriptReads ?? 0);
const xterms = (browser: Browser) => browser.evaluate(() => document.querySelectorAll('.taskview .xterm').length);

async function watchStudio12(app: { open: (url: string) => Promise<unknown> }, browser: Browser): Promise<void> {
  await app.open(mock('/s/studio/t/12'));
  await terminalShown(browser);
  await touchTask(browser, 'sb_studio', 12, { owner: { login: 'anna-q' } });
  await expect(browser.locator('.taskview .watch .ss-tui')).toBeVisible();
}

test("someone else's task shows its live transcript instead of the terminal, and the terminal comes back with control", async ({ app, browser, screen }) => {
  await watchStudio12(app, browser);
  await expect(screen.getByText('Add Stripe checkout to the cart page. Use the test keys from .env.')).toBeVisible();
  await expect.poll(() => xterms(browser), { message: 'a watcher opens no terminal' }).toBe(0);
  await expect(screen.getByText('Read-only: @anna-q owns this task')).toBeHidden();
  // Where the prompt box was: who types, and that this person watches. Not a control.
  await expect(browser.locator('.watch .ss-locked')).toHaveText(/@anna-q is at the keyboard · you are watching/);

  // The session writes on: a prompt and an answer appear without a reload.
  await append(browser, [{ role: 'user', text: 'Please add Apple Pay too.' }, { role: 'assistant', text: 'Adding Apple Pay to the checkout.' }]);
  await expect(screen.getByText('Adding Apple Pay to the checkout.')).toBeVisible();
  await expect(browser.locator('.watch .ss-prompt:has-text("Please add Apple Pay too.")')).toBeVisible();
  // A call, then its result in a later read: the result shows inside its call, not on its own.
  await append(browser, [{ role: 'assistant', blocks: [{ type: 'tool_call', id: 'live1', name: 'Bash', summary: 'npm test', input: '{}', diff: null }] }]);
  await expect(browser.locator('.watch .ss-tool:has-text("npm test")')).toBeVisible();
  await append(browser, [{ role: 'user', blocks: [{ type: 'tool_result', tool_use_id: 'live1', text: 'PASS apple-pay.test.ts', is_error: false, truncated: false }] }]);
  await expect.poll(() => browser.evaluate(() => {
    const call = [...document.querySelectorAll('.watch .ss-tool')].find((el) => el.textContent!.includes('npm test'));
    return call ? call.textContent!.includes('PASS apple-pay.test.ts') : false;
  }), { message: 'the result lands inside its call' }).toBe(true);
  expect(await browser.evaluate(() => document.querySelectorAll('.watch .ss-entries > li').length), 'the result folded into its call is no entry of its own').toBe(6);
  expect(await axeViolations(browser)).toEqual([]);

  // anna-q hands it back: the terminal returns; handing it away again brings the transcript back.
  await touchTask(browser, 'sb_studio', 12, { owner: { login: 'mkarp' } });
  await terminalShown(browser);
  await expect(browser.locator('.taskview .watch')).toBeHidden();
  await touchTask(browser, 'sb_studio', 12, { owner: { login: 'anna-q' } });
  await expect(browser.locator('.taskview .watch .ss-tui')).toBeVisible();
  await expect.poll(() => xterms(browser)).toBe(0);
  await expect(screen.getByText('Adding Apple Pay to the checkout.')).toBeVisible();
});

test('the watched transcript follows the bottom, and stays put with "New messages" when scrolled up', async ({ app, browser, screen }) => {
  await watchStudio12(app, browser);
  const many = Array.from({ length: 30 }, (_, i) => ({ role: 'assistant', text: `Working through file ${i + 1} of 30.` }));
  await append(browser, many);
  await expect(screen.getByText('Working through file 30 of 30.')).toBeVisible();
  const scroll = () => browser.evaluate(() => {
    const box = document.querySelector('.watch .watch-scroll')!;
    return { top: Math.round(box.scrollTop), bottom: Math.round(box.scrollHeight - box.scrollTop - box.clientHeight) };
  });
  await expect.poll(async () => (await scroll()).bottom, { message: 'at the bottom it follows' }).toBeLessThan(4);

  await browser.evaluate(() => { document.querySelector('.watch .watch-scroll')!.scrollTop = 0; });
  await append(browser, [{ role: 'assistant', text: 'All 30 files are done.' }]);
  await expect(screen.getByRole('button', 'New messages')).toBeVisible();
  expect((await scroll()).top, 'scrolled up, the reader stays where they read').toBe(0);
  await screen.getByRole('button', 'New messages').click();
  await expect.poll(async () => (await scroll()).bottom).toBeLessThan(4);
  await expect(screen.getByRole('button', 'New messages')).toBeHidden();
});

// Task #170: the view scrolled down only when lines came, and took "at the bottom" from where it
// was then. "✻ Working…" showing below the last line (its margin and a line, more than the slack
// once lines are taller than 20 px) or a shorter view left it above the bottom, and the next lines
// came with "New messages" instead of the view scrolling down.
test('at the bottom the view keeps following when "Working…" shows, the view gets shorter or lines come after', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/t/12'));
  await terminalShown(browser);
  await browser.evaluate(() => { document.documentElement.style.setProperty('--lh', '24px'); });
  await touchTask(browser, 'sb_studio', 12, { owner: { login: 'anna-q' } });
  await expect(browser.locator('.taskview .watch .ss-tui')).toBeVisible();
  // As in Safari, which has no scroll anchoring: Chromium's sometimes moved the view down with what
  // grew below it, and hid the defect.
  await browser.evaluate(() => { (document.querySelector('.watch .watch-scroll') as HTMLElement).style.overflowAnchor = 'none'; });
  // Idle: the control card keeps its height, so only the transcript changes below.
  await touchTask(browser, 'sb_studio', 12, { status: 'running', activity: 'idle' });
  await expect(browser.locator('.watch .watch-working')).toBeHidden();
  await append(browser, Array.from({ length: 30 }, (_, i) => ({ role: 'assistant', text: `Step ${i + 1} of 30.` })));
  await expect(screen.getByText('Step 30 of 30.')).toBeVisible();
  const below = () => browser.evaluate(() => {
    const box = document.querySelector('.watch .watch-scroll')!;
    return Math.round(box.scrollHeight - box.scrollTop - box.clientHeight);
  });
  await expect.poll(below).toBeLessThan(4);

  await touchTask(browser, 'sb_studio', 12, { activity: 'working' });
  await expect(browser.locator('.watch .watch-working')).toBeVisible();
  await expect.poll(below, { message: '"Working…" appearing keeps the view at the bottom' }).toBeLessThan(4);

  await browser.evaluate(() => { (document.querySelector('.watch') as HTMLElement).style.maxHeight = '240px'; });
  await expect.poll(below, { message: 'a shorter view stays at the bottom' }).toBeLessThan(4);

  await append(browser, [{ role: 'assistant', text: 'Step 31, after the agent went on working.' }]);
  await expect(browser.locator('.watch .ss-entry:has-text("Step 31, after the agent went on working.")')).toBeVisible();
  await expect.poll(below, { message: 'new lines are followed' }).toBeLessThan(4);
  await expect(screen.getByRole('button', 'New messages')).toBeHidden();

  // Scrolled up, a height that changes leaves the reader where they read.
  await browser.evaluate(() => { document.querySelector('.watch .watch-scroll')!.scrollTop = 0; });
  await touchTask(browser, 'sb_studio', 12, { activity: 'idle' });
  await expect(browser.locator('.watch .watch-working')).toBeHidden();
  await browser.evaluate(() => { (document.querySelector('.watch') as HTMLElement).style.maxHeight = '200px'; });
  await touchTask(browser, 'sb_studio', 12, { activity: 'working' });
  await expect(browser.locator('.watch .watch-working')).toBeVisible();
  await browser.evaluate(() => new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done))));
  expect(await browser.evaluate(() => document.querySelector('.watch .watch-scroll')!.scrollTop)).toBe(0);
});

// The review of #575's fix: the Working line hiding at the end of each turn (the end clamps the view
// up) keeps it following; scrolling back down by hand follows again; and a reader left within the
// slack of the bottom by a shrink is at the bottom, so the next lines are followed.
test('following survives "Working…" hiding, comes back by scrolling down, and takes up a reader a shrink left at the bottom', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/t/12'));
  await terminalShown(browser);
  await browser.evaluate(() => { document.documentElement.style.setProperty('--lh', '24px'); });
  await touchTask(browser, 'sb_studio', 12, { owner: { login: 'anna-q' }, status: 'running', activity: 'working' });
  await expect(browser.locator('.taskview .watch .ss-tui')).toBeVisible();
  await browser.evaluate(() => { (document.querySelector('.watch .watch-scroll') as HTMLElement).style.overflowAnchor = 'none'; });
  await append(browser, Array.from({ length: 30 }, (_, i) => ({ role: 'assistant', text: `Turn ${i + 1} of 30.` })));
  await expect(browser.locator('.watch .ss-entry:has-text("Turn 30 of 30.")')).toBeVisible();
  await expect(browser.locator('.watch .watch-working')).toBeVisible();
  const box = () => browser.evaluate(() => {
    const el = document.querySelector('.watch .watch-scroll')!;
    return { top: Math.round(el.scrollTop), below: Math.round(el.scrollHeight - el.scrollTop - el.clientHeight) };
  });
  await expect.poll(async () => (await box()).below).toBeLessThan(4);

  // The turn ends: the Working line goes, and the end pulls the view up with it.
  await touchTask(browser, 'sb_studio', 12, { activity: 'idle' });
  await expect(browser.locator('.watch .watch-working')).toBeHidden();
  await append(browser, [{ role: 'assistant', text: 'Turn 31, after a turn ended.' }]);
  await expect(browser.locator('.watch .ss-entry:has-text("Turn 31, after a turn ended.")')).toBeVisible();
  await expect.poll(async () => (await box()).below, { message: 'still following after the Working line went' }).toBeLessThan(4);
  await expect(screen.getByRole('button', 'New messages')).toBeHidden();

  // Scrolled up, then back down by hand (each scroll handled, a frame apart): the next lines, taller
  // than the slack together, are followed.
  const frame = () => browser.evaluate(() => new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done))));
  await browser.evaluate(() => { document.querySelector('.watch .watch-scroll')!.scrollTop = 0; });
  await frame();
  await browser.evaluate(() => { const el = document.querySelector('.watch .watch-scroll')!; el.scrollTop = el.scrollHeight; });
  await frame();
  await append(browser, Array.from({ length: 4 }, (_, i) => ({ role: 'assistant', text: `Back at the bottom, line ${i + 1}.` })));
  await expect(browser.locator('.watch .ss-entry:has-text("Back at the bottom, line 4.")')).toBeVisible();
  await expect.poll(async () => (await box()).below, { message: 'back at the bottom, it follows' }).toBeLessThan(4);
  await expect(screen.getByRole('button', 'New messages')).toBeHidden();
  await touchTask(browser, 'sb_studio', 12, { activity: 'working' });
  await expect(browser.locator('.watch .watch-working')).toBeVisible();
  await expect.poll(async () => (await box()).below).toBeLessThan(4);

  // 50 px up is reading; the Working line going leaves the reader 14 px from the bottom, at it.
  await browser.evaluate(() => { document.querySelector('.watch .watch-scroll')!.scrollTop -= 50; });
  await frame(); // its scroll event is handled before the line goes
  expect((await box()).below).toBeGreaterThan(40);
  await touchTask(browser, 'sb_studio', 12, { activity: 'idle' });
  await expect(browser.locator('.watch .watch-working')).toBeHidden();
  await append(browser, [{ role: 'assistant', text: 'Turn 32, seen from near the bottom.' }]);
  await expect(browser.locator('.watch .ss-entry:has-text("Turn 32, seen from near the bottom.")')).toBeVisible();
  await expect.poll(async () => (await box()).below, { message: 'a reader at the bottom again is followed' }).toBeLessThan(4);
  await expect(screen.getByRole('button', 'New messages')).toBeHidden();
});

test('a burst longer than a page is read to its end', async ({ app, browser, screen }) => {
  await watchStudio12(app, browser);
  await expect(screen.getByText('Add Stripe checkout to the cart page. Use the test keys from .env.')).toBeVisible();
  // 250 entries at once, after the first read: one page of 200, then the rest from where it stopped.
  await append(browser, Array.from({ length: 250 }, (_, i) => ({ role: 'assistant', text: `Burst line ${i + 1}.` })));
  await expect(screen.getByText('Burst line 250.')).toBeVisible();
  expect(await browser.evaluate(() => document.querySelectorAll('.watch .ss-entries > li').length)).toBe(3 + 250);
});

test('a watcher asks for new lines while the task is live, and once more when it ends', async ({ app, browser }) => {
  await watchStudio12(app, browser);
  const state = () => browser.evaluate(() => (document.querySelector('.taskview .watch') as HTMLElement).dataset.reading ?? null);
  expect(await state()).toBe('live');
  const before = await reads(browser);
  await expect.poll(() => reads(browser), { message: 'a live task is read again' }).toBeGreaterThan(before);
  // The task ends: it is read once more, with the session's last lines, and no read is planned.
  await append(browser, [{ role: 'assistant', text: 'The session’s last words.' }]);
  await touchTask(browser, 'sb_studio', 12, { status: 'stopped_cancelled', activity: null });
  await expect(browser.locator('.watch .ss-entry:has-text("The session’s last words.")')).toBeVisible();
  await expect.poll(state, { message: 'an ended task is not read again' }).toBe('ended');
  await expect(browser.locator('.watch .ss-locked')).toHaveText(/@anna-q owns this task · you are watching/);
});

test('a watched session keeps the live terminal’s colours, not a past session’s faded ones, AA in both themes', async ({ app, browser }) => {
  await watchStudio12(app, browser);
  for (const theme of ['dark', 'light']) {
    const m = await browser.evaluate((name) => {
      document.documentElement.setAttribute('data-theme', name);
      const well = document.querySelector('.watch .ss-tui')!;
      const color = (token: string, inside: Element) => {
        const probe = inside.appendChild(document.createElement('span'));
        probe.style.color = `var(${token})`;
        const value = getComputedStyle(probe).color;
        probe.remove();
        return value;
      };
      const rgb = (value: string) => value.match(/\d+(\.\d+)?/g)!.slice(0, 3).map(Number);
      const lum = (c: number[]) => {
        const [r, g, b] = c.map((v) => { const s = v / 255; return s <= 0.03928 ? s / 12.92 : ((s + 0.055) / 1.055) ** 2.4; });
        return 0.2126 * r + 0.7152 * g + 0.0722 * b;
      };
      const contrast = (a: string, b: string) => { const [x, y] = [lum(rgb(color(a, well))), lum(rgb(color(b, well)))].sort((p, q) => q - p); return (x + 0.05) / (y + 0.05); };
      const page = document.body;
      return {
        bg: getComputedStyle(well).backgroundColor, termBg: color('--term-bg', page),
        fg: getComputedStyle(well).color, termFg: color('--term-fg', page),
        ok: color('--session-ok', well), green: color('--ansi-green', page),
        err: color('--session-err', well), red: color('--ansi-red', page),
        faded: color('--session-fg', page),
        pairs: [contrast('--session-dim', '--session-bg'), contrast('--session-fg', '--session-prompt-bg'), contrast('--session-dim', '--session-prompt-bg'), contrast('--session-fg', '--session-add-bg'), contrast('--session-fg', '--session-del-bg'), contrast('--session-ok', '--session-bg'), contrast('--session-err', '--session-bg')],
      };
    }, theme);
    expect([m.bg, m.fg], theme).toEqual([m.termBg, m.termFg]);
    expect([m.ok, m.err], theme).toEqual([m.green, m.red]);
    expect(m.fg, `${theme}: not the faded session colour`).not.toBe(m.faded);
    for (const ratio of m.pairs) expect(ratio, theme).toBeGreaterThanOrEqual(4.5);
  }
  expect(await axeViolations(browser)).toEqual([]);
});

test('the view says whose task it is and that you watch it, above the transcript; the sidebar marks other people’s tasks', async ({ app, browser, screen }) => {
  await watchStudio12(app, browser);
  await expect(screen.getByText("You are watching @anna-q's task.")).toBeVisible();
  await expect(browser.locator('.attention-bar .attention-text')).toHaveText("You are watching @anna-q's task. Only @anna-q can type in it; it acts on GitHub as them.");
  await expect(screen.getByRole('button', 'Request control')).toBeVisible();
  // In the flow above the transcript: it covers no line of it.
  expect(await browser.evaluate(() => {
    const bar = document.querySelector('.taskview .attention-bar')!.getBoundingClientRect();
    const view = document.querySelector('.taskview .watch')!.getBoundingClientRect();
    return Math.round(view.top - bar.bottom) >= 0;
  })).toBe(true);

  // The sidebar: anna-q's initials on her task, said in words; none on mkarp's own tasks.
  const mark = browser.locator('.task-row:has(.task-num:has-text("#12")) .task-owner');
  await expect(mark).toHaveText('AQ');
  expect(await browser.evaluate(() => {
    const row = [...document.querySelectorAll('.task-row')].find((r) => r.querySelector('.task-num')!.textContent!.endsWith('#12'))!;
    return [row.querySelector('.task-owner')!.getAttribute('title'), row.querySelector('.task-link')!.getAttribute('title')!.includes('owned by @anna-q'), row.querySelector('.task-link .sr-only:not(.task-num .sr-only)')!.textContent!.includes('owned by @anna-q')];
  })).toEqual(['@anna-q owns this task', true, true]);
  expect(await browser.evaluate(() => [...document.querySelectorAll('.task-row')].filter((r) => !(r.querySelector('.task-owner') as HTMLElement).hidden).length), 'only someone else’s task is marked').toBe(1);
  expect(await axeViolations(browser)).toEqual([]);
  // Handed back: the mark goes.
  await touchTask(browser, 'sb_studio', 12, { owner: { login: 'mkarp' } });
  await expect(mark).toBeHidden();
});

// Task #170: "also reduce horizontal padding in this view". The watched transcript is inset as the
// control card above it (16 px), not as a past session's page (32 px); phones keep their 12 px.
test('a watched transcript has 16 px at its sides, as the card above it; a past session keeps 32 px', async ({ app, browser }) => {
  await watchStudio12(app, browser);
  const sides = () => browser.evaluate(() => {
    const style = getComputedStyle(document.querySelector('.ss-tui')!);
    return [style.paddingLeft, style.paddingRight];
  });
  expect(await sides()).toEqual(['16px', '16px']);
  await expect(browser.locator('.watch .ss-entry').first()).toBeVisible();
  expect(await browser.evaluate(() => {
    const card = document.querySelector('.taskview-term.watching > .attention-bar')!.getBoundingClientRect();
    const entry = document.querySelector('.watch .ss-entry')!.getBoundingClientRect();
    return Math.round(entry.left - card.left);
  }), 'the transcript’s entries start where the card does').toBe(0);
  await browser.setViewport({ width: 390, height: 844 });
  await expect.poll(sides).toEqual(['12px', '12px']);
  await browser.setViewport({ width: 1440, height: 900 });
  await app.open(mock('/s/studio/sessions/ses_imp_checkout'));
  await expect(browser.locator('.ss-tui .ss-locked')).toBeVisible();
  expect(await sides()).toEqual(['32px', '32px']);
});

// Task #170: "remove the octopus, it was an overkill" (SPEC-185 OCT-001). The transcript starts at
// the top of its well, with no lane above it; past sessions keep their spider.
test('a watched task has no octopus and no lane: its first line is at the top of the transcript', async ({ app, browser }) => {
  await watchStudio12(app, browser);
  await expect(browser.locator('.watch .ss-entry').first()).toBeVisible();
  const view = await browser.evaluate(() => {
    const tui = document.querySelector('.watch .ss-tui')!;
    const entry = document.querySelector('.watch .ss-entry')!;
    return {
      octopus: document.querySelectorAll('.octo').length,
      lane: document.querySelectorAll('.watch-lane').length,
      gap: Math.round(entry.getBoundingClientRect().top - tui.getBoundingClientRect().top - parseFloat(getComputedStyle(tui).paddingTop)),
    };
  });
  expect(view.octopus).toBe(0);
  expect(view.lane).toBe(0);
  expect(view.gap, 'the first entry sits at the top of the well').toBeLessThan(20);
  await app.open(mock('/s/studio/sessions/ses_imp_checkout'));
  await expect(browser.locator('.ss-tui .ss-locked')).toBeVisible();
  expect(await browser.evaluate(() => [document.querySelectorAll('.ss-tui .ss-web').length, document.querySelectorAll('.octo').length])).toEqual([1, 0]);
});

type Reads = { __yuliusMock: { failTranscriptReads?: number, transcriptQueries?: string[], newTranscriptSession: (sid: string, n: number, session: string, items: Item[]) => void } };

test('a first read that failed gives way to reading on, once the session comes', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/t/12'));
  await terminalShown(browser);
  await browser.evaluate(() => { (window as unknown as Reads).__yuliusMock.failTranscriptReads = 1; });
  await touchTask(browser, 'sb_studio', 12, { owner: { login: 'anna-q' } });
  await expect(screen.getByText(/Couldn’t load the session/)).toBeVisible();
  // A task event meanwhile neither hides why nor ends the reading.
  await touchTask(browser, 'sb_studio', 12, { activity: 'waiting' });
  await expect(screen.getByText(/Couldn’t load the session/)).toBeVisible();
  await screen.getByRole('button', 'Try again').click();
  await expect(screen.getByText('Add Stripe checkout to the cart page. Use the test keys from .env.')).toBeVisible();
  await append(browser, [{ role: 'assistant', text: 'Back after the restart.' }]);
  await expect(screen.getByText('Back after the restart.')).toBeVisible();
});

test('a failed first read retried by a task event reads on by itself', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/t/12'));
  await terminalShown(browser);
  await browser.evaluate(() => { (window as unknown as Reads).__yuliusMock.failTranscriptReads = 1; });
  await touchTask(browser, 'sb_studio', 12, { owner: { login: 'anna-q' } });
  await expect(screen.getByText(/Couldn’t load the session/)).toBeVisible();
  // The task ends and starts again: the view reads, and from then on by itself, with no event.
  await touchTask(browser, 'sb_studio', 12, { status: 'stopped_cancelled', activity: null });
  await expect(screen.getByText('Add Stripe checkout to the cart page. Use the test keys from .env.')).toBeVisible();
  await touchTask(browser, 'sb_studio', 12, { status: 'running', activity: 'working' });
  await expect.poll(() => browser.evaluate(() => (document.querySelector('.taskview .watch') as HTMLElement).dataset.reading)).toBe('live');
  // Reading on by itself: three more reads, more than the mock's own task events (one per 16 s) make.
  const after = await reads(browser);
  await expect.poll(() => reads(browser), { timeout: 10_000, message: 'it polls on its own' }).toBeGreaterThan(after + 2);
});

test('a watcher reads on with the size and session it saw, and from the tail of a new session', async ({ app, browser, screen }) => {
  await watchStudio12(app, browser);
  await expect(screen.getByText('Add Stripe checkout to the cart page. Use the test keys from .env.')).toBeVisible();
  await expect.poll(() => browser.evaluate(() => ((window as unknown as Reads).__yuliusMock.transcriptQueries ?? []).some((q) => /(^|&)from=\d+/.test(q) && /seen=\d+/.test(q) && /session=b1f0-12/.test(q))), { message: 'later reads say what was seen' }).toBe(true);
  // A continue started another session: the old lines go, the new session's tail comes.
  await browser.evaluate(() => (window as unknown as Reads).__yuliusMock.newTranscriptSession('sb_studio', 12, 'b1f0-13', [{ role: 'user', text: 'Continue where you left off.' }]));
  await expect(screen.getByText('Continue where you left off.')).toBeVisible();
  await expect(screen.getByText('Add Stripe checkout to the cart page. Use the test keys from .env.')).toBeHidden();
  expect(await browser.evaluate(() => document.querySelectorAll('.watch .ss-entries > li').length)).toBe(1);
});

test('a hidden page asks for nothing until it is shown again', async ({ app, browser }) => {
  await watchStudio12(app, browser);
  const state = () => browser.evaluate(() => (document.querySelector('.taskview .watch') as HTMLElement).dataset.reading ?? null);
  await expect.poll(state).toBe('live');
  await browser.evaluate(() => {
    Object.defineProperty(document, 'hidden', { configurable: true, get: () => true });
    document.dispatchEvent(new Event('visibilitychange'));
  });
  await expect.poll(state).toBe('paused');
  const reads = await browser.evaluate(() => (window as unknown as Mocked).__yuliusMock.transcriptReads ?? 0);
  await append(browser, [{ role: 'assistant', text: 'Written while nobody looked.' }]);
  await browser.evaluate(() => {
    Object.defineProperty(document, 'hidden', { configurable: true, get: () => false });
    document.dispatchEvent(new Event('visibilitychange'));
  });
  await expect(browser.locator('.watch .ss-entry:has-text("Written while nobody looked.")')).toBeVisible();
  expect(await browser.evaluate(() => (window as unknown as Mocked).__yuliusMock.transcriptReads ?? 0), 'shown again, it reads at once').toBeGreaterThan(reads);
  await expect.poll(state).toBe('live');
});

test('the watch view cancels its read when left, and task events do not put off the next read (unit, in the page)', async ({ app, browser }) => {
  await app.open(mock('/s/studio'));
  const result = await browser.evaluate(async () => {
    const url = '/assets/watchview.js';
    const { createWatchView } = await import(url);
    const task = { id: 't', status: 'running', activity: 'working' };
    const options = { taskId: 't', task: () => task, owner: () => 'anna-q', emptyMessage: () => null, sessionPath: () => '/x' };
    // Left with a read in flight: the read is cancelled.
    let signal: AbortSignal | null = null;
    const hanging = createWatchView({ api: { get: (_path: string, init: { signal: AbortSignal }) => { signal = init.signal; return new Promise(() => {}); } } }, options);
    hanging.start();
    hanging.destroy();
    // Events while a read waits on its timer keep that timer.
    const timers: { set: number, clear: number } = { set: 0, clear: 0 };
    const timer = { set: () => { timers.set += 1; return timers.set; }, clear: () => { timers.clear += 1; } };
    const page = { session_id: 's', doc_id: null, entries: [], next: null, end: 0, bytes: 0, earlier: false, skipped_lines: 0 };
    const view = createWatchView({ api: { get: async () => page } }, { ...options, timer });
    view.start();
    for (let i = 0; i < 20 && !timers.set; i += 1) await new Promise((resolve) => { setTimeout(resolve, 10); });
    const before = { ...timers };
    view.update();
    view.update();
    view.update();
    view.destroy();
    return { aborted: (signal as AbortSignal | null)?.aborted ?? null, before, after: timers };
  });
  expect(result.aborted).toBe(true);
  expect(result.before.set).toBe(1);
  expect(result.after.set, 'no new timer for an event').toBe(1);
});

test('results of several calls on one line each land in their call, and an opened fold stays open (unit, in the page)', async ({ app, browser }) => {
  await app.open(mock('/s/studio'));
  const result = await browser.evaluate(async () => {
    const url = '/assets/pages/sessions.js';
    const { transcriptEntries } = await import(url);
    const list = transcriptEntries();
    document.body.append(list.el);
    const call = (id: string, summary: string) => ({ type: 'tool_call', id, name: 'Bash', summary, input: '{}', diff: null });
    const result = (id: string, text: string) => ({ line: 0, role: 'user', at: null, sidechain: false, blocks: [{ type: 'tool_result', tool_use_id: id, text, is_error: false, truncated: false }] });
    list.append([{ line: 0, role: 'assistant', at: null, sidechain: false, blocks: [call('a', 'npm test'), call('b', 'npm run lint')] }]);
    (list.el.querySelector('details.ss-tool') as HTMLDetailsElement).open = true;
    list.append([{ ...result('a', 'PASS all tests'), line: 1 }]);
    const openAfterFirst = (list.el.querySelector('details.ss-tool') as HTMLDetailsElement).open;
    list.append([{ ...result('b', 'no lint errors'), line: 2 }]);
    const text = list.el.textContent;
    const out = { entries: list.el.children.length, a: text.includes('PASS all tests'), b: text.includes('no lint errors'), openAfterFirst, openAfterSecond: (list.el.querySelector('details.ss-tool') as HTMLDetailsElement).open };
    list.destroy();
    list.el.remove();
    return out;
  });
  expect(result).toEqual({ entries: 1, a: true, b: true, openAfterFirst: true, openAfterSecond: true });
});

test('an edit whose result comes in a later read opens on its diff, as one printed with it (unit, in the page)', async ({ app, browser }) => {
  await app.open(mock('/s/studio'));
  const result = await browser.evaluate(async () => {
    const url = '/assets/pages/sessions.js';
    const { transcriptEntries } = await import(url);
    const list = transcriptEntries();
    document.body.append(list.el);
    const diff = { path: 'src/a.ts', hunks: [{ old: 'a()', new: 'b()' }], truncated: false };
    list.append([{ line: 0, role: 'assistant', at: null, sidechain: false, blocks: [{ type: 'tool_call', id: 'e', name: 'Edit', summary: 'src/a.ts', input: '{}', diff }] }]);
    const before = (list.el.querySelector('details.ss-tool') as HTMLDetailsElement).open;
    list.append([{ line: 1, role: 'user', at: null, sidechain: false, blocks: [{ type: 'tool_result', tool_use_id: 'e', text: 'updated', is_error: false, truncated: false }] }]);
    const after = (list.el.querySelector('details.ss-tool') as HTMLDetailsElement).open;
    list.destroy();
    list.el.remove();
    return { before, after };
  });
  expect(result).toEqual({ before: false, after: true });
});

test('"New messages" keeps the keyboard in the transcript, and a late result alone shows no button', async ({ app, browser, screen }) => {
  await watchStudio12(app, browser);
  await append(browser, Array.from({ length: 30 }, (_, i) => ({ role: 'assistant', text: `Filler ${i + 1}.` })));
  await expect(screen.getByText('Filler 30.')).toBeVisible();
  await browser.evaluate(() => { document.querySelector('.watch .watch-scroll')!.scrollTop = 0; });
  // The Update call at the top gets its result: nothing new at the bottom, no button.
  await append(browser, [{ role: 'user', blocks: [{ type: 'tool_result', tool_use_id: 't2', text: 'The file src/api/checkout.ts has been updated.', is_error: false, truncated: false }] }]);
  await expect.poll(() => browser.evaluate(() => document.querySelector('.watch')!.textContent!.includes('Updated src/api/checkout.ts'))).toBe(true);
  await expect(screen.getByRole('button', 'New messages')).toBeHidden();
  await append(browser, [{ role: 'assistant', text: 'Something new at the bottom.' }]);
  await screen.getByRole('button', 'New messages').click();
  expect(await browser.evaluate(() => document.activeElement?.classList.contains('watch-scroll') ?? false)).toBe(true);
});

test('handing a task away while typing in its terminal leaves the keyboard on its transcript', async ({ app, browser }) => {
  await app.open(mock('/s/studio/t/12'));
  await terminalShown(browser);
  await browser.locator('.taskview .xterm-helper-textarea').focus();
  await touchTask(browser, 'sb_studio', 12, { owner: { login: 'anna-q' } });
  await expect(browser.locator('.taskview .watch .ss-tui')).toBeVisible();
  await expect.poll(() => browser.evaluate(() => document.activeElement?.classList.contains('watch-scroll') ?? false)).toBe(true);
});
