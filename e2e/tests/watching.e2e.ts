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

// "In archive, we have a spider. In other's owner, we need an electroblue octopus which is moving
// between corners, being sticked to the top, and moving tentacles in pixel animation. […] the face
// should be exactly like here": dot eyes and an ε mouth (WAT-006).
test('an electro-blue pixel octopus with dot eyes and an ε mouth crawls between the top corners of a watched task', async ({ app, browser, screen }) => {
  await watchStudio12(app, browser);
  const octo = () => browser.evaluate(() => {
    const el = document.querySelector('.watch .watch-lane > .octo') as HTMLElement | null;
    if (!el) return null;
    const lane = el.parentElement!.getBoundingClientRect();
    const box = el.getBoundingClientRect();
    const frames = (kind: string) => [...el.querySelectorAll(`.octo-${kind}`)].map((f) => getComputedStyle(f).visibility === 'visible');
    return {
      classes: [...el.classList].filter((c) => c.startsWith('is-')).sort(),
      corner: el.dataset.corner ?? null,
      left: Math.round(box.left - lane.left), right: Math.round(lane.right - box.right),
      crawl: frames('crawl'), rest: frames('rest'), moving: el.getAnimations().length,
    };
  });
  // Its face and body: two single-pixel eyes either side of the head's middle, an ε of 7 half pixels
  // below them, eight arms in every frame (crawl ×4, rest ×4, wave ×2), electro blue.
  const sprite = await browser.evaluate(() => {
    const svg = document.querySelector('.watch .octo svg')!;
    const eyes = [...svg.querySelectorAll('.octo-eye')].map((r) => [Number(r.getAttribute('x')), Number(r.getAttribute('width'))]);
    const mouth = [...svg.querySelectorAll('.octo-mouth')].map((r) => [Number(r.getAttribute('x')), Number(r.getAttribute('y'))]);
    const width = Number(svg.getAttribute('viewBox')!.split(' ')[2]);
    const body = getComputedStyle(svg.querySelector('.octo-body')!).fill;
    return {
      eyes, mouth: mouth.length, mouthLeft: Math.min(...mouth.map(([x]) => x)), mouthTop: Math.min(...mouth.map(([, y]) => y)),
      eyeRow: Number(svg.querySelector('.octo-eye')!.getAttribute('y')), middle: width / 2,
      frames: ['crawl', 'rest', 'wave'].map((kind) => svg.querySelectorAll(`.octo-frame.octo-${kind}`).length), body,
    };
  });
  expect(sprite.eyes.map(([, w]) => w)).toEqual([1, 1]);
  expect(sprite.middle - (sprite.eyes[0][0] + 0.5), 'eyes as far from the middle').toBe(sprite.eyes[1][0] + 0.5 - sprite.middle);
  expect(sprite.mouth).toBe(7);
  expect(sprite.mouthTop, 'the mouth below the eyes').toBeGreaterThan(sprite.eyeRow);
  expect(sprite.mouthLeft > sprite.eyes[0][0] && sprite.mouthLeft < sprite.eyes[1][0], 'the mouth between them').toBe(true);
  expect(sprite.frames).toEqual([4, 4, 2]);
  const [r, g, b] = sprite.body.match(/\d+/g)!.map(Number);
  expect(b > 200 && b > r + 120 && g > r, `electro blue: ${sprite.body}`).toBe(true);

  // It starts resting in the top-right corner.
  const start = (await octo())!;
  expect(start.corner).toBe('right');
  expect(start.right).toBeLessThan(2);
  // It crawls to the other corner, its arms changing frames, its eyes looking where it goes.
  await expect.poll(async () => (await octo())!.classes.includes('is-crawling'), { timeout: 10_000, message: 'it sets off' }).toBe(true);
  expect((await octo())!.classes).toContain('is-left');
  const seen = new Set<number>();
  await expect.poll(async () => {
    const now = (await octo())!;
    seen.add(now.crawl.indexOf(true));
    return [...seen].filter((i) => i >= 0).length;
  }, { message: 'its arms step through their crawling frames' }).toBeGreaterThan(2);
  const lefts: number[] = [];
  await expect.poll(async () => {
    lefts.push((await octo())!.left);
    return lefts.length > 1 && lefts[lefts.length - 1] < lefts[0];
  }, { message: 'it moves left' }).toBe(true);
  await browser.evaluate(() => { for (const a of document.querySelector('.watch .octo')!.getAnimations()) a.finish(); });
  await expect.poll(async () => (await octo())!.corner).toBe('left');
  const arrived = (await octo())!;
  expect(arrived.left).toBeLessThan(2);
  expect(arrived.classes).toContain('is-resting');

  // Stuck to the top: the transcript scrolls under its lane, which stays at the top of the view.
  await append(browser, Array.from({ length: 40 }, (_, i) => ({ role: 'assistant', text: `Line ${i} of a long answer.` })));
  await expect(screen.getByText('Line 39 of a long answer.')).toBeVisible();
  expect(await browser.evaluate(() => {
    const scroller = document.querySelector('.watch .watch-scroll')!;
    const lane = document.querySelector('.watch .watch-lane')!;
    return [scroller.scrollTop > 200, Math.round(lane.getBoundingClientRect().top - scroller.getBoundingClientRect().top)];
  })).toEqual([true, 0]);

  // Leaving the task takes it away.
  await app.open(mock('/s/studio/t/14'));
  await expect.poll(() => browser.evaluate(() => document.querySelectorAll('.octo').length)).toBe(0);
});

test('under reduced motion the octopus rests still in the top-right corner', async ({ app, browser }) => {
  await browser.addInitScript(() => {
    const media = window.matchMedia.bind(window);
    window.matchMedia = (query: string) => (query.includes('prefers-reduced-motion') ? ({ ...media(query), matches: true, media: query } as MediaQueryList) : media(query));
  });
  await watchStudio12(app, browser);
  const state = await browser.evaluate(() => {
    const el = document.querySelector('.watch .octo') as HTMLElement;
    const lane = el.parentElement!.getBoundingClientRect();
    return { still: el.classList.contains('is-still'), corner: el.dataset.corner, right: Math.round(lane.right - el.getBoundingClientRect().right), moving: el.getAnimations().length };
  });
  expect(state).toEqual({ still: true, corner: 'right', right: 0, moving: 0 });
  // The stylesheet stops its frames, its bob and its blink under reduced motion.
  const stopped = await browser.evaluate(() => {
    const found: string[] = [];
    const visit = (rules: CSSRuleList, reduced: boolean) => {
      for (const rule of rules) {
        if (rule instanceof CSSMediaRule) visit(rule.cssRules, reduced || rule.conditionText.includes('prefers-reduced-motion: reduce'));
        else if (reduced && rule instanceof CSSStyleRule && rule.style.animationName === 'none') found.push(rule.selectorText);
      }
    };
    for (const sheet of document.styleSheets) { try { visit(sheet.cssRules, false); } catch { /* another origin */ } }
    return found.join(' | ');
  });
  for (const part of ['.octo-frame', '.octo-head', '.octo-eyes']) expect(stopped).toContain(part);
});

test('a past session keeps its spider and has no octopus', async ({ app, browser }) => {
  await app.open(mock('/s/studio/sessions/ses_imp_checkout'));
  await expect(browser.locator('.ss-tui .ss-locked')).toBeVisible();
  expect(await browser.evaluate(() => [document.querySelectorAll('.ss-tui .ss-web').length, document.querySelectorAll('.octo').length])).toEqual([1, 0]);
});
