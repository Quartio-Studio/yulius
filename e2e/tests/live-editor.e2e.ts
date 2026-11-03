// SPEC-411: the Editor's text is shared live and saves itself. Two people are two windows: this
// page (mkarp) and one it opens (bob); the mock keeps one live document per file in this page,
// which Bob's window shares, as two browsers share the server's.
import { test, type Browser } from '@e2e-dev/web';
import { expect } from 'e2e';
import { editorShown, failOnPageErrors, MONACO_LOAD_MS, mock } from './support.ts';

failOnPageErrors();

type Who = 'me' | 'bob';
type LiveHub = {
  holdEdits: boolean;
  holdSaves: boolean;
  edits: { path: string; client: string; at: number }[];
  saveAsks: { path: string; force: boolean; at: number }[];
  saves: { path: string; text: string; force: boolean }[];
  diskAsks: { path: string; force: boolean }[];
};
type LiveMock = {
  live: LiveHub;
  liveMuted?: boolean;
  repoFile(sandbox: string, path: string, text: string): void;
  repoFileTooLarge(sandbox: string, path: string): void;
  fileReads?: { path: string }[];
  repoTrees: Record<string, { files: Map<string, string> }>;
  liveDoc(path: string): { session: string; revision: number; text: string; saved: boolean; disk_sha256: string | null } | null;
  dropLive(): void;
  releaseEdits(): void;
  releaseSaves(): void;
  resyncNow(): void;
  request(method: string, path: string, body?: unknown): Promise<number>;
};
type Ed = {
  getModel(): { getValue(): string; getOffsetAt(p: { lineNumber: number; column: number }): number } | null;
  setPosition(p: { lineNumber: number; column: number }): void;
  trigger(source: string, handler: string, payload: unknown): void;
  focus(): void;
};
type Win = Window & { __bob: Window; __yuliusMock: LiveMock; monaco: { editor: { getEditors(): Ed[] } } };

const FILE = 'notes.txt';
const ON_DISK = 'alpha\nbeta\ngamma\n';

/** Runs `body` (a function body of `ed`, `w` and `mock`) in `who`'s window. */
const inWindow = <T extends import('@e2e-dev/web').JsonValue>(browser: Browser, who: Who, body: string) =>
  browser.evaluate(([whose, code]) => {
    const page = window as unknown as Win;
    const w = (whose === 'bob' ? page.__bob : window) as unknown as Win;
    const ed = w.monaco?.editor.getEditors()[0];
    return new Function('ed', 'w', 'mock', code)(ed, w, page.__yuliusMock);
  }, [who, body]) as Promise<T>;

const textOf = (browser: Browser, who: Who) => inWindow<string>(browser, who, "return ed?.getModel()?.getValue() ?? ''");
const statusOf = (browser: Browser, who: Who) => inWindow<string>(browser, who, "return w.document.querySelector('.ed-status')?.textContent ?? ''");
const onDisk = (browser: Browser) => inWindow<string>(browser, 'me', `return mock.repoTrees['sb_studio|sandbox'].files.get('${FILE}') ?? ''`);
/** Types `text` at `line`:`column` in `who`'s editor, as the keyboard does; answers when (ms). */
const typeAt = (browser: Browser, who: Who, line: number, column: number, text: string) =>
  inWindow<number>(browser, who, `ed.focus(); ed.setPosition({ lineNumber: ${line}, column: ${column} }); ed.trigger('keyboard', 'type', { text: ${JSON.stringify(text)} }); return Date.now();`);

/** This page in the Editor with `FILE` in front. */
async function openHere(app: { open(url: string): Promise<void> }, browser: Browser, screen: import('@e2e-dev/web').Screen) {
  await app.open(mock('/s/studio/t/12'));
  await inWindow(browser, 'me', `mock.repoFile('sb_studio', '${FILE}', ${JSON.stringify(ON_DISK)})`);
  await screen.getByRole('button', /^Show tools/).click();
  await screen.getByRole('button', 'Editor').click();
  await screen.getByRole('tree', 'Files').getByRole('treeitem', FILE).click();
  await editorShown(browser);
  await expect.poll(() => textOf(browser, 'me')).toBe(ON_DISK);
}

/** Clicks the first element matching `selector` (with `label`) in `who`'s window, once it is there. */
async function clickIn(browser: Browser, who: Who, selector: string, label = '') {
  await expect.poll(() => inWindow<boolean>(browser, who, `
    const el = [...w.document.querySelectorAll(${JSON.stringify(selector)})].find((node) => !${JSON.stringify(label)} || (node.textContent ?? '').trim() === ${JSON.stringify(label)} || (node.getAttribute('aria-label') ?? '').startsWith(${JSON.stringify(label)}));
    if (!el) return false;
    el.click();
    return true;`), { message: `click ${selector} ${label}` }).toBe(true);
}

