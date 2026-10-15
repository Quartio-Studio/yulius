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

/** Puts the pointer on `text`. */
async function pointAt(browser: Browser, text: string, scope: string): Promise<void> {
  const point = await spot(browser, text, scope);
  expect(point, `${text} is on the screen`).not.toBeNull();
  const { x, y } = point!;
  await browser.mouse.move(x - 4, y);
  await browser.mouse.move(x, y);
}

/**
 * Rests the pointer on `text` and waits until xterm shows it as a link: a file link is known once
 * its folder's listing arrived, which a busy runner delays (#320). A press there (without moving)
 * then activates the link.
 */
async function hoverLink(browser: Browser, text: string, scope = '.taskview'): Promise<void> {
  await pointAt(browser, text, scope);
  await expect.poll(() => onLink(browser), { message: `${text} is a link` }).toBe(true);
}

/**
 * Rests the pointer on `text` and says whether it is a link after a rest. Only for text expected to
 * be plain: no signal says "the listing answered: not a link", and a late answer can only make
 * this pass, never fail (expected links use hoverLink).
 */
async function hoverPlain(browser: Browser, text: string, scope = '.taskview'): Promise<boolean> {
  await pointAt(browser, text, scope);
  await settle();
  return onLink(browser);
}

/**
 * Waits until the Editor a link opened shows an editor. Its first opening loads Monaco (many
 * files), which takes however long a starved runner needs (#320): a condition, as in
 * filedrop.e2e.ts.
 */
const editorLoaded = (browser: Browser) => expect
  .poll(() => browser.evaluate(() => ((window as unknown as { monaco?: Monaco }).monaco?.editor.getEditors().length ?? 0) > 0), { timeout: 60000 })
  .toBe(true);

/** Presses and releases the primary button where the pointer rests (a click without moving). */
async function press(browser: Browser): Promise<void> {
  await browser.mouse.down();
  await browser.mouse.up();
}

/** Clicks the page at `point` with the primary button. */
async function clickAt(browser: Browser, point: { x: number; y: number }): Promise<void> {
  await browser.mouse.move(point.x, point.y);
  await browser.mouse.down();
  await browser.mouse.up();
}

/**
 * Records the input frames that reach the program's side from now on (the mock refuses a
 * read-only viewer's input, as the server does, so the screen alone would not show input that was
 * sent); the returned function reads them.
 */
async function recordInput(browser: Browser): Promise<() => Promise<string[]>> {
  await browser.evaluate(() => {
    const sent: string[] = [];
    (window as unknown as { __sent: string[] }).__sent = sent;
    for (const [, session] of (window as unknown as { __yuliusMock: { sessions: Map<string, { receive(ws: unknown, data: string): void }> } }).__yuliusMock.sessions) {
      const receive = session.receive.bind(session);
      session.receive = (ws, data) => { const frame = JSON.parse(data); if (frame.type === 'input') sent.push(frame.data); receive(ws, data); };
    }
  });
  return () => browser.evaluate(() => (window as unknown as { __sent: string[] }).__sent);
}

/** Whether the task terminal shows a selection (xterm draws it as boxes over the rows). */
const selected = (browser: Browser) => browser.evaluate(() => document.querySelectorAll('.taskview .xterm-selection > div').length > 0);

/** The key that takes the mouse back from a program that asked for it: Option on macOS, else Shift (xterm). */
const pageKey = (browser: Browser) => browser.evaluate(() => (/Mac|iPhone|iPad/.test(navigator.platform || navigator.userAgent || '') ? 'altKey' : 'shiftKey'));

/**
 * Selects the row showing `text` by dragging across it with the key that takes the mouse back from
 * the program (SPEC-098 MSE-003). The test mouse cannot hold a key, so the drag is dispatched in
 * the page, as xterm listens for it: a press on the rows, moves and the release on the document.
 */
