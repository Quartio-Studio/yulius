import { test, type Browser } from '@e2e-dev/web';
import { expect } from 'e2e';
import { failOnPageErrors, mock } from './support.ts';

failOnPageErrors();

// The task terminal and user terminals (F 2026-10-10): each viewer sees the terminal full size on
// its side — another viewer's grid is scaled to fill the box, with a small chip instead of a
// banner, and a click takes the size (UI-019, TSK-006); copy and paste work as in a desktop
// terminal; file paths in the output open the editor at the file and line (UI-018).

type Session = { stopTicking(): void; ensureTicking(): void };
type WithMock = Window & { __yuliusMock: { sessions: Map<string, Session> } };
type Monaco = { editor: { getEditors(): { getPosition(): { lineNumber: number; column: number } | null }[] } };

const rows = (browser: Browser) => browser.locator('.taskview .xterm-rows');
/** The terminal row that shows `text` (in the task view, or in `scope`). */
const row = (browser: Browser, text: string, scope = '.taskview') => browser.locator(`${scope} .xterm-rows > div`).filter({ hasText: text }).first();
/** Lets a pointer move settle (xterm asks its link providers when the pointer rests). */
const settle = (ms = 500) => new Promise((resolve) => { setTimeout(resolve, ms); });

/** Holds the mock agent's transcript still (it prints a line every few seconds). */
const holdStill = (browser: Browser) => browser.evaluate(() => {
  for (const [, session] of (window as unknown as WithMock).__yuliusMock.sessions) {
    session.stopTicking();
    session.ensureTicking = () => {};
  }
});

/** Where `text` is drawn on the page (the middle of its second character), or null. */
const spot = (browser: Browser, text: string, scope = '.taskview') => browser.evaluate(([where, needle]) => {
  const line = [...document.querySelectorAll(`${where} .xterm-rows > div`)].find((d) => (d.textContent || '').includes(needle));
  if (!line) return null;
  const index = (line.textContent || '').indexOf(needle) + 1;
  const walker = document.createTreeWalker(line, NodeFilter.SHOW_TEXT);
  let seen = 0;
  for (let node = walker.nextNode(); node; node = walker.nextNode()) {
    const length = (node.textContent || '').length;
    if (seen + length > index) {
      const range = document.createRange();
      range.setStart(node, index - seen);
      range.setEnd(node, index - seen + 1);
      const r = range.getBoundingClientRect();
      return { x: r.left + r.width / 2, y: r.top + r.height / 2 };
    }
    seen += length;
  }
  return null;
}, [scope, text] as const);

/** Whether the pointer rests on a link (xterm shows the hand cursor over links). */
const onLink = (browser: Browser) => browser.evaluate(() => Boolean(document.querySelector('.xterm-cursor-pointer')));

/** Rests the pointer on `text`; resolves to its page position and whether it is a link. */
async function hoverText(browser: Browser, text: string, scope = '.taskview'): Promise<{ x: number; y: number; link: boolean }> {
  const point = await spot(browser, text, scope);
  expect(point, `${text} is on the screen`).not.toBeNull();
  const { x, y } = point!;
  await browser.mouse.move(x - 4, y);
  await browser.mouse.move(x, y);
  // File links are known once the folder's listing arrived.
  await settle();
  return { x, y, link: await onLink(browser) };
}

/** Clicks the page at `point` with the primary button. */
async function clickAt(browser: Browser, point: { x: number; y: number }): Promise<void> {
  await browser.mouse.move(point.x, point.y);
  await browser.mouse.down();
  await browser.mouse.up();
}

/** Selects the text of the row showing `text` by dragging across it. */
async function selectRow(browser: Browser, text: string): Promise<void> {
  const box = await row(browser, text).boundingBox();
  expect(box, `${text} is on the screen`).not.toBeNull();
  const y = box!.y + box!.height / 2;
  await browser.mouse.move(box!.x + 4, y);
  await browser.mouse.down();
  await browser.mouse.move(box!.x + 200, y);
  await browser.mouse.move(box!.x + 400, y);
  await browser.mouse.up();
}