/** Bob, in his own window, has `FILE` open in the Editor. */
async function bobOpens(browser: Browser) {
  await browser.evaluate(() => { (window as unknown as Win).__bob = window.open(`${location.origin}/?mock=1&as=bob#/s/studio/t/12`, 'bob', 'width=1280,height=900') as Window; });
  await expect.poll(() => inWindow<boolean>(browser, 'bob', `
    const buttons = [...w.document.querySelectorAll('button')];
    const show = buttons.find((b) => (b.getAttribute('aria-label') ?? '').startsWith('Show tools'));
    if (show) show.click();
    return Boolean(show) || buttons.some((b) => (b.getAttribute('aria-label') ?? '').startsWith('Hide tools'));`)).toBe(true);
  await clickIn(browser, 'bob', '.rail button', 'Editor');
  await clickIn(browser, 'bob', '.ed-tree [role="treeitem"]', FILE);
  await expect.poll(() => inWindow<boolean>(browser, 'bob', `return w.document.querySelector('.ed-tab.active')?.getAttribute('data-path') === '${FILE}' && Boolean(w.monaco?.editor.getEditors()[0]?.getModel()?.getValue())`), { timeout: MONACO_LOAD_MS }).toBe(true);
}

/** Where the teammate drawn in `who`'s editor is: their caret's offset, or null. */
const peerHead = (browser: Browser, who: Who) => inWindow<string | null>(browser, who, "return w.document.querySelector('.ed-host .ed-layer .tl-peer:not(.tl-leaving)')?.dataset.head ?? null");

// AC-411-4: keystrokes reach the teammate before anyone saves; concurrent typing converges.
test('two people type into one file and see each other’s keystrokes before anything is saved', async ({ app, browser, screen }) => {
  await openHere(app, browser, screen);
  await bobOpens(browser);
  await expect.poll(() => textOf(browser, 'bob')).toBe(ON_DISK);
  // Nothing is saved in this test: the saves wait.
  await inWindow(browser, 'me', 'mock.live.holdSaves = true');

  await typeAt(browser, 'me', 1, 1, 'Hello ');
  await expect.poll(() => textOf(browser, 'bob')).toBe(`Hello ${ON_DISK}`);
  await typeAt(browser, 'bob', 3, 6, ' (bob)');
  await expect.poll(() => textOf(browser, 'me')).toBe('Hello alpha\nbeta\ngamma (bob)\n');

  // Both type at the same place while neither edit is answered: both land, in one order, the same
  // text in both windows.
  await inWindow(browser, 'me', 'mock.live.holdEdits = true');
  await typeAt(browser, 'me', 2, 1, 'M');
  await typeAt(browser, 'bob', 2, 1, 'B');
  await expect.poll(() => inWindow<number>(browser, 'me', 'return mock.live.edits.length')).toBeGreaterThanOrEqual(4);
  await inWindow(browser, 'me', 'mock.releaseEdits()');
  await expect.poll(async () => { const [mine, his] = [await textOf(browser, 'me'), await textOf(browser, 'bob')]; return mine === his ? mine : `${mine} ≠ ${his}`; }).toMatch(/^Hello alpha\n(MB|BM)beta\ngamma \(bob\)\n$/);
  expect(await inWindow<string>(browser, 'me', `return mock.liveDoc('${FILE}').text`)).toBe(await textOf(browser, 'me'));

  // Bob's caret is drawn on his character in this window, also while this person types before it.
  await inWindow(browser, 'bob', 'ed.setPosition({ lineNumber: 3, column: 6 })');
  const his = await inWindow<number>(browser, 'bob', 'return ed.getModel().getOffsetAt({ lineNumber: 3, column: 6 })');
  await expect.poll(() => peerHead(browser, 'me')).toBe(String(his));
  await inWindow(browser, 'me', 'mock.live.holdEdits = true');
  await typeAt(browser, 'me', 1, 1, '>> ');
  await expect.poll(() => peerHead(browser, 'me')).toBe(String(his + 3));
  await inWindow(browser, 'me', 'mock.releaseEdits()');
  await expect.poll(() => textOf(browser, 'bob')).toMatch(/^>> Hello/);
  await expect.poll(() => peerHead(browser, 'me')).toBe(String(his + 3));

  // Nothing of it was written.
  expect(await onDisk(browser)).toBe(ON_DISK);
  expect(await inWindow<number>(browser, 'me', 'return mock.live.saves.length')).toBe(0);
  await browser.evaluate(() => (window as unknown as Win).__bob.close());
});