async function keySelectRow(browser: Browser, text: string): Promise<void> {
  const box = await row(browser, text).boundingBox();
  expect(box, `${text} is on the screen`).not.toBeNull();
  const key = await pageKey(browser);
  await browser.evaluate(({ x, y, key }) => {
    const at = (dx: number, buttons: number) => ({ bubbles: true, cancelable: true, composed: true, clientX: x + dx, clientY: y, button: 0, buttons, detail: 1, [key]: true });
    document.elementFromPoint(x + 4, y)!.dispatchEvent(new MouseEvent('mousedown', at(4, 1)));
    document.dispatchEvent(new MouseEvent('mousemove', at(200, 1)));
    document.dispatchEvent(new MouseEvent('mousemove', at(400, 1)));
    document.dispatchEvent(new MouseEvent('mouseup', at(400, 0)));
  }, { x: box!.x, y: box!.y + box!.height / 2, key });
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
  // The program's copy (OSC 52, SPEC-158): a write's text, never a query or a broken payload.
  const osc = await browser.evaluate(async () => {
    const url = '/assets/termkeys.js';
    const { osc52Text } = await import(url);
    const utf8 = (text: string) => btoa(String.fromCharCode(...new TextEncoder().encode(text)));
    return ['c;' + utf8('git push → ✓'), ';' + utf8('no letters'), 'pc;' + utf8('two'), 'c;?', 'c;', 'c;%%%', utf8('no separator'), 'c;' + utf8('')].map((data) => osc52Text(data));
  });
  expect(osc).toEqual(['git push → ✓', 'no letters', 'two', null, null, null, null, null]);
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

  // With a selection, Ctrl+C (⌘C on macOS) copies it instead of interrupting the program. The
  // program has the mouse, so the selection is made with Shift (Option on macOS) held.
  await keySelectRow(browser, 'Read 142 lines');
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
  // rows sit under its viewport, so the pointer cannot be aimed at them by role). The program has
  // the mouse: a plain right-click is the program's, Shift (Option on macOS) brings the menu.
  const rightClick = (key: string | null) => browser.evaluate((key) => {
    const line = [...document.querySelectorAll('.taskview .xterm-rows > div')].find((d) => (d.textContent || '').includes('Read 142 lines'))!;
    const box = line.getBoundingClientRect();
    const event = new MouseEvent('contextmenu', { bubbles: true, cancelable: true, button: 2, clientX: box.left + 60, clientY: box.top + box.height / 2, ...(key ? { [key]: true } : {}) });
    line.dispatchEvent(event);
    return event.defaultPrevented;
  }, key);
  // No menu at all, not even the browser's own over the program's right-click.
  expect(await rightClick(null), 'the browser menu is held back').toBe(true);
  await settle();
  await expect(screen.getByRole('menuitem', /^Paste/)).toBeHidden();
  await rightClick(await pageKey(browser));
  await expect(screen.getByRole('menuitem', /^Copy/)).toBeVisible();
  await expect(screen.getByRole('menuitem', /^Paste/)).toBeVisible();
  await expect(screen.getByRole('menuitem', /^Select all/)).toBeVisible();
  await browser.keyboard.press('Escape');
  await expect(screen.getByRole('menuitem', /^Paste/)).toBeHidden();
});

