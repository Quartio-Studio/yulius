import { test, type Browser } from '@e2e-dev/web';
import { expect } from 'e2e';
import { editorShown, failOnPageErrors, mock, terminalShown, touchTask } from './support.ts';

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
/** How long a wheel turn up waits for the program's answer before it counts as the top (terminal-core.js). */
const TOP_SETTLE_MS = 400;
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

// Claude Code 2.1.292 at 60 columns (`claude --resume` of a synthetic session, rendered by a
// terminal emulator; SPEC-339): it cuts a URL longer than a row at the right edge and continues it
// on the next row at the text's indent, with a cursor move, never a soft wrap.
const CLAUDE_ROWS_60 = [
  '❯ where is login and pass for https://quartio-v2-dev.vm.qua ',
  '  rtio.ai/admin/connectors?tab=all&page=2                   ',
  '',
  '● Open https://quartio-v2-dev.vm.quartio.ai/admin/connectors',
  '  ?tab=all&page=2 and sign in.',
  '',
  '  - Docs at https://quartio-v2-dev.vm.quartio.ai/admin/conne',
  '    ctors?tab=all&page=2/help',
  '  - Short https://example.com/a',
  '',
  '  https://quartio-v2-dev.vm.quartio.ai/admin/connectors?tab=',
  '  all&page=2',
  '',
  '● 1. Docs at https://quartio-v2-dev.vm.quartio.ai/admin/conn',
  '     ectors?tab=all&page=2/help',
  '  2. Two',
  '',
  '  See https://example.com/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa',
  '  aaaaaaa then sign in.',
  '',
  '  ▎ quote https://quartio-v2-dev.vm.quartio.ai/admin/connect',
  '  ▎ ors?tab=all&page=2https://quartio-v2-dev.vm.quartio.ai/a',
  '  ▎ dmin/connectors?tab=all&page=2',
  '',
  // Every row of the prompt keeps one blank column at the right edge, not only its first.
  '❯ The connectors page in the admin panel shows an error',
  '  when I open it. Where is login and pass for https://quart',
  '  io-v2-dev.vm.quartio.ai/admin/connectors?tab=all&page=2',
  '',
  '  check this too',
  '  https://quartio-v2-dev.vm.quartio.ai/admin/connectors?tab',
  '  =all&page=2',
];

test('a URL Claude Code continues on the next rows is found whole, and nothing else is joined (#940)', async ({ app, browser }) => {
  await app.open(mock('/s/studio'));
  const url38 = `https://example.com/${'a'.repeat(18)}`; // 38 characters: with "● " a full 40-column row
  const result = await browser.evaluate(async ([claude, long]) => {
    const url = '/assets/termlinks.js';
    const { findContinuedUrls } = await import(url);
    type Found = { url: string; start: { row: number; col: number }; end: { row: number; col: number } };
    const spans = (rows: (string | null)[], cols: number) => findContinuedUrls(rows, cols).map((l: Found) => [l.url, l.start.row, l.start.col, l.end.row, l.end.col]);
    return {
      claude: spans(claude, 60),
      // Three rows: a row that is all URL continues too.
      chain: spans([`● ${long}`, `  ${'b'.repeat(38)}`, '  end/x and more'], 40),
      // The row ends short of the right edge.
      short: spans(['● see https://example.com/abc', '  def'], 40),
      // The next row is indented elsewhere.
      indent: spans([`● ${long}`, '    more/path'], 40),
      // The word across the break fits a row: Claude Code would have moved it whole.
      fits: spans([`● ${'x'.repeat(19)} https://ex.co/abc`, '  sign in.'], 40),
      // The next row starts another URL.
      another: spans([`● ${long}`, '  https://other.example/x'], 40),
      // The next row is xterm's own soft wrap (the web-links addon's).
      soft: spans([`● ${long}`, null], 40),
      // A row cut inside the scheme.
      scheme: spans([`● ${'x'.repeat(34)} htt`, `  ps://example.com/${'p'.repeat(30)}`], 40),
      // A URL that ends exactly at the edge of a 58-column text row, followed by a word that makes
      // 58: Claude Code would have moved a 58-character word whole.
      edge: spans([`● See https://example.com/${'a'.repeat(34)}`, '  then sign in.'], 60),
      // Only the prompt keeps a blank column at the right edge: an answer row ending there was not cut.
      gap: spans([`● See https://example.com/${'a'.repeat(33)}`, '  configure it.'], 60),
      // Nor a later row of an answer.
      laterGap: spans(['● Intro.', `  See https://example.com/${'a'.repeat(33)}`, '  configure it.'], 60),
      // A URL longer than the rows looked at is no link rather than a wrong one; one row less is.
      tooLong: spans([`● https://example.com/${'c'.repeat(18)}`, ...Array(31).fill(`  ${'c'.repeat(38)}`), '  end'], 40),
      longest: spans([`● https://example.com/${'c'.repeat(18)}`, ...Array(31).fill(`  ${'c'.repeat(38)}`)], 40),
    };
  }, [CLAUDE_ROWS_60, url38] as const);
  const whole = 'https://quartio-v2-dev.vm.quartio.ai/admin/connectors?tab=all&page=2';
  const at = (row: number, text: string) => CLAUDE_ROWS_60[row].indexOf(text);
  expect(result.claude).toEqual([
    [whole, 0, at(0, 'https'), 1, at(1, '2 ')],
    [whole, 3, at(3, 'https'), 4, at(4, '2 and')],
    [`${whole}/help`, 6, at(6, 'https'), 7, CLAUDE_ROWS_60[7].length - 1],
    [whole, 10, at(10, 'https'), 11, CLAUDE_ROWS_60[11].length - 1],
    [`${whole}/help`, 13, at(13, 'https'), 14, CLAUDE_ROWS_60[14].length - 1],
    [`https://example.com/${'a'.repeat(41)}`, 17, at(17, 'https'), 18, at(18, ' then') - 1],
    [`${whole}${whole}`, 20, at(20, 'https'), 22, CLAUDE_ROWS_60[22].length - 1],
    [whole, 25, at(25, 'https'), 26, CLAUDE_ROWS_60[26].length - 1],
    [whole, 29, at(29, 'https'), 30, CLAUDE_ROWS_60[30].length - 1],
  ]);
  expect(result.scheme).toEqual([[`https://example.com/${'p'.repeat(30)}`, 0, 37, 1, 48]]);
  expect(result.edge).toEqual([]);
  expect(result.gap).toEqual([]);
  expect(result.laterGap).toEqual([]);
  expect(result.tooLong).toEqual([]);
  expect(result.longest.map((l: unknown[]) => [l[1], l[3], l[4]])).toEqual([[0, 31, 39]]);
  expect(result.chain).toEqual([[`${url38}${'b'.repeat(38)}end/x`, 0, 2, 2, 6]]);
  expect(result.short).toEqual([]);
  expect(result.indent).toEqual([]);
  expect(result.fits).toEqual([]);
  expect(result.another).toEqual([]);
  expect(result.soft).toEqual([]);
});