// AC-411-5: typing saves by itself once it stops, and leaving the file saves at once.
test('typing saves by itself a second after it stops, and leaving the file saves at once', async ({ app, browser, screen }) => {
  await openHere(app, browser, screen);
  const typed = await typeAt(browser, 'me', 1, 1, 'one ');
  await expect(screen.getByRole('tab', /^notes\.txt\s*, modified/)).toBeVisible();
  await expect.poll(() => onDisk(browser)).toBe(`one ${ON_DISK}`);
  await expect.poll(() => statusOf(browser, 'me')).toContain('Saved');
  await expect(screen.getByRole('tab', /^notes\.txt$/)).toBeVisible();
  // Asked once typing had stopped for the autosave delay, not on every key, and never by Ctrl+S.
  const timing = await inWindow<{ waited: number; asks: number }>(browser, 'me', `
    const asks = mock.live.saveAsks.filter((a) => a.path === '${FILE}');
    return { waited: asks[0].at - ${typed}, asks: asks.length };`);
  const AUTOSAVE_MS = await browser.evaluate(async () => (await import('/assets/livedoc.js' as string) as { AUTOSAVE_MS: number }).AUTOSAVE_MS);
  expect(timing.asks).toBe(1);
  expect(timing.waited).toBeGreaterThanOrEqual(AUTOSAVE_MS);

  // Leaving the text saves it without waiting: the autosave timer is held, the blur saves.
  await browser.evaluate(async () => {
    const { AUTOSAVE_MS: delay } = await import('/assets/livedoc.js' as string) as { AUTOSAVE_MS: number };
    const real = window.setTimeout.bind(window);
    window.setTimeout = ((fn: () => void, ms?: number, ...rest: unknown[]) => (ms === delay ? 0 : real(fn, ms, ...rest))) as typeof window.setTimeout;
  });
  await typeAt(browser, 'me', 1, 1, 'two ');
  await expect.poll(() => inWindow<number>(browser, 'me', `return mock.live.edits.filter((e) => e.path === '${FILE}').length`)).toBeGreaterThan(1);
  expect(await onDisk(browser)).toBe(`one ${ON_DISK}`);
  await screen.getByRole('tree', 'Files').getByRole('treeitem', 'README.md').click();
  await expect.poll(() => onDisk(browser)).toBe(`two one ${ON_DISK}`);
});

// AC-411-6: the disk under saved text shows for both; under unsaved text both are asked.
test('an agent’s write shows for both, and over unsaved text waits for Keep mine or Take disk version', async ({ app, browser, screen }) => {
  await openHere(app, browser, screen);
  await bobOpens(browser);
  const write = (text: string) => inWindow(browser, 'me', `mock.repoFile('sb_studio', '${FILE}', ${JSON.stringify(text)})`);
  // Saved text: the disk's version for both, within a few looks.
  await write('zero\nalpha\nbeta\ngamma\n');
  await expect.poll(() => textOf(browser, 'me'), { timeout: 10_000 }).toBe('zero\nalpha\nbeta\ngamma\n');
  await expect.poll(() => textOf(browser, 'bob'), { timeout: 10_000 }).toBe('zero\nalpha\nbeta\ngamma\n');

  // Unsaved text (its save held): both see the conflict, the text stays, and nothing is written.
  await inWindow(browser, 'me', 'mock.live.holdSaves = true');
  await typeAt(browser, 'me', 1, 1, 'mine ');
  await expect.poll(() => textOf(browser, 'bob')).toBe('mine zero\nalpha\nbeta\ngamma\n');
  await write('theirs\n');
  await expect.poll(() => statusOf(browser, 'me'), { timeout: 10_000 }).toContain('Changed on disk');
  await expect.poll(() => statusOf(browser, 'bob')).toContain('Changed on disk');
  await inWindow(browser, 'me', 'mock.releaseSaves()');
  await expect.poll(() => inWindow<number>(browser, 'me', 'return mock.live.holdSaves ? -1 : mock.live.saveAsks.length')).toBeGreaterThan(0);
  expect(await onDisk(browser)).toBe('theirs\n');
  expect(await textOf(browser, 'bob')).toBe('mine zero\nalpha\nbeta\ngamma\n');

  // Keep mine (here): the shared text over the disk, saved for both.
  await screen.getByRole('button', 'Keep mine').click();
  await expect.poll(() => onDisk(browser)).toBe('mine zero\nalpha\nbeta\ngamma\n');
  await expect.poll(() => statusOf(browser, 'bob')).not.toContain('Changed on disk');
  await expect.poll(() => statusOf(browser, 'bob')).toContain('Saved');

  // Again; Take disk version (Bob): the disk's text for both.
  await inWindow(browser, 'me', 'mock.live.holdSaves = true');
  await typeAt(browser, 'bob', 1, 1, 'his ');
  await expect.poll(() => textOf(browser, 'me')).toBe('his mine zero\nalpha\nbeta\ngamma\n');
  await write('disk wins\n');
  await expect.poll(() => statusOf(browser, 'bob'), { timeout: 10_000 }).toContain('Changed on disk');
  await clickIn(browser, 'bob', '.ed-take-disk');
  await expect.poll(() => textOf(browser, 'me')).toBe('disk wins\n');
  await expect.poll(() => textOf(browser, 'bob')).toBe('disk wins\n');
  await expect.poll(() => statusOf(browser, 'me')).toContain('Saved');
  await inWindow(browser, 'me', 'mock.releaseSaves()');
  expect(await onDisk(browser)).toBe('disk wins\n');
  await browser.evaluate(() => (window as unknown as Win).__bob.close());
});