test('the clipboard keys follow desktop terminals', async ({ app, browser }) => {
  await app.open(mock('/s/studio'));
  const table = await browser.evaluate(async () => {
    const url = '/assets/termkeys.js';
    const { clipboardKeyAction } = await import(url);
    const key = (o: Record<string, unknown>) => ({ type: 'keydown', key: '', code: '', ctrlKey: false, metaKey: false, shiftKey: false, altKey: false, ...o });
    const cases: Record<string, [Record<string, unknown>, Record<string, boolean>]> = {
      'mac ⌘C': [{ key: 'c', metaKey: true }, { mac: true }],
      'mac ⌘V': [{ key: 'v', metaKey: true }, { mac: true }],
      'mac Ctrl+C, selection': [{ key: 'c', ctrlKey: true }, { mac: true, selection: true }],
      'mac Ctrl+C': [{ key: 'c', ctrlKey: true }, { mac: true }],
      'mac Ctrl+V': [{ key: 'v', ctrlKey: true }, { mac: true }],
      'Ctrl+C, selection': [{ key: 'c', ctrlKey: true }, { selection: true }],
      'Ctrl+C': [{ key: 'c', ctrlKey: true }, {}],
      'Ctrl+V': [{ key: 'v', ctrlKey: true }, {}],
      'Ctrl+Shift+C': [{ key: 'C', ctrlKey: true, shiftKey: true }, { selection: true }],
      'Ctrl+Shift+V': [{ key: 'V', ctrlKey: true, shiftKey: true }, {}],
      'Ctrl+Insert, selection': [{ key: 'Insert', ctrlKey: true }, { selection: true }],
      'Shift+Insert': [{ key: 'Insert', shiftKey: true }, {}],
      'Ctrl+C on a Cyrillic layout, selection': [{ key: 'с', code: 'KeyC', ctrlKey: true }, { selection: true }],
      'AltGr+C': [{ key: 'c', ctrlKey: true, altKey: true }, { selection: true }],
      'c': [{ key: 'c' }, { selection: true }],
    };
    return Object.fromEntries(Object.entries(cases).map(([name, [event, options]]) => [name, clipboardKeyAction(key(event), options)]));
  });
  expect(table).toEqual({
    'mac ⌘C': { copy: true, clear: false },
    'mac ⌘V': { paste: 'native' },
    'mac Ctrl+C, selection': { copy: true, clear: true },
    'mac Ctrl+C': null,
    'mac Ctrl+V': { paste: 'read' },
    'Ctrl+C, selection': { copy: true, clear: true },
    'Ctrl+C': null,
    'Ctrl+V': { paste: 'native' },
    'Ctrl+Shift+C': { copy: true, clear: false },
    'Ctrl+Shift+V': { paste: 'native' },
    'Ctrl+Insert, selection': { copy: true, clear: false },
    'Shift+Insert': { paste: 'native' },
    'Ctrl+C on a Cyrillic layout, selection': { copy: true, clear: true },
    'AltGr+C': null,
    'c': null,
  });
});

test('file references are found in every form and resolved inside the folder only', async ({ app, browser }) => {
  await app.open(mock('/s/studio'));
  const result = await browser.evaluate(async () => {
    const url = '/assets/termlinks.js';
    const { findPathCandidates, resolvePath, parseFileUrl, relativeCwd } = await import(url);
    const roots = ['/srv/w/12'];
    const line = '⏺ Spec: docs/specs/2026-10-10-x.md · `src/app.js:12:5`, (AGENTS.md), Read(src/a.ts) /srv/w/12/README.md#L3, '
      + 'missing.md, ../up.md, .git/config, /etc/hosts, ~/notes.md, https://example.com/a/b.md, e.g. done.';
    const found = findPathCandidates(line).map((c: { text: string; raw: string; line: number | null; col: number | null }) => [c.text, resolvePath(c.raw, { roots }), c.line, c.col]);
    return {
      found,
      inSubdir: resolvePath('app.js', { cwd: 'src', roots }),
      outsideCwd: resolvePath('app.js', { cwd: null, roots }),
      climbs: resolvePath('src/../../x.md', { roots }),
      url: parseFileUrl('file:///srv/w/12/src/a%20b.js#L4'),
      urlColon: parseFileUrl('file://host/srv/w/12/src/a.js:7:2'),
      web: parseFileUrl('https://example.com/a.js'),
      cwd: [relativeCwd('/srv/w/12/src', roots), relativeCwd('/srv/w/12', roots), relativeCwd('/tmp', roots)],
    };
  });
  expect(result.found).toEqual([
    ['docs/specs/2026-10-10-x.md', 'docs/specs/2026-10-10-x.md', null, null],
    ['src/app.js:12:5', 'src/app.js', 12, 5],
    ['AGENTS.md', 'AGENTS.md', null, null],
    ['src/a.ts', 'src/a.ts', null, null],
    ['/srv/w/12/README.md#L3', 'README.md', 3, null],
    ['missing.md', 'missing.md', null, null], // a candidate: only existing files become links
    ['../up.md', null, null, null],
    ['.git/config', null, null, null],
    ['/etc/hosts', null, null, null],
    ['~/notes.md', null, null, null],
    ['e.g', 'e.g', null, null],
  ]);
  expect(result.inSubdir).toBe('src/app.js');
  expect(result.outsideCwd).toBeNull();
  expect(result.climbs).toBeNull();
  expect(result.url).toEqual({ path: '/srv/w/12/src/a b.js', line: 4, col: null });
  expect(result.urlColon).toEqual({ path: '/srv/w/12/src/a.js', line: 7, col: 2 });
  expect(result.web).toBeNull();
  expect(result.cwd).toEqual(['src', '', null]);
});

