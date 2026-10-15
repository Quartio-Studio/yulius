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