// AC-411-7: lost events and a document the server no longer has.
test('lost events are caught up and a lost document is started again from the person’s text', async ({ app, browser, screen }) => {
  await openHere(app, browser, screen);
  const doc = () => inWindow<{ session: string; revision: number; text: string } | null>(browser, 'me', `return mock.liveDoc('${FILE}')`);
  const teammateTypes = async (text: string) => {
    const now = await doc();
    const length = now?.text.length ?? 0;
    return inWindow(browser, 'me', `return mock.request('POST', '/api/sandboxes/sb_studio/files/live/edit', { path: '${FILE}', session: '${now?.session}', client: 'teammate', revision: ${now?.revision}, op: [${length}, ${JSON.stringify(text)}] })`);
  };
  // The stream loses two steps: the next one shows the gap, and both are caught up.
  await inWindow(browser, 'me', 'mock.liveMuted = true');
  await teammateTypes('one\n');
  await teammateTypes('two\n');
  await inWindow(browser, 'me', 'mock.liveMuted = false');
  await teammateTypes('three\n');
  await expect.poll(() => textOf(browser, 'me')).toBe(`${ON_DISK}one\ntwo\nthree\n`);
  // A `resync` (the stream reconnected) catches up what was lost without a later step.
  await inWindow(browser, 'me', 'mock.liveMuted = true');
  await teammateTypes('four\n');
  await inWindow(browser, 'me', 'mock.liveMuted = false; mock.resyncNow()');
  await expect.poll(() => textOf(browser, 'me')).toBe(`${ON_DISK}one\ntwo\nthree\nfour\n`);

  // The server forgets the document (a restart): the next keystroke starts it again from this text.
  await inWindow(browser, 'me', 'mock.live.holdSaves = true; mock.dropLive()');
  await typeAt(browser, 'me', 1, 1, 'kept ');
  const expected = `kept ${ON_DISK}one\ntwo\nthree\nfour\n`;
  await expect.poll(async () => (await doc())?.text ?? null).toBe(expected);
  expect(await textOf(browser, 'me')).toBe(expected);
  await inWindow(browser, 'me', 'mock.releaseSaves()');
  await expect.poll(() => onDisk(browser)).toBe(expected);
});

// Review round 1 (M2): a file closed while its typing is still on its way keeps its document
// until the typing is confirmed and written.
test('a file closed right after typing still saves all of it', async ({ app, browser, screen }) => {
  await openHere(app, browser, screen);
  await inWindow(browser, 'me', 'mock.live.holdEdits = true');
  await typeAt(browser, 'me', 1, 1, 'X');
  await typeAt(browser, 'me', 1, 2, 'Y');
  await screen.getByRole('tab', /^notes\.txt/).getByText('●').click();
  await expect(screen.getByRole('tab', /^notes\.txt/)).toHaveCount(0);
  await inWindow(browser, 'me', 'mock.releaseEdits()');
  await expect.poll(() => onDisk(browser)).toBe(`XY${ON_DISK}`);
});