test('Ctrl+C copies a selection and interrupts without one; paste types the clipboard', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/t/12'));
  await expect(rows(browser)).toHaveText(/Read 142 lines/);
  await holdStill(browser);
  // What a copy puts on the clipboard, read from the copy event (no clipboard permission needed).
  await browser.evaluate(() => {
    const copied: string[] = [];
    (window as unknown as { __copied: string[] }).__copied = copied;
    document.addEventListener('copy', (event) => copied.push(event.clipboardData ? event.clipboardData.getData('text/plain') : ''));
  });
  const copied = () => browser.evaluate(() => (window as unknown as { __copied: string[] }).__copied);

  // With a selection, Ctrl+C (⌘C on macOS) copies it instead of interrupting the program.
  await selectRow(browser, 'Read 142 lines');
  await browser.keyboard.press('ControlOrMeta+c');
  await expect.poll(copied).toEqual([expect.stringContaining('Read 142 lines')]);
  await expect(rows(browser)).not.toHaveText(/Interrupted by user/);
  // Without a selection Ctrl+C is ^C for the program (a click clears the selection, which ⌘C keeps).
  await rows(browser).click({ position: { x: 300, y: 8 } });
  await browser.keyboard.press('Control+c');
  await expect(rows(browser)).toHaveText(/Interrupted by user/);

  // A paste goes to the program as typed input.
  await browser.evaluate(() => {
    const data = new DataTransfer();
    data.setData('text/plain', 'pasted words');
    document.querySelector('.taskview .xterm-helper-textarea')!.dispatchEvent(new ClipboardEvent('paste', { clipboardData: data, bubbles: true, cancelable: true }));
  });
  await expect(row(browser, '❯')).toHaveText(/pasted words/);

  // The right-click menu has Copy, Paste and Select all (the event a right-click sends; xterm's
  // rows sit under its viewport, so the pointer cannot be aimed at them by role).
  await browser.evaluate(() => {
    const line = [...document.querySelectorAll('.taskview .xterm-rows > div')].find((d) => (d.textContent || '').includes('Read 142 lines'))!;
    const box = line.getBoundingClientRect();
    line.dispatchEvent(new MouseEvent('contextmenu', { bubbles: true, cancelable: true, button: 2, clientX: box.left + 60, clientY: box.top + box.height / 2 }));
  });
  await expect(screen.getByRole('menuitem', /^Copy/)).toBeVisible();
  await expect(screen.getByRole('menuitem', /^Paste/)).toBeVisible();
  await expect(screen.getByRole('menuitem', /^Select all/)).toBeVisible();
  await browser.keyboard.press('Escape');
  await expect(screen.getByRole('menuitem', /^Paste/)).toBeHidden();
});

test('a read-only viewer cannot paste', async ({ app, browser, screen }) => {
  await app.open(mock('/s/sandbox-3/t/1'));
  await expect(screen.getByText('Read-only (GitHub: read)')).toBeVisible();
  await browser.evaluate(() => {
    const data = new DataTransfer();
    data.setData('text/plain', 'should not arrive');
    document.querySelector('.taskview .xterm-helper-textarea')!.dispatchEvent(new ClipboardEvent('paste', { clipboardData: data, bubbles: true, cancelable: true }));
  });
  await expect(browser.locator('.taskview .term-notice')).toHaveText('Read-only (GitHub: read)');
  await expect(rows(browser)).not.toHaveText(/should not arrive/);
});