test('a selection the program copies (OSC 52) reaches the clipboard, or the copy key when the browser refuses', async ({ app, browser }) => {
  // Task #149, SPEC-158: Claude Code selects a plain drag itself and copies it with OSC 52, as the
  // mock does; the browser clipboard must get it.
  await app.open(mock('/s/studio/t/12'));
  await expect(rows(browser)).toHaveText(/Read 142 lines/);
  await holdStill(browser);
  type Clip = { __written: string[]; __refuse: boolean; __copied: string[] };
  // The clipboard API, recorded; `__refuse` rejects writes as Safari does outside a key press.
  await browser.evaluate(() => {
    const w = window as unknown as Clip;
    w.__written = [];
    w.__refuse = false;
    w.__copied = [];
    navigator.clipboard.writeText = (text: string) => {
      if (w.__refuse) return Promise.reject(new DOMException('Write permission denied.', 'NotAllowedError'));
      w.__written.push(text);
      return Promise.resolve();
    };
    document.addEventListener('copy', (event) => w.__copied.push(event.clipboardData ? event.clipboardData.getData('text/plain') : ''));
  });
  const written = () => browser.evaluate(() => (window as unknown as Clip).__written);
  const copied = () => browser.evaluate(() => (window as unknown as Clip).__copied);
  const notice = browser.locator('.taskview .term-notice');
  /** The program prints `sequence` to every viewer, unasked. */
  const print = (sequence: string) => browser.evaluate((text) => {
    for (const [, session] of (window as unknown as { __yuliusMock: { sessions: Map<string, { broadcast(text: string): void }> } }).__yuliusMock.sessions) session.broadcast(text);
  }, sequence);

  // A copy the program prints while this viewer has not used the terminal writes nothing.
  await print(`\x1b]52;c;${btoa('unasked')}\x07`);
  await settle();
  expect(await written(), 'no write without this viewer\'s input').toEqual([]);

  // A plain drag is the program's: it selects and copies the text, which reaches the clipboard.
  await selectRow(browser, 'Read 142 lines');
  await expect.poll(written).toEqual([expect.stringContaining('Read 142 lines')]);
  await expect(notice).toHaveText('Copied');
  expect(await selected(browser), 'xterm itself selects nothing').toBe(false);

  // A query is never answered, and nothing is written for it.
  const sent = await recordInput(browser);
  await print('\x1b]52;c;?\x07');
  await settle();
  expect(await sent(), 'no answer to a query').toEqual([]);
  expect((await written()).length).toBe(1);

  // A browser that refuses the write: the copy key copies the program's text.
  await browser.evaluate(() => { (window as unknown as Clip).__refuse = true; });
  await selectRow(browser, 'Read 142 lines');
  const mac = await browser.evaluate(() => /Mac|iPhone|iPad/.test(navigator.platform || navigator.userAgent || ''));
  await expect(notice).toHaveText(mac ? 'Press ⌘C to copy' : 'Press Ctrl+Shift+C to copy');
  await browser.keyboard.press(mac ? 'Meta+c' : 'Control+Shift+c');
  await expect.poll(copied).toEqual([expect.stringContaining('Read 142 lines')]);
  await expect(notice).toHaveText('Copied');
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

test('only keystrokes count as typing, as on the server (SPEC-068)', async ({ app, browser }) => {
  await app.open(mock('/s/studio'));
  const table = await browser.evaluate(async () => {
    const url = '/assets/typing.js';
    const { isTyping } = await import(url);
    const cases: Record<string, string> = {
      'focus in': '\x1b[I',
      'focus out': '\x1b[O',
      'SGR press': '\x1b[<0;12;5M',
      'SGR release': '\x1b[<0;12;5m',
      'X10 press': '\x1b[M #%',
      'X10 far right': '\x1b[M \u00e0\u0100',
      'device attributes': '\x1b[?1;2c',
      'cursor position': '\x1b[24;80R',
      'mode report': '\x1b[?2004;1$y',
      'OSC answer': '\x1b]11;rgb:0000/0000/0000\x1b\\',
      'DCS answer': '\x1bP>|xterm.js(5.5.0)\x1b\\',
      'text': 'a',
      'Enter': '\r',
      'Backspace': '\x7f',
      'arrow': '\x1b[A',
      'Escape': '\x1b',
      'paste': '\x1b[200~draft\x1b[201~',
      'focus, then a key': '\x1b[Ix',
    };
    return Object.fromEntries(Object.entries(cases).map(([name, data]) => [name, isTyping(data)]));
  });
  expect(table).toEqual({
    'focus in': false,
    'focus out': false,
    'SGR press': false,
    'SGR release': false,
    'X10 press': false,
    'X10 far right': false,
    'device attributes': false,
    'cursor position': false,
    'mode report': false,
    'OSC answer': false,
    'DCS answer': false,
    'text': true,
    'Enter': true,
    'Backspace': true,
    'arrow': true,
    'Escape': true,
    'paste': true,
    'focus, then a key': true,
  });
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

test('the program has the mouse, as in a native terminal; Shift (Option on macOS) selects', async ({ app, browser }) => {
  // Task #25, SPEC-098: the mock draws full screen with mouse reporting, like Claude Code, and
  // toggles a collapsed tool group on a primary press at its row. No chip offers to switch.
  await app.open(mock('/s/studio/t/12'));
  await expect(rows(browser)).toHaveText(/Searched for 2 patterns, read 1 file/);
  await holdStill(browser);
  const expanded = /loyalty_points/;
  const group = async () => {
    const point = await spot(browser, 'Searched for 2 patterns');
    expect(point, 'the group is on the screen').not.toBeNull();
    return point!;
  };
  expect(await browser.evaluate(() => document.querySelectorAll('.term-mouse-toggle').length), 'no mouse chip').toBe(0);

  // A click expands the group at once, a second click collapses it.
  await clickAt(browser, await group());
  await expect(rows(browser)).toHaveText(expanded);
  await expect(row(browser, 'Searched for 2 patterns')).not.toHaveText(/click to expand/);
  await clickAt(browser, await group());
  await expect(rows(browser)).not.toHaveText(expanded);

  // A plain drag is the program's too: its press reaches it and nothing is selected.
  await selectRow(browser, 'Searched for 2 patterns');
  await expect(rows(browser)).toHaveText(expanded);
  expect(await selected(browser), 'a plain drag selects nothing').toBe(false);
  await clickAt(browser, await group());
  await expect(rows(browser)).not.toHaveText(expanded);

  // With Shift (Option on macOS) held, a drag selects the text and does not reach the program.
  await keySelectRow(browser, 'Searched for 2 patterns');
  await expect.poll(() => selected(browser), { message: 'a Shift-drag selects' }).toBe(true);
  await settle();
  await expect(rows(browser)).not.toHaveText(expanded);
});

test('the wheel scrolls the program; pinch-zoom and horizontal swipes stay with the browser', async ({ app, browser }) => {
  await app.open(mock('/s/studio/t/12'));
  await expect(rows(browser)).toHaveText(/Searched for 2 patterns/);
  await holdStill(browser);
  const sent = await recordInput(browser);
  const point = await spot(browser, 'Searched for 2 patterns');
  expect(point, 'the group is on the screen').not.toBeNull();
  // A wheel turn reaches the program as a wheel report in its encoding (SGR 64: up).
  await browser.mouse.move(point!.x, point!.y);
  await browser.mouse.wheel(0, -200);
  await expect.poll(async () => (await sent()).some((data) => data.startsWith('\x1b[<64;')), { message: 'a wheel report' }).toBe(true);
  // Ctrl+wheel (a trackpad pinch) and a horizontal swipe are the browser's: xterm neither reports
  // nor cancels them (SPEC-098 MSE-006, #78).
  const before = (await sent()).length;
  const kept = await browser.evaluate(({ x, y }) => {
    const target = document.elementFromPoint(x, y)!;
    return [{ deltaY: -120, ctrlKey: true }, { deltaX: 120 }].map((delta) => {
      const event = new WheelEvent('wheel', { bubbles: true, cancelable: true, composed: true, clientX: x, clientY: y, deltaMode: 0, ...delta });
      target.dispatchEvent(event);
      return !event.defaultPrevented;
    });
  }, point!);
  expect(kept, 'pinch and swipe left to the browser').toEqual([true, true]);
  await settle();
  expect((await sent()).length, 'no report for a pinch or a swipe').toBe(before);
});

test('mouse use while disconnected says nothing; typed input still warns', async ({ app, browser }) => {
  await app.open(mock('/s/studio/t/12'));
  await expect(rows(browser)).toHaveText(/Searched for 2 patterns/);
  await holdStill(browser);
  const point = await spot(browser, 'Searched for 2 patterns');
  expect(point, 'the group is on the screen').not.toBeNull();
  await browser.evaluate(() => (window as unknown as { __yuliusMock: { goOffline(): void } }).__yuliusMock.goOffline());
  await expect(browser.locator('.taskview .term-conn')).toBeVisible();
  // A click is not input the person sent: no "Not connected" notice for it (SPEC-098).
  await clickAt(browser, point!);
  await settle();
  await expect(browser.locator('.taskview .term-notice')).toBeHidden();
  await browser.keyboard.press('x');
  await expect(browser.locator('.taskview .term-notice')).toHaveText('Not connected — input was not sent');
});

test('a read-only viewer’s mouse selects text and does not reach the program', async ({ app, browser, screen }) => {
  await app.open(mock('/s/sandbox-3/t/1'));
  await expect(screen.getByText('Read-only (GitHub: read)')).toBeVisible();
  await expect(rows(browser)).toHaveText(/Searched for 2 patterns/);
  const sent = await recordInput(browser);
  const point = await spot(browser, 'Searched for 2 patterns');
  expect(point, 'the group is on the screen').not.toBeNull();
  await clickAt(browser, point!);
  await settle();
  await expect(rows(browser)).not.toHaveText(/loyalty_points/);
  // A read-only viewer cannot send anything, so its plain drag selects text (SPEC-098 MSE-004).
  await selectRow(browser, 'Searched for 2 patterns');
  await expect.poll(() => selected(browser), { message: 'a plain drag selects' }).toBe(true);
  expect(await sent()).toEqual([]);
});

test('a click beside another viewer’s grid takes the size but does not reach the program', async ({ app, browser, screen }) => {
  // Another viewer's 70×30 grid is centred in this wider box, with blank margins at its sides.
  // Ten columns or more each side whatever the fonts: at 90 columns the margin depended on the
  // font's width and was 7 px on macOS (SPEC-082).
  await app.open(mock('/s/studio/t/12', '&othercols=70&otherrows=30'));
  await expect(rows(browser)).toHaveText(/Searched for 2 patterns/);
  await holdStill(browser);
  const point = await spot(browser, 'Searched for 2 patterns');
  expect(point, 'the group is on the screen').not.toBeNull();
  const margin = await browser.evaluate(() => {
    const box = document.querySelector('.taskview .term-fit')!.getBoundingClientRect();
    const grid = document.querySelector('.taskview .xterm-screen')!.getBoundingClientRect();
    return { left: box.left, width: grid.left - box.left };
  });
  expect(margin.width, 'the grid has a margin at its left').toBeGreaterThan(8);
  const sent = await recordInput(browser);
  await clickAt(browser, { x: margin.left + margin.width / 2, y: point!.y });
  await expect(screen.getByText('Sized by another viewer')).toBeHidden();
  await settle();
  await expect(rows(browser)).not.toHaveText(/loyalty_points/);
  // xterm would report a press beside the grid at its first column (SPEC-098 MSE-005).
  expect(await sent(), 'no mouse report').toEqual([]);
});

test('file paths in the task terminal open the editor at the file and line', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/t/12'));
  await expect(rows(browser)).toHaveText(/Not links:/);
  await holdStill(browser);

  // Links: a bare path, `path:line:col`, a name in parentheses, an absolute path in the worktree,
  // an OSC 8 file:// hyperlink.
  for (const text of ['docs/specs/2026-10-10-improve-the-code.md', 'src/api/checkout.rs:2:5', 'AGENTS.md', 'README.md#L3', 'the checkout source']) {
    await hoverLink(browser, text);
  }
  // Plain text: a missing file, a path above the worktree, .git, an OSC 8 link outside it.
  for (const text of ['docs/missing.md', '../outside.md', '.git/config', 'hosts']) {
    expect(await hoverPlain(browser, text), `${text} is plain text`).toBe(false);
  }
  // Web links stay web links (UI-018).
  await hoverLink(browser, 'https://example.com/docs/guide.md');

  // A click opens the Editor at the file, line and column (once xterm knows the link under the
  // pointer: before, the click lands on plain text).
  await hoverLink(browser, 'src/api/checkout.rs:2:5');
  await press(browser);
  await editorLoaded(browser);
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
  expect(await hoverPlain(browser, 'Cargo.toml', shell), 'not in the repository').toBe(false);
  await hoverLink(browser, 'AGENTS.md', shell);
  await press(browser);
  await editorLoaded(browser);
  await expect(screen.getByRole('tab', /AGENTS\.md/)).toHaveAttribute('aria-selected', 'true');
});