// Review round 1 (minor 4): a document the server lost and someone else started again keeps the
// typing this tab had not had confirmed, on top of the text it now has.
test('a document started again by someone else keeps the person’s unconfirmed typing on top', async ({ app, browser, screen }) => {
  await openHere(app, browser, screen);
  await inWindow(browser, 'me', 'mock.live.holdSaves = true; mock.live.holdEdits = true');
  await typeAt(browser, 'me', 1, 1, 'kept ');
  // The server restarts; a teammate's Editor starts the document again from its own text.
  await inWindow(browser, 'me', `mock.dropLive(); return mock.request('POST', '/api/sandboxes/sb_studio/files/live', { path: '${FILE}', client: 'teammate', text: 'alpha\\nbeta\\ngamma\\ndelta\\n', saved: false })`);
  await inWindow(browser, 'me', 'mock.releaseEdits()');
  const expected = 'kept alpha\nbeta\ngamma\ndelta\n';
  await expect.poll(() => inWindow<string | null>(browser, 'me', `return mock.liveDoc('${FILE}')?.text ?? null`)).toBe(expected);
  await expect.poll(() => textOf(browser, 'me')).toBe(expected);
});

// Review round 1 (minor 7): a version on disk the server refuses to take is asked for once.
test('a disk version that is no longer text is asked for once, and says so', async ({ app, browser, screen }) => {
  await openHere(app, browser, screen);
  await inWindow(browser, 'me', `mock.repoFile('sb_studio', '${FILE}', 'grown'); mock.repoFileTooLarge('sb_studio', '${FILE}')`);
  await expect(browser.locator('.toast-msg:has-text("no longer text the editor opens")')).toBeVisible();
  const reads = () => inWindow<number>(browser, 'me', `return (mock.fileReads ?? []).filter((r) => r.path === '${FILE}').length`);
  const after = await reads();
  await expect.poll(reads, { timeout: 10_000 }).toBeGreaterThanOrEqual(after + 2);
  expect(await inWindow<number>(browser, 'me', `return mock.live.diskAsks.filter((a) => a.path === '${FILE}').length`)).toBe(1);
  expect(await textOf(browser, 'me')).toBe(ON_DISK);
});

// EDT-016: the browser's operations converge as the server's do (the mock uses them as the server).
test('text operations converge in either order, compose, and place a caret through them', async ({ app, browser }) => {
  await app.open(mock('/s/studio'));
  const failures = await browser.evaluate(async () => {
    type Op = (number | string)[];
    const m = await import('/assets/livetext.js' as string) as {
      builder(): { ops: Op; retain(n: number): unknown; insert(t: string): unknown; delete(n: number): unknown };
      apply(t: string, op: Op): string; transform(a: Op, b: Op): [Op, Op]; compose(a: Op, b: Op): Op;
      transformIndex(i: number, op: Op): number; fromChanges(c: { rangeOffset: number; rangeLength: number; text: string }[], length: number): Op;
    };
    let seed = 411;
    const rnd = (n: number) => { seed = (seed * 1103515245 + 12345) % 2147483648; return seed % Math.max(1, n); };
    const pieces = ['a', 'bc', '\n', 'é', 'xyz'];
    const text = () => Array.from({ length: rnd(8) }, () => pieces[rnd(pieces.length)]).join('');
    const op = (t: string) => {
      const b = m.builder();
      let at = 0;
      while (at < t.length) {
        const n = Math.min(t.length, at + 1 + rnd(3));
        const k = rnd(3);
        if (k === 0) b.retain(n - at); else if (k === 1) b.delete(n - at); else { b.insert(pieces[rnd(pieces.length)]); b.retain(n - at); }
        at = n;
      }
      if (rnd(2)) b.insert(pieces[rnd(pieces.length)]);
      return b.ops;
    };
    const out: string[] = [];
    for (let i = 0; i < 2000 && out.length < 3; i += 1) {
      const s = text();
      const a = op(s);
      const b = op(s);
      const [a2, b2] = m.transform(a, b);
      if (m.apply(m.apply(s, a), b2) !== m.apply(m.apply(s, b), a2)) out.push(`transform ${JSON.stringify({ s, a, b })}`);
      const c = op(m.apply(s, a));
      if (m.apply(s, m.compose(a, c)) !== m.apply(m.apply(s, a), c)) out.push(`compose ${JSON.stringify({ s, a, c })}`);
    }
    // At one place, the first operation's insert comes first, as on the server.
    const [x2] = m.transform([1, 'X', 1], [1, 'Y', 1]);
    if (m.apply(m.apply('ab', [1, 'Y', 1]), x2) !== 'aXYb') out.push('insert order');
    if (m.transformIndex(3, [1, 'XY', 4]) !== 5 || m.transformIndex(3, [1, -2, 2]) !== 1) out.push('transformIndex');
    if (JSON.stringify(m.fromChanges([{ rangeOffset: 4, rangeLength: 1, text: '' }, { rangeOffset: 1, rangeLength: 0, text: 'Q' }], 6)) !== JSON.stringify([1, 'Q', 3, -1, 1])) out.push('fromChanges');
    return out;
  });
  expect(failures).toEqual([]);
});