test('another viewer’s size fills this box, with a chip instead of a banner, until a click takes it', async ({ app, browser, screen }) => {
  // Another, active viewer set the terminal to 90×30 (the mock's `othercols`/`otherrows`).
  await app.open(mock('/s/studio/t/12', '&othercols=90&otherrows=30'));
  await expect(rows(browser)).toHaveText(/Read 142 lines/);
  await holdStill(browser);
  const chip = screen.getByText('Sized by another viewer');
  await expect(chip).toBeVisible();
  await expect(screen.getByRole('button', 'Take control')).toBeHidden();
  await expect(browser.locator('.taskview .term-banner')).toBeHidden();
  const layout = () => browser.evaluate(() => {
    const box = document.querySelector('.taskview .term-fit')!.getBoundingClientRect();
    const grid = document.querySelector('.taskview .xterm-screen')!.getBoundingClientRect();
    return {
      rows: document.querySelectorAll('.taskview .xterm-rows > div').length,
      fill: Math.max(grid.width / box.width, grid.height / box.height),
      overflow: Math.max(grid.right - box.right, grid.bottom - box.bottom),
      centred: Math.abs((grid.left - box.left) - (box.right - grid.right)) <= 2,
    };
  });
  // The whole 90×30 grid, as large as the box allows in one dimension, never cut off, centred.
  await expect.poll(async () => (await layout()).rows).toBe(30);
  const shown = await layout();
  expect(shown.fill).toBeGreaterThan(0.85);
  expect(shown.overflow).toBeLessThanOrEqual(0);
  expect(shown.centred).toBe(true);

  // A click in the terminal takes the size: this window's own grid at its own font.
  await rows(browser).click({ position: { x: 200, y: 30 } });
  await expect(chip).toBeHidden();
  await expect.poll(async () => (await layout()).rows).toBeGreaterThan(30);
});

test('file paths in the task terminal open the editor at the file and line', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/t/12'));
  await expect(rows(browser)).toHaveText(/Not links:/);
  await holdStill(browser);

  // Links: a bare path, `path:line:col`, a name in parentheses, an absolute path in the worktree,
  // an OSC 8 file:// hyperlink.
  for (const text of ['docs/specs/2026-10-10-improve-the-code.md', 'src/api/checkout.rs:2:5', 'AGENTS.md', 'README.md#L3', 'the checkout source']) {
    expect((await hoverText(browser, text)).link, `${text} is a link`).toBe(true);
  }
  // Plain text: a missing file, a path above the worktree, .git, an OSC 8 link outside it.
  for (const text of ['docs/missing.md', '../outside.md', '.git/config', 'hosts']) {
    expect((await hoverText(browser, text)).link, `${text} is plain text`).toBe(false);
  }
  // Web links stay web links (UI-018).
  expect((await hoverText(browser, 'https://example.com/docs/guide.md')).link).toBe(true);

  // A click opens the Editor at the file, line and column.
  await clickAt(browser, await hoverText(browser, 'src/api/checkout.rs:2:5'));
  await expect(screen.getByRole('tab', /checkout\.rs/)).toHaveAttribute('aria-selected', 'true');
  await expect
    .poll(() => browser.evaluate(() => (window as unknown as { monaco?: Monaco }).monaco?.editor.getEditors()[0]?.getPosition() ?? null))
    .toEqual({ lineNumber: 2, column: 5 });
});

test('file names a shell prints open the editor', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/t/12'));
  await screen.getByRole('button', /^Show tools/).click();
  await screen.getByRole('button', 'Shell').click();
  const shell = '.term-views .term-slot:not([hidden])';
  await expect(browser.locator(`${shell} .xterm-rows`)).toHaveText(/Contained sandbox shell/);
  await expect(browser.locator(`${shell} .xterm-helper-textarea`)).toBeFocused();
  for (const key of ['l', 's', 'Enter']) await browser.keyboard.press(key);
  await expect(browser.locator(`${shell} .xterm-rows`)).toHaveText(/AGENTS\.md/);
  expect((await hoverText(browser, 'Cargo.toml', shell)).link, 'not in the repository').toBe(false);
  const agents = await hoverText(browser, 'AGENTS.md', shell);
  expect(agents.link).toBe(true);
  await clickAt(browser, agents);
  await expect(screen.getByRole('tab', /AGENTS\.md/)).toHaveAttribute('aria-selected', 'true');
});