/** Records the URLs the page opens in a new tab (window.open) from now on; the returned function reads them. */
async function recordOpened(browser: Browser): Promise<() => Promise<string[]>> {
  await browser.evaluate(() => {
    const page = window as unknown as { __opened: string[]; open: (url: string) => null };
    page.__opened = [];
    page.open = (url) => { page.__opened.push(String(url)); return null; };
  });
  return () => browser.evaluate(() => (window as unknown as { __opened: string[] }).__opened);
}

test('a URL Claude Code wraps onto the next rows opens whole from any of them (#940)', async ({ app, browser }) => {
  const whole = 'https://example.com/admin/connectors?tab=all&page=2&sort=name&filter=active&view=compact'
    + '&columns=name,owner,status,updated&return=%2Fadmin%2Fconnectors%2Fsettings%2Fsecurity%2Fsigning-keys'
    + '&hint=rotate-the-signing-key-before-the-next-deploy&after=%2Fadmin%2Fdeploys%2Fproduction%2Fchecklist'
    + '&note=the-old-key-stays-valid-for-twenty-four-hours-after-the-rotation&state=done';
  // The mock cuts the URL for the terminal's width, as Claude Code does: each row after the first
  // is a part that only a joined link can open. A press is tried on the first row, the next one
  // and the last.
  await app.open(mock('/s/studio/t/12', '&wrapurl=1'));
  await expect(rows(browser)).toHaveText(/Sign in at/);
  await holdStill(browser);
  const parts = await browser.evaluate(() => {
    const lines = [...document.querySelectorAll('.taskview .xterm-rows > div')].map((d) => (d.textContent || '').trim());
    const first = lines.findIndex((l) => l.includes('Sign in at'));
    const continued = [];
    for (let i = first + 1; i < lines.length && lines[i].length && !lines[i].includes(' '); i += 1) continued.push(lines[i]);
    return continued;
  });
  expect(parts.length, 'the URL continues on the next rows').toBeGreaterThanOrEqual(1);
  expect(whole.endsWith(parts.join('')), 'the rows continue the URL').toBe(true);
  const opened = await recordOpened(browser);
  const presses = ['example.com/admin', parts[0].slice(0, 16), parts[parts.length - 1].slice(-12)];
  for (const [i, part] of presses.entries()) {
    await hoverLink(browser, part);
    await press(browser);
    await expect.poll(opened, { message: `a press on ${part} opens the whole URL` }).toHaveLength(i + 1);
  }
  expect(await opened()).toEqual(presses.map(() => whole));
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

type Clip = { __written: string[]; __refuse: boolean; __copied: string[] };

/**
 * Records the clipboard: `written()` what the clipboard API was given (`refuse(true)` rejects it,
 * as Safari does outside a key press) and `copied()` what each copy event carried.
 */
async function recordClipboard(browser: Browser) {
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
  return {
    written: () => browser.evaluate(() => (window as unknown as Clip).__written),
    copied: () => browser.evaluate(() => (window as unknown as Clip).__copied),
    refuse: (on: boolean) => browser.evaluate((value) => { (window as unknown as Clip).__refuse = value; }, on),
  };
}

/**
 * The program prints `sequence` to every viewer, unasked, then the line `marker`; once the marker
 * shows, the terminal has handled the sequence (output is parsed in order).
 */
async function printUnasked(browser: Browser, sequence: string, marker: string): Promise<void> {
  await browser.evaluate(([text, line]) => {
    for (const [, session] of (window as unknown as { __yuliusMock: { sessions: Map<string, { broadcast(text: string): void; appendLine(line: string): void }> } }).__yuliusMock.sessions) {
      session.broadcast(text);
      session.appendLine(line);
    }
  }, [sequence, marker] as const);
  await expect(rows(browser)).toHaveText(new RegExp(marker));
}

test('a selection the program copies (OSC 52) reaches the clipboard, or the copy key when the browser refuses', async ({ app, browser }) => {
  // Task #149, SPEC-158: Claude Code selects a plain drag itself and copies it with OSC 52, as the
  // mock does; the browser clipboard must get it.
  await app.open(mock('/s/studio/t/12'));
  await expect(rows(browser)).toHaveText(/Read 142 lines/);
  await holdStill(browser);
  const clip = await recordClipboard(browser);
  const notice = browser.locator('.taskview .term-notice');

  // A copy the program prints while this viewer has not used the terminal writes nothing.
  await printUnasked(browser, `\x1b]52;c;${btoa('unasked')}\x07`, 'marker one');
  expect(await clip.written(), 'no write without this viewer\'s input').toEqual([]);

  // A plain drag is the program's: it selects and copies the text, which reaches the clipboard.
  await selectRow(browser, 'Read 142 lines');
  await expect.poll(clip.written).toEqual([expect.stringContaining('Read 142 lines')]);
  await expect(notice).toHaveText('Copied');
  expect(await selected(browser), 'xterm itself selects nothing').toBe(false);

  // A query is never answered, and nothing is written for it.
  const sent = await recordInput(browser);
  await printUnasked(browser, '\x1b]52;c;?\x07', 'marker two');
  expect(await sent(), 'no answer to a query').toEqual([]);
  expect((await clip.written()).length).toBe(1);

  // A browser that refuses the write: the copy key copies the program's text.
  await clip.refuse(true);
  await selectRow(browser, 'Read 142 lines');
  const mac = await browser.evaluate(() => /Mac|iPhone|iPad/.test(navigator.platform || navigator.userAgent || ''));
  const copyKey = mac ? 'Meta+c' : 'Control+Shift+c';
  await expect(notice).toHaveText(mac ? 'Press ⌘C to copy' : 'Press Ctrl+Shift+C to copy');
  await browser.keyboard.press(copyKey);
  await expect.poll(clip.copied).toEqual([expect.stringContaining('Read 142 lines')]);
  await expect(notice).toHaveText('Copied');

  // xterm's own selection wins over a refused program copy, and drops it (OSC-004).
  await selectRow(browser, 'Read 142 lines');
  await expect(notice).toHaveText(mac ? 'Press ⌘C to copy' : 'Press Ctrl+Shift+C to copy');
  await keySelectRow(browser, 'Searched for 2 patterns');
  await expect.poll(() => selected(browser), { message: 'a Shift-drag selects' }).toBe(true);
  await browser.keyboard.press(copyKey);
  await expect.poll(clip.copied).toEqual([expect.stringContaining('Read 142 lines'), expect.stringContaining('Searched for 2 patterns')]);
  await rows(browser).click({ position: { x: 300, y: 8 } });
  expect(await selected(browser), 'a click clears the selection').toBe(false);
  // The copy event fires during the key press, so it would be recorded by now.
  await browser.keyboard.press(copyKey);
  expect((await clip.copied()).length, 'the program copy went with the selection').toBe(2);
});

test('a read-only viewer’s clipboard is never written by the program', async ({ app, browser, screen }) => {
  // SPEC-158 OSC-003: a viewer who cannot write only watches; the program's copy is not theirs,
  // even right after they clicked into the terminal.
  await app.open(mock('/s/sandbox-3/t/1'));
  await expect(screen.getByText('Read-only (GitHub: read)')).toBeVisible();
  await expect(rows(browser)).toHaveText(/Read 142 lines/);
  const clip = await recordClipboard(browser);
  await rows(browser).click({ position: { x: 300, y: 8 } });
  await printUnasked(browser, `\x1b]52;c;${btoa('not yours')}\x07`, 'marker four');
  expect(await clip.written(), 'nothing written for a read-only viewer').toEqual([]);
  await expect(browser.locator('.taskview .term-notice')).not.toHaveText(/Copied|to copy/);
});

test('a read-only viewer cannot paste', async ({ app, browser, screen }) => {
  await app.open(mock('/s/sandbox-3/t/1'));
  await expect(screen.getByText('Read-only (GitHub: read)')).toBeVisible();
  // That banner is the sandbox's; the terminal says read-only once its first state arrived, which
  // shows it. A paste before then has no terminal to land in (#354).
  await terminalShown(browser);
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
  // nor cancels them (SPEC-098 MSE-006, #78). The turn above sends a report per row: all of them
  // arrive first.
  await settle();
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

test('a wheel turn reports one row per row of its distance (#863)', async ({ app, browser }) => {
  await app.open(mock('/s/studio/t/12'));
  await expect(rows(browser)).toHaveText(/Searched for 2 patterns/);
  await holdStill(browser);
  const sent = await recordInput(browser);
  const point = await spot(browser, 'Searched for 2 patterns');
  expect(point, 'the group is on the screen').not.toBeNull();
  // Claude Code moves one row per wheel report, so a turn must send a report per row it covers:
  // xterm alone sends one per event and damps small (trackpad) ones (SPEC-280 WHL-001, WHL-002).
  const prevented = await browser.evaluate(({ x, y }) => {
    const target = document.elementFromPoint(x, y)!;
    const screen = document.querySelector('.taskview .xterm-screen')!;
    const row = screen.getBoundingClientRect().height / document.querySelectorAll('.taskview .xterm-rows > div').length;
    const wheel = (deltaY: number, deltaMode = 0) => {
      const event = new WheelEvent('wheel', { bubbles: true, cancelable: true, composed: true, clientX: x, clientY: y, deltaMode, deltaY });
      target.dispatchEvent(event);
      return event.defaultPrevented;
    };
    const turns = [wheel(-5.5 * row)];
    for (let i = 0; i < 10; i++) turns.push(wheel(-0.2 * row));
    turns.push(wheel(-3, 1), wheel(2.5 * row));
    return turns.every(Boolean);
  }, point!);
  expect(prevented, 'the page does not scroll as well').toBe(true);
  const count = (all: string[], button: number) => all.join('').split(`\x1b[<${button};`).length - 1;
  // 5.5 rows: 5 up; ten of 0.2 with the half row carried: 2; three lines: 3; then 2.5 rows down: 2.
  await expect.poll(async () => count(await sent(), 64), { message: 'wheel-up reports' }).toBe(10);
  await expect.poll(async () => count(await sent(), 65), { message: 'wheel-down reports' }).toBe(2);
  // Each report is xterm's, at the pointer's cell (WHL-003).
  const cell = await browser.evaluate(({ x, y }) => {
    const box = document.querySelector('.taskview .xterm-screen')!.getBoundingClientRect();
    type WithSessions = { __yuliusMock: { sessions: Map<string, { cols: number }> } };
    const session = (window as unknown as WithSessions).__yuliusMock.sessions.get('tsk_studio_12')!;
    const rowCount = document.querySelectorAll('.taskview .xterm-rows > div').length;
    return `${Math.floor((x - box.left) / (box.width / session.cols)) + 1};${Math.floor((y - box.top) / (box.height / rowCount)) + 1}M`;
  }, point!);
  const cells = new Set((await sent()).join('').match(/\x1b\[<6[45];\d+;\d+M/g)!.map((report) => report.slice(6)));
  expect([...cells], 'every report at the pointer\'s cell').toEqual([cell]);
  // Two pages send one screen of rows (WHL-002). A modified wheel stays xterm's (WHL-004): it
  // reports Alt+wheel once with Alt's flag (SGR 64 + 8) and Shift+wheel not at all.
  const before = (await sent()).length;
  const screenRows = await browser.evaluate(({ x, y }) => {
    const target = document.elementFromPoint(x, y)!;
    for (const init of [{ deltaY: -2, deltaMode: 2 }, { deltaY: -400, shiftKey: true }, { deltaY: -400, altKey: true }]) {
      target.dispatchEvent(new WheelEvent('wheel', { bubbles: true, cancelable: true, composed: true, clientX: x, clientY: y, ...init }));
    }
    return document.querySelectorAll('.taskview .xterm-rows > div').length;
  }, point!);
  await expect.poll(async () => count((await sent()).slice(before), 72), { message: 'the Alt wheel, once' }).toBe(1);
  await settle();
  const after = (await sent()).slice(before);
  expect(count(after, 64), 'two pages: one screen of rows').toBe(screenRows);
  expect(count(after, 68), 'the Shift wheel: nothing').toBe(0);
  expect(count(after, 72), 'the Alt wheel: once').toBe(1);
});

// ---- the whole session past the terminal's top (SPEC-326, #911)

type Item = { role: string; text: string };
type Session12 = { lines: string[]; viewers: Set<unknown>; broadcast(text: string): void; snapshot(): string; wheel(button: number): void };
type HistoryMock = { __yuliusMock: { sessions: Map<string, Session12>; transcriptQueries?: string[]; appendTranscript(sid: string, n: number, items: Item[]): void; newTranscriptSession(sid: string, n: number, session: string, items: Item[]): void } };
const FIRST_MESSAGE = 'Add Stripe checkout to the cart page. Use the test keys from .env.';
const offer = (browser: Browser) => browser.locator('.taskview .term-history-offer');
const history = (browser: Browser) => browser.locator('.taskview .watch.history');
/** Whether the way on to the whole session is on the page now. */
const offerShown = (browser: Browser) => browser.evaluate(() => {
  const button = document.querySelector<HTMLElement>('.taskview .term-history-offer');
  return Boolean(button && button.checkVisibility());
});
/** The terminal's rows as text. */
const shown = (browser: Browser) => browser.evaluate(() => document.querySelector('.taskview .xterm-rows')!.textContent);
/** The task transcript reads made so far. */
const queries = (browser: Browser) => browser.evaluate(() => (window as unknown as HistoryMock).__yuliusMock.transcriptQueries || []);
/** Turns the wheel over `point` by `deltaY` pixels (negative: up). */
const turn = (browser: Browser, point: { x: number; y: number }, deltaY: number) => browser.evaluate(({ x, y, deltaY }) => {
  document.elementFromPoint(x, y)!.dispatchEvent(new WheelEvent('wheel', { bubbles: true, cancelable: true, composed: true, clientX: x, clientY: y, deltaY }));
}, { ...point, deltaY });
/** Turns the wheel up by `count` of the terminal's rows. */
const turnRowsUp = (browser: Browser, point: { x: number; y: number }, count: number) => browser.evaluate(({ x, y, count }) => {
  const screen = document.querySelector('.taskview .xterm-screen')!;
  const row = screen.getBoundingClientRect().height / document.querySelectorAll('.taskview .xterm-rows > div').length;
  document.elementFromPoint(x, y)!.dispatchEvent(new WheelEvent('wheel', { bubbles: true, cancelable: true, composed: true, clientX: x, clientY: y, deltaY: -count * row }));
}, { ...point, count });
/** Waits until the terminal's rows stop changing. */
async function screenStill(browser: Browser): Promise<void> {
  let last = '';
  await expect.poll(async () => { const now = await shown(browser); const still = now === last; last = now; await settle(300); return still; }, { message: 'the screen stopped moving' }).toBe(true);
}

/** Takes the way on away with a key that types nothing (Shift), and checks it went. */
async function hideOfferWithKey(browser: Browser): Promise<void> {
  await browser.evaluate(() => document.querySelector<HTMLElement>('.taskview .xterm-helper-textarea')!.focus());
  await browser.keyboard.press('Shift');
  expect(await offerShown(browser), 'a key takes the offer away').toBe(false);
}

/** Opens the whole session from task #12's terminal (an old run's: the first turn up offers it). */
async function openClassicHistory(browser: Browser, point: { x: number; y: number }): Promise<void> {
  await turn(browser, point, -120);
  await offer(browser).click();
  await expect(history(browser)).toBeVisible();
}

/** Task #12 with a transcript longer than its screen, so the program has rows to scroll; the pointer's spot. */
async function longTask12(app: { open: (url: string) => Promise<unknown> }, browser: Browser): Promise<{ x: number; y: number }> {
  await app.open(mock('/s/studio/t/12'));
  await expect(rows(browser)).toHaveText(/Searched for 2 patterns/);
  await holdStill(browser);
  await browser.evaluate(() => {
    const session = (window as unknown as HistoryMock).__yuliusMock.sessions.get('tsk_studio_12')!;
    session.lines.push(...Array.from({ length: 60 }, (_, i) => `Earlier output line ${i + 1}`));
    session.broadcast(session.snapshot());
  });
  await expect(rows(browser)).toHaveText(/Earlier output line 60/);
  const point = await spot(browser, 'Earlier output line 60');
  expect(point, 'the last line is on the screen').not.toBeNull();
  return point!;
}

/** Task #12 as an old run on Claude Code's classic renderer; the pointer's spot. */
async function classicTask12(app: { open: (url: string) => Promise<unknown> }, browser: Browser): Promise<{ x: number; y: number }> {
  // Started before SPEC-310, the run kept Claude Code's main-screen renderer through the updates
  // that froze and thawed it: no mouse reporting, and only the visible screen reaches a viewer.
  await app.open(mock('/s/studio/t/12', '&classic=1'));
  await expect(rows(browser)).toHaveText(/Searched for 2 patterns/);
  await holdStill(browser);
  const point = await spot(browser, 'Searched for 2 patterns');
  expect(point, 'the group is on the screen').not.toBeNull();
  return point!;
}

test('a wheel turn past the top offers the whole session from its first message (#911)', async ({ app, browser, screen }) => {
  const point = await longTask12(app, browser);
  // Claude Code shows a session only from its last compaction: what scrolls still scrolls, and a
  // turn that moved the screen offers nothing (SPEC-326 HIS-001).
  const before = await shown(browser);
  await turnRowsUp(browser, point, 1);
  await expect.poll(() => shown(browser), { message: 'the program scrolled' }).not.toBe(before);
  await settle(1000);
  await expect(offer(browser)).toBeHidden();
  // Up to the program's top: the screen stops moving.
  for (let i = 0; i < 3; i++) await turnRowsUp(browser, point, 200);
  await expect(rows(browser)).toHaveText(/Welcome to Claude/);
  await screenStill(browser);
  const atTop = await shown(browser);
  // The gesture may already have reached the top: a key takes any offer away first (HIS-002).
  await hideOfferWithKey(browser);
  // A turn past it leaves the screen as it was: the way on to the whole session shows.
  await settle(TOP_SETTLE_MS);
  await turnRowsUp(browser, point, 3);
  await expect(offer(browser)).toBeVisible();
  await expect(offer(browser)).toHaveText(/Earlier messages: show the whole session/);
  // The terminal's connection is kept while the whole session shows.
  await browser.evaluate(() => {
    const session = (window as unknown as HistoryMock).__yuliusMock.sessions.get('tsk_studio_12')!;
    (window as unknown as { __viewer: unknown }).__viewer = [...session.viewers][0];
  });
  await offer(browser).click();

  // The whole session, read from its first line, in the terminal's place (HIS-003).
  await expect(history(browser)).toBeVisible();
  await expect(screen.getByText(FIRST_MESSAGE)).toBeVisible();
  expect((await queries(browser))[0], 'read from the first line').toMatch(/^from=0&limit=500$/);
  await expect(browser.locator('.taskview .watch.history .ss-locked')).toHaveText(/The whole session, from its first message\. Read-only\./);
  expect(await browser.evaluate(() => document.querySelectorAll('.taskview .xterm').length), 'the terminal is kept').toBe(1);
  await expect(browser.locator('.taskview .term')).toBeHidden();
  await screen.getByRole('button', { name: 'Back to the terminal' }).click();
  await expect(history(browser)).toBeHidden();
  await expect(rows(browser)).toBeVisible();
  expect(await browser.evaluate(() => {
    const session = (window as unknown as HistoryMock).__yuliusMock.sessions.get('tsk_studio_12')!;
    return session.viewers.size === 1 && [...session.viewers][0] === (window as unknown as { __viewer: unknown }).__viewer;
  }), 'the same connection, never closed').toBe(true);

  // A turn back down hides the way on and scrolls the program down.
  await turnRowsUp(browser, point, 3);
  await expect(offer(browser)).toBeVisible();
  await turn(browser, point, 40);
  await expect(offer(browser)).toBeHidden();
  await expect.poll(() => shown(browser), { message: 'the program scrolled down' }).not.toBe(atTop);
  // The turn back up moves it and offers nothing; the next one does.
  await turnRowsUp(browser, point, 200);
  await expect.poll(() => shown(browser), { message: 'back at the top' }).toBe(atTop);
  await settle(TOP_SETTLE_MS + 600);
  await expect(offer(browser)).toBeHidden();
  await turnRowsUp(browser, point, 3);
  await offer(browser).click();
  await expect(history(browser)).toBeVisible();
  // Escape goes back, with the keyboard in the terminal again.
  await browser.keyboard.press('Escape');
  await expect(history(browser)).toBeHidden();
  await expect(rows(browser)).toBeVisible();
  await expect.poll(() => browser.evaluate(() => Boolean(document.activeElement && document.activeElement.closest('.taskview .xterm'))), { message: 'the keyboard is in the terminal' }).toBe(true);
});

test('a program that answers the wheel late takes the offer back once its screen moves (#911)', async ({ app, browser }) => {
  const point = await longTask12(app, browser);
  // Over a slow link the program's screen arrives after the check's wait: the turn was no top.
  await browser.evaluate((delay) => {
    const session = (window as unknown as HistoryMock).__yuliusMock.sessions.get('tsk_studio_12')!;
    const wheel = session.wheel.bind(session);
    session.wheel = (button) => { setTimeout(() => wheel(button), delay); };
  }, TOP_SETTLE_MS + 300);
  const before = await shown(browser);
  await turnRowsUp(browser, point, 5);
  await expect.poll(() => shown(browser), { message: 'the program scrolled, late' }).not.toBe(before);
  await expect.poll(() => offerShown(browser), { message: 'no offer while the program scrolls' }).toBe(false);
});

test('an old run on the classic renderer, with nothing above its screen, offers the whole session at once (#911)', async ({ app, browser, screen }) => {
  const point = await classicTask12(app, browser);
  const sent = await recordInput(browser);
  // xterm scrolls the main screen itself and is at its first line: the first turn up is the top.
  await turn(browser, point, -120);
  await expect(offer(browser)).toBeVisible();
  expect((await sent()).join('').includes('\x1b['), 'nothing reaches a program without the mouse').toBe(false);
  // The keyboard opens it too: Space on the button (the key does not take it away first).
  await browser.evaluate(() => document.querySelector<HTMLElement>('.taskview .term-history-offer')!.focus());
  await browser.keyboard.press('Space');
  await expect(history(browser)).toBeVisible();
  await expect(screen.getByText(FIRST_MESSAGE)).toBeVisible();
});

test('a live viewer of an old run scrolls its scrollback first, then is offered the whole session (#911)', async ({ app, browser }) => {
  const point = await classicTask12(app, browser);
  // The program writes on while this viewer watches: lines scroll off the screen into xterm's scrollback.
  await browser.evaluate(() => {
    const session = (window as unknown as HistoryMock).__yuliusMock.sessions.get('tsk_studio_12')!;
    session.broadcast(`\x1b[r\x1b[999;1H${Array.from({ length: 80 }, (_, i) => `\r\nOld output ${i + 1}`).join('')}`);
  });
  await expect(rows(browser)).toHaveText(/Old output 80/);
  // Turns within the scrollback scroll it and offer nothing. xterm scrolls its own buffer only for
  // the browser's own wheel events.
  await browser.mouse.move(point.x, point.y);
  const bottom = await shown(browser);
  await browser.mouse.wheel(0, -120);
  await expect.poll(() => shown(browser), { message: 'the scrollback scrolled' }).not.toBe(bottom);
  await settle(TOP_SETTLE_MS + 600);
  expect(await offerShown(browser), 'no offer within the scrollback').toBe(false);
  // At its first line, the next turn offers the whole session.
  let last = '';
  await expect.poll(async () => {
    await browser.mouse.wheel(0, -2000);
    await settle(300);
    const now = await shown(browser);
    const still = now === last;
    last = now;
    return still;
  }, { message: 'the scrollback is at its first line', timeout: 30_000 }).toBe(true);
  await expect(rows(browser)).toHaveText(/Welcome to Claude Code/);
  await hideOfferWithKey(browser);
  await browser.mouse.wheel(0, -120);
  await expect(offer(browser)).toBeVisible();
});

test('the whole session is read page by page to its end, and from its first line again for a new session (#911)', async ({ app, browser, screen }) => {
  const point = await classicTask12(app, browser);
  await browser.evaluate(() => (window as unknown as HistoryMock).__yuliusMock.appendTranscript('sb_studio', 12, Array.from({ length: 600 }, (_, i) => ({ role: 'assistant', text: `Step ${i + 1} of 600.` }))));
  const readsBefore = (await queries(browser)).length;
  await turn(browser, point, -120);
  await offer(browser).click();
  await expect(history(browser)).toBeVisible();
  // It starts at the bottom, the newest message, and the first one is above it.
  await expect(screen.getByText('Step 600 of 600.')).toBeVisible();
  await browser.evaluate(() => { document.querySelector('.taskview .watch.history .watch-scroll')!.scrollTop = 0; });
  await expect(screen.getByText(FIRST_MESSAGE)).toBeVisible();
  const reads = (await queries(browser)).slice(readsBefore);
  expect(reads[0], 'the first page').toBe('from=0&limit=500');
  expect(reads[1], 'the next page, as large').toMatch(/^from=500&session=b1f0-12&limit=500$/);
  // A continue started another session: it is read from its first line too.
  const readsThen = (await queries(browser)).length;
  await browser.evaluate(() => (window as unknown as HistoryMock).__yuliusMock.newTranscriptSession('sb_studio', 12, 'b1f0-13', [{ role: 'user', text: 'Continue where you left off.' }]));
  await expect(screen.getByText('Continue where you left off.')).toBeVisible();
  await expect(screen.getByText('Step 600 of 600.')).toBeHidden();
  expect((await queries(browser)).slice(readsThen), 'the new session from its first line').toContain('from=0&limit=500');
});

test('the attention bar’s Reply leaves the whole session for the terminal (#911)', async ({ app, browser, screen }) => {
  const point = await classicTask12(app, browser);
  // The agent asks in the terminal: Reply puts the keyboard there, so the terminal shows again.
  await openClassicHistory(browser, point);
  await touchTask(browser, 'sb_studio', 12, { activity: 'needs_attention', needs_you: { action: 'Add a ruleset', why: 'agents can push to main', at: new Date().toISOString() } });
  await expect(browser.locator('.attention-bar')).toHaveText(/Needs you[\s\S]*To do[\s\S]*Add a ruleset/);
  await screen.getByRole('button', 'Reply').click();
  await expect(history(browser)).toBeHidden();
  await expect.poll(() => browser.evaluate(() => Boolean(document.activeElement && document.activeElement.closest('.taskview .xterm'))), { message: 'the keyboard is in the terminal' }).toBe(true);
  // A text reply is typed into the session: the terminal shows it arrive.
  await openClassicHistory(browser, point);
  await touchTask(browser, 'sb_studio', 12, { activity: 'waiting', needs_you: null, background_shells: 0 });
  await expect(browser.locator('.attention-bar')).toHaveText(/waiting for your reply/);
  await screen.getByRole('button', 'Reply').click();
  await screen.getByRole('textbox', 'Reply to the agent').fill('Go on');
  await screen.getByRole('button', 'Send').click();
  await expect(history(browser)).toBeHidden();
  await expect(rows(browser)).toHaveText(/> Go on/);
});

test('the way on sits below a held card over the terminal (#911)', async ({ app, browser }) => {
  const point = await classicTask12(app, browser);
  await touchTask(browser, 'sb_studio', 12, { heldPrompt: { id: 'hold_9', class: 'new_scope', excerpt: 'Also add a dark mode to the invoice settings page.', percent: 92, typed_by: 'mkarp', created_at: new Date().toISOString() } });
  await expect(browser.locator('.taskview .held-slot')).toBeVisible();
  await turn(browser, point, -120);
  await expect(offer(browser)).toBeVisible();
  const boxes = await browser.evaluate(() => {
    const card = document.querySelector('.taskview .held-slot')!.getBoundingClientRect();
    const button = document.querySelector('.taskview .term-history-offer')!.getBoundingClientRect();
    return { cardBottom: card.bottom, buttonTop: button.top };
  });
  expect(boxes.buttonTop, 'the button is below the card').toBeGreaterThanOrEqual(boxes.cardBottom);
});

test('a task that never ran offers no earlier messages (#911)', async ({ app, browser }) => {
  await app.open(mock('/s/product/t/7'));
  // The mock draws a screen even for a waiting task: the task's state alone holds the offer back.
  await expect(rows(browser)).toHaveText(/Welcome to Claude/);
  const point = await browser.evaluate(() => {
    const box = document.querySelector('.taskview .xterm-screen')!.getBoundingClientRect();
    return { x: box.left + 40, y: box.top + 20 };
  });
  await turn(browser, point, -120);
  await settle(TOP_SETTLE_MS + 600);
  expect(await offerShown(browser), 'nothing to offer before a session').toBe(false);
});

test('a blank screen offers nothing (#911)', async ({ app, browser }) => {
  const point = await classicTask12(app, browser);
  // The program cleared the screen and its scrollback: nothing earlier to show here.
  await browser.evaluate(() => (window as unknown as HistoryMock).__yuliusMock.sessions.get('tsk_studio_12')!.broadcast('\x1b[r\x1b[2J\x1b[3J\x1b[H'));
  await expect.poll(() => browser.evaluate(() => document.querySelector('.taskview .xterm-rows')!.textContent!.trim())).toBe('');
  await turn(browser, point, -120);
  await settle(TOP_SETTLE_MS + 600);
  expect(await offerShown(browser), 'a blank screen offers nothing').toBe(false);
});

test('a reply that could not be sent shows why in the terminal, not behind the whole session (#911)', async ({ app, browser, screen }) => {
  const point = await classicTask12(app, browser);
  await openClassicHistory(browser, point);
  await touchTask(browser, 'sb_studio', 12, { activity: 'waiting', needs_you: null, background_shells: 0 });
  await expect(browser.locator('.attention-bar')).toHaveText(/waiting for your reply/);
  await browser.evaluate(() => (window as unknown as { __yuliusMock: { goOffline(): void } }).__yuliusMock.goOffline());
  await screen.getByRole('button', 'Reply').click();
  await screen.getByRole('textbox', 'Reply to the agent').fill('Go on');
  await screen.getByRole('button', 'Send').click();
  await expect(history(browser)).toBeHidden();
  await expect(browser.locator('.taskview .term-notice')).toHaveText('Not connected — input was not sent');
});

test('dismissing a queued message kept after the start leaves the whole session for the terminal (#911)', async ({ app, browser, screen }) => {
  await app.open(mock('/s/product/t/7'));
  const card = screen.getByRole('region', 'Queued messages');
  await card.getByRole('textbox', 'Message for the agent').click();
  await browser.keyboard.type('Only for paid invoices, please.');
  await touchTask(browser, 'sb_product', 7, { status: 'running', started: true });
  await expect(card).toContainText('The task has started: copy your message into its session.');
  await expect(rows(browser)).toHaveText(/Welcome to Claude/);
  const point = await browser.evaluate(() => {
    const box = document.querySelector('.taskview .xterm-screen')!.getBoundingClientRect();
    return { x: box.left + 40, y: box.top + 20 };
  });
  await expect.poll(async () => { await turn(browser, point, -120); await settle(TOP_SETTLE_MS + 200); return offerShown(browser); }, { message: 'the started task offers its session' }).toBe(true);
  await offer(browser).click();
  await expect(history(browser)).toBeVisible();
  await browser.locator('.queue-compose > button:has-text("Dismiss")').click();
  await expect(history(browser)).toBeHidden();
  await expect(rows(browser)).toBeVisible();
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
  await editorShown(browser);
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
  await editorShown(browser);
  await expect(screen.getByRole('tab', /AGENTS\.md/)).toHaveAttribute('aria-selected', 'true');
});
