import { test, type Browser } from '@e2e-dev/web';
import { expect } from 'e2e';
import { axeViolations, failOnPageErrors, mock, mockReady } from './support.ts';

failOnPageErrors();

test('a file from the tree opens in an editor tab', async ({ app, screen }) => {
  await app.open(mock('/s/studio/t/12'));
  await screen.getByRole('button', /^Show tools/).click();
  await screen.getByRole('button', 'Editor').click();

  const tree = screen.getByRole('tree', 'Files');
  await tree.getByRole('treeitem', 'README.md').click();

  await expect(screen.getByRole('tab', /^README\.md/)).toHaveAttribute('aria-selected', 'true');
  await expect(screen.getByRole('textbox', 'Editor content')).toBeAttached();

  const folder = tree.getByRole('treeitem', /src$/);
  await expect(folder).toHaveAttribute('aria-expanded', 'false');
  await folder.click();
  await expect(folder).toHaveAttribute('aria-expanded', 'true');
});

type WithMonaco = Window & { monaco: { editor: { getEditors(): { getRawOptions(): { wordWrap?: string } }[] } } };

/** The open editor's word wrap as Monaco has it ('on' or 'off'). */
const monacoWordWrap = (browser: Browser) =>
  browser.evaluate(() => (window as unknown as WithMonaco).monaco.editor.getEditors()[0].getRawOptions().wordWrap ?? 'off');

// F 2026-10-09: "In editor I should be able to select whether to break lines or not."
test('word wrap turns on and off from the toolbar and Alt+Z, and stays after a reload', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/t/12'));
  await screen.getByRole('button', /^Show tools/).click();
  await screen.getByRole('button', 'Editor').click();
  await screen.getByRole('tree', 'Files').getByRole('treeitem', 'README.md').click();
  const content = screen.getByRole('textbox', 'Editor content');
  await expect(content).toBeAttached();

  const wrap = screen.getByRole('button', 'Word wrap');
  await expect(wrap).toHaveAttribute('aria-pressed', 'false');
  await expect.poll(() => monacoWordWrap(browser)).toBe('off');

  await wrap.click();
  await expect(wrap).toHaveAttribute('aria-pressed', 'true');
  await expect.poll(() => monacoWordWrap(browser)).toBe('on');

  // As in VS Code: Alt+Z (⌥Z) in the editor.
  await content.focus();
  await browser.keyboard.press('Alt+z');
  await expect(wrap).toHaveAttribute('aria-pressed', 'false');
  await expect.poll(() => monacoWordWrap(browser)).toBe('off');
  await browser.keyboard.press('Alt+z');
  await expect(wrap).toHaveAttribute('aria-pressed', 'true');
  await expect.poll(() => monacoWordWrap(browser)).toBe('on');
  // The shortcut toggles; it never types into the file (a typed character would mark it modified).
  await expect(screen.getByRole('tab', 'README.md')).toHaveAttribute('aria-selected', 'true');
  // The editor with its toolbar passes WCAG A/AA (explorer rows are 24 px targets).
  expect(await axeViolations(browser)).toEqual([]);

  // Kept in this browser: after a reload the editor opens wrapped.
  await browser.reload();
  await screen.getByRole('button', 'Editor').click();
  await expect(screen.getByRole('button', 'Word wrap')).toHaveAttribute('aria-pressed', 'true');
  await screen.getByRole('tree', 'Files').getByRole('treeitem', 'README.md').click();
  await expect(screen.getByRole('textbox', 'Editor content')).toBeAttached();
  await expect.poll(() => monacoWordWrap(browser)).toBe('on');
});

// #325: Monaco on Chromium types through an EditContext on a plain div (not a textarea), and a
// task terminal that shows late must leave the keyboard there too.
test('the task terminal showing late does not take the keys typed into the Editor', async ({ app, browser, screen }) => {
  type Hold = { __yuliusHoldTerminal?: Promise<void>; __releaseTerminal?: () => void };
  await browser.addInitScript(() => {
    const w = window as unknown as Hold;
    w.__yuliusHoldTerminal = new Promise<void>((resolve) => { w.__releaseTerminal = resolve; });
  });
  await app.open(mock('/s/studio/t/12'));
  await screen.getByRole('button', /^Show tools/).click();
  await screen.getByRole('button', 'Editor').click();
  await screen.getByRole('tree', 'Files').getByRole('treeitem', 'README.md').click();
  const content = screen.getByRole('textbox', 'Editor content');
  await expect(content).toBeAttached();
  // The file has loaded: keys typed before it arrives are replaced by its content (SPEC-082
  // TST-003; before, the test typed into the empty model about one run in three).
  const text = () => browser.evaluate(() => (window as unknown as { monaco: { editor: { getEditors(): { getValue(): string }[] } } }).monaco.editor.getEditors()[0]?.getValue() ?? '');
  await expect.poll(text).toMatch(/^# studio/);
  await content.focus();
  await browser.keyboard.press('Control+Home');
  await browser.keyboard.type('abc');
  await browser.evaluate(() => (window as unknown as Hold).__releaseTerminal?.());
  await expect.poll(() => browser.evaluate(() => document.querySelector('.taskview .term-pending') === null)).toBe(true);
  await browser.keyboard.type('def');
  await expect.poll(text).toMatch(/^abcdef# studio/);
  expect(await browser.evaluate(() => document.activeElement?.closest('.monaco-editor') != null)).toBe(true);
});

// ---- SPEC-127: the notepad's smooth selection and teammates' presence, in the Editor.

type Ed = {
  setSelection(range: { startLineNumber: number; startColumn: number; endLineNumber: number; endColumn: number }): void;
  setScrollTop(top: number): void;
  revealLine(line: number): void;
  focus(): void;
  getModel(): { getValue(): string; getOffsetAt(position: { lineNumber: number; column: number }): number } | null;
  executeEdits(source: string, edits: { range: unknown; text: string }[]): void;
};
type EdWindow = Window & { monaco: { editor: { getEditors(): Ed[] }; Range: new (a: number, b: number, c: number, d: number) => unknown } };

/** A file of 200 lines, the fifth long enough to wrap. */
const LONG = Array.from({ length: 200 }, (_, i) => (i === 4 ? `long ${'word '.repeat(120)}end` : `line ${i + 1} of the long file`)).join('\n');

/** Opens the Editor with `long.md` (set in the mock repository first). */
async function openLong(app: { open(url: string): Promise<void> }, browser: Browser, screen: import('@e2e-dev/web').Screen, options = '') {
  await app.open(mock('/s/studio/t/12', options));
  await mockReady(browser);
  await browser.evaluate((text) => (window as unknown as { __yuliusMock: { repoFile(s: string, p: string, t: string): void } }).__yuliusMock.repoFile('sb_studio', 'long.md', text), LONG);
  await screen.getByRole('button', /^Show tools/).click();
  await screen.getByRole('button', 'Editor').click();
  await screen.getByRole('tree', 'Files').getByRole('treeitem', 'long.md').click();
  await expect(screen.getByRole('tab', /^long\.md/)).toHaveAttribute('aria-selected', 'true');
  await expect.poll(() => browser.evaluate(() => (window as unknown as EdWindow).monaco.editor.getEditors()[0].getModel()?.getValue().length ?? 0)).toBe(LONG.length);
}

const select = (browser: Browser, range: [number, number, number, number]) =>
  browser.evaluate(([a, b, c, d]) => {
    const ed = (window as unknown as EdWindow).monaco.editor.getEditors()[0];
    ed.focus();
    ed.setSelection({ startLineNumber: a, startColumn: b, endLineNumber: c, endColumn: d });
  }, range);

/**
 * The person's selection as Monaco lays it out (its boxes, see-through) and as drawn (the shapes
 * shown), in whole pixels of the window, top to bottom.
 */
const ownSelection = (browser: Browser) => browser.evaluate(() => {
  const root = document.querySelector('.ed-host .monaco-editor');
  const rect = (el: Element) => { const r = el.getBoundingClientRect(); return [Math.round(r.left), Math.round(r.top), Math.round(r.width), Math.round(r.height)]; };
  const order = (a: number[], b: number[]) => a[1] - b[1] || a[0] - b[0];
  const native = [...(root?.querySelectorAll('.view-overlays .selected-text') ?? [])];
  const shapes = [...(root?.querySelectorAll('.ed-layer .tl-own .tl-shape:not(.tl-gone)') ?? [])];
  return {
    monaco: native.map(rect).filter((r) => r[2] > 0).sort(order),
    drawn: shapes.map(rect).sort(order),
    nativePainted: native.some((el) => getComputedStyle(el).backgroundColor !== 'rgba(0, 0, 0, 0)'),
    radius: shapes[0] ? getComputedStyle(shapes[0]).borderRadius : '',
    transition: shapes[0] ? getComputedStyle(shapes[0]).transitionProperty : '',
    blurred: Boolean(root?.querySelector('.ed-layer .tl-own.tl-blurred')),
  };
});

/** Waits until the drawn shapes lie exactly on Monaco's boxes; answers how many there are. */
async function drawnOnMonaco(browser: Browser): Promise<number> {
  await expect.poll(async () => { const s = await ownSelection(browser); return s.monaco.length > 0 && JSON.stringify(s.drawn) === JSON.stringify(s.monaco); }).toBe(true);
  return (await ownSelection(browser)).monaco.length;
}

test('a selection in the editor is drawn rounded where Monaco lays it out, gliding, wrapped or not, scrolled', async ({ app, browser, screen }) => {
  await openLong(app, browser, screen);

  // Three lines: three rounded shapes on Monaco's three boxes, which are not painted themselves.
  await select(browser, [1, 3, 3, 5]);
  expect(await drawnOnMonaco(browser)).toBe(3);
  let seen = await ownSelection(browser);
  expect(seen.nativePainted).toBe(false);
  expect(seen.radius).toBe('4px');
  expect(seen.transition).toContain('transform');
  expect(seen.blurred).toBe(false);

  // It moves: the shapes glide to the new boxes.
  await select(browser, [2, 1, 6, 3]);
  expect(await drawnOnMonaco(browser)).toBe(5);

  // Word wrap off: the long line is one row; on: it is several, each its own shape.
  await select(browser, [5, 1, 5, 400]);
  expect(await drawnOnMonaco(browser)).toBe(1);
  await screen.getByRole('button', 'Word wrap').click();
  await expect(screen.getByRole('button', 'Word wrap')).toHaveAttribute('aria-pressed', 'true');
  await select(browser, [5, 1, 5, 400]);
  expect(await drawnOnMonaco(browser)).toBeGreaterThan(1);
  await screen.getByRole('button', 'Word wrap').click();
  await expect(screen.getByRole('button', 'Word wrap')).toHaveAttribute('aria-pressed', 'false');

  // Scrolled: a selection far down is drawn on its lines, and stays on them as the view scrolls.
  await select(browser, [120, 2, 140, 4]);
  await browser.evaluate(() => (window as unknown as EdWindow).monaco.editor.getEditors()[0].revealLine(130));
  expect(await drawnOnMonaco(browser)).toBeGreaterThan(5);
  await browser.evaluate(() => { const ed = (window as unknown as EdWindow).monaco.editor.getEditors()[0]; ed.setScrollTop(ed.getModel() ? 2600 : 0); });
  await drawnOnMonaco(browser);
  await browser.evaluate(() => (window as unknown as EdWindow).monaco.editor.getEditors()[0].setScrollTop(2200));
  await drawnOnMonaco(browser);

  // Without the keyboard the selection is softer.
  await screen.getByRole('tree', 'Files').getByRole('treeitem', 'long.md').focus();
  await expect.poll(async () => (await ownSelection(browser)).blurred).toBe(true);
  await drawnOnMonaco(browser);
});

/** The person's own page, or the teammate's window opened from it (`window.__bob`). */
type Who = 'me' | 'bob';
type Peer = { name: string; colour: string; expected: string; shapes: number; caret: boolean; start: string; end: string; head: string };

/** The teammates drawn in `who`'s editor, with their colour and the colour Notes gives them. */
const peersIn = (browser: Browser, who: Who) => browser.evaluate(async (whose) => {
  const w = (whose === 'bob' ? (window as unknown as { __bob: Window }).__bob : window) as Window & typeof globalThis;
  const { peerColour } = await import('/assets/textlayer.js') as { peerColour(id: string): string };
  const groups = [...w.document.querySelectorAll('.ed-host .ed-layer .tl-peer:not(.tl-leaving)')] as HTMLElement[];
  return groups.map((group) => {
    const caret = group.querySelector('.tl-caret') as HTMLElement;
    const probe = w.document.createElement('span');
    const userId = group.dataset.user ?? '';
    probe.style.color = peerColour(userId);
    w.document.body.append(probe);
    const expected = w.getComputedStyle(probe).color;
    probe.remove();
    return {
      name: group.querySelector('.tl-name')?.textContent ?? '',
      colour: w.getComputedStyle(caret).backgroundColor,
      expected,
      shapes: group.querySelectorAll('.tl-shape:not(.tl-gone)').length,
      caret: !caret.classList.contains('tl-off'),
      start: group.dataset.start ?? '',
      end: group.dataset.end ?? '',
      head: group.dataset.head ?? '',
    };
  });
}, who) as Promise<Peer[]>;

/** Runs `fn` with the editor of `who`'s window. */
const inEditor = <T extends import('@e2e-dev/web').JsonValue>(browser: Browser, who: Who, fn: string) =>
  browser.evaluate(([whose, body]) => {
    const w = (whose === 'bob' ? (window as unknown as { __bob: Window }).__bob : window) as unknown as EdWindow;
    const ed = w.monaco.editor.getEditors()[0];
    return new Function('ed', body)(ed);
  }, [who, fn]) as Promise<T>;

/** Clicks the first element matching `selector` in Bob's window, once it is there. */
async function bobClick(browser: Browser, selector: string, text?: string) {
  await expect.poll(() => browser.evaluate(([css, label]) => {
    const doc = (window as unknown as { __bob: Window }).__bob.document;
    const el = [...doc.querySelectorAll(css)].find((node) => !label || (node.textContent ?? '').trim() === label || (node.getAttribute('aria-label') ?? '').startsWith(label)) as HTMLElement | undefined;
    if (!el) return false;
    el.click();
    return true;
  }, [selector, text ?? '']), { message: `click ${selector} ${text ?? ''}` }).toBe(true);
}

/** Bob's editor shows `name` (opened from the tree, in his own signed-in window). */
async function bobOpens(browser: Browser, name: string) {
  await bobClick(browser, '.ed-tree [role="treeitem"]', name);
  await expect.poll(() => browser.evaluate((file) => {
    const w = (window as unknown as { __bob: EdWindow & Window }).__bob;
    return w.document.querySelector('.ed-tab.active')?.getAttribute('data-path') === file && Boolean(w.monaco?.editor.getEditors()[0]?.getModel()?.getValue());
  }, name)).toBe(true);
}

// SPEC-127 AC-127-1: two signed-in people (this page as mkarp, a second window as bob, the mock
// servers relaying to each other like the real one) see each other in the same file.
test('two people in one file see each other’s caret, name and selection in their colour, until one leaves', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/t/12'));
  await screen.getByRole('button', /^Show tools/).click();
  await screen.getByRole('button', 'Editor').click();
  await screen.getByRole('tree', 'Files').getByRole('treeitem', 'README.md').click();
  await expect.poll(() => inEditor<number>(browser, 'me', 'return ed.getModel()?.getValue().length ?? 0')).toBeGreaterThan(0);

  // Bob signs in in his own window and opens the same file.
  await browser.evaluate(() => { (window as unknown as { __bob: Window | null }).__bob = window.open(`${location.origin}/?mock=1&as=bob#/s/studio/t/12`, 'bob', 'width=1280,height=900'); });
  // The tools rail is shown (the person's choice, kept in this browser) or Bob shows it.
  await expect.poll(() => browser.evaluate(() => {
    const doc = (window as unknown as { __bob: Window }).__bob.document;
    const buttons = [...doc.querySelectorAll('button')];
    const show = buttons.find((b) => (b.getAttribute('aria-label') ?? '').startsWith('Show tools'));
    if (show) (show as HTMLElement).click();
    return Boolean(show) || buttons.some((b) => (b.getAttribute('aria-label') ?? '').startsWith('Hide tools'));
  })).toBe(true);
  await bobClick(browser, '.rail button', 'Editor');
  await bobOpens(browser, 'README.md');

  // Each selects; each sees the other: their name, a caret and a selection in their colour.
  await inEditor(browser, 'me', 'ed.focus(); ed.setSelection({ startLineNumber: 1, startColumn: 3, endLineNumber: 1, endColumn: 8 })');
  await expect.poll(() => peersIn(browser, 'bob')).toEqual([expect.objectContaining({ name: 'mkarp', shapes: 1, caret: true, start: '2', end: '7', head: '7' })]);
  const mine = (await peersIn(browser, 'bob'))[0];
  expect(mine.colour).toBe(mine.expected);
  await inEditor(browser, 'bob', 'ed.focus(); ed.setSelection({ startLineNumber: 3, startColumn: 9, endLineNumber: 3, endColumn: 4 })');
  await expect.poll(() => peersIn(browser, 'me')).toEqual([expect.objectContaining({ name: 'bob', shapes: 1, caret: true })]);
  const bobs = (await peersIn(browser, 'me'))[0];
  expect(bobs.colour).toBe(bobs.expected);
  expect(bobs.colour).not.toBe(mine.colour);
  expect(Number(bobs.head)).toBeLessThan(Number(bobs.end)); // selected backwards: the caret at its start

  // Live: Bob moves, the person sees it move.
  await inEditor(browser, 'bob', 'ed.setSelection({ startLineNumber: 1, startColumn: 1, endLineNumber: 1, endColumn: 1 })');
  await expect.poll(() => peersIn(browser, 'me')).toEqual([expect.objectContaining({ name: 'bob', shapes: 0, caret: true, head: '0' })]);

  // Bob switches to another file: gone here; back: there again.
  await bobOpens(browser, 'AGENTS.md');
  await expect.poll(() => peersIn(browser, 'me')).toEqual([]);
  await bobClick(browser, '.ed-tab[data-path="README.md"]');
  await expect.poll(() => peersIn(browser, 'me')).toEqual([expect.objectContaining({ name: 'bob' })]);
  // …and Bob sees the person again, answered to his hello.
  await expect.poll(() => peersIn(browser, 'bob')).toEqual([expect.objectContaining({ name: 'mkarp', start: '2', end: '7' })]);

  // Bob closes the file: gone.
  await bobClick(browser, '.ed-tab.active .ed-tab-close');
  await expect.poll(() => peersIn(browser, 'me')).toEqual([]);

  // Bob opens it again, then closes the Editor: gone.
  await bobOpens(browser, 'README.md');
  await expect.poll(() => peersIn(browser, 'me')).toEqual([expect.objectContaining({ name: 'bob' })]);
  await bobClick(browser, '.rail button', 'Editor');
  await expect.poll(() => peersIn(browser, 'me')).toEqual([]);

  // The person leaves the page (Bob's Editor shown again first): gone for Bob.
  await bobClick(browser, '.rail button', 'Editor');
  await expect.poll(() => peersIn(browser, 'bob')).toEqual([expect.objectContaining({ name: 'mkarp' })]);
  await screen.getByRole('button', 'Editor').click();
  await expect.poll(() => peersIn(browser, 'bob')).toEqual([]);
  await browser.evaluate(() => (window as unknown as { __bob: Window }).__bob.close());
});

type FileMock = { __yuliusMock: { filePresence(event: Record<string, unknown>): void; filePresenceSent?: { path: string; sha256: string; selection: { anchor: number; head: number } | null; hello: boolean }[] } };
const teammateInFile = (browser: Browser, event: Record<string, unknown>) =>
  browser.evaluate((e) => (window as unknown as FileMock).__yuliusMock.filePresence(e), event);
const fileSent = (browser: Browser) => browser.evaluate(() => (window as unknown as FileMock).__yuliusMock.filePresenceSent ?? []);

test('a teammate in a file follows the person’s unsaved typing, hides at another saved version, and is answered', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/t/12'));
  await screen.getByRole('button', /^Show tools/).click();
  await screen.getByRole('button', 'Editor').click();
  await screen.getByRole('tree', 'Files').getByRole('treeitem', 'README.md').click();
  await expect.poll(() => inEditor<number>(browser, 'me', 'return ed.getModel()?.getValue().length ?? 0')).toBeGreaterThan(0);
  // Shown: a hello with the file's saved version.
  await expect.poll(async () => (await fileSent(browser)).some((u) => u.path === 'README.md' && u.hello && /^[0-9a-f]{64}$/.test(u.sha256))).toBe(true);
  const sha = (await fileSent(browser))[0].sha256;

  // Alice selects "studio" in the saved text ("# studio\n…"): drawn there, in her colour.
  const before = (await fileSent(browser)).length;
  await teammateInFile(browser, { path: 'README.md', user_id: 'usr_alice', name: 'alice', sha256: sha, selection: { anchor: 2, head: 8 }, hello: true });
  await expect.poll(() => peersIn(browser, 'me')).toEqual([expect.objectContaining({ name: 'alice', shapes: 1, caret: true, start: '2', end: '8', head: '8' })]);
  const alice = (await peersIn(browser, 'me'))[0];
  expect(alice.colour).toBe(alice.expected);
  // Her hello is answered with where the person is.
  await expect.poll(async () => (await fileSent(browser)).slice(before).some((u) => u.selection !== null && !u.hello)).toBe(true);

  // The person types before her selection (unsaved): it moves with the text.
  await inEditor(browser, 'me', "ed.executeEdits('t', [{ range: { startLineNumber: 1, startColumn: 1, endLineNumber: 1, endColumn: 1 }, text: '>> ' }])");
  await expect.poll(() => peersIn(browser, 'me')).toEqual([expect.objectContaining({ name: 'alice', start: '5', end: '11' })]);

  // Her offsets in another saved version: hidden until both have the same.
  await teammateInFile(browser, { path: 'README.md', user_id: 'usr_alice', name: 'alice', sha256: 'f'.repeat(64), selection: { anchor: 0, head: 1 } });
  await expect.poll(() => peersIn(browser, 'me')).toEqual([]);
  await teammateInFile(browser, { path: 'README.md', user_id: 'usr_alice', name: 'alice', sha256: sha, selection: { anchor: 0, head: 0 } });
  await expect.poll(() => peersIn(browser, 'me')).toEqual([expect.objectContaining({ name: 'alice', shapes: 0, head: '0' })]);

  // The person's own echo (another tab of theirs), another file and another folder draw nothing.
  await teammateInFile(browser, { path: 'README.md', user_id: 'usr_1', name: 'mkarp', client: 'other', sha256: sha, selection: { anchor: 1, head: 3 } });
  await teammateInFile(browser, { path: 'AGENTS.md', user_id: 'usr_bob', name: 'bob', sha256: sha, selection: { anchor: 1, head: 3 } });
  await teammateInFile(browser, { path: 'README.md', root: 'task:t_12', user_id: 'usr_carol', name: 'carol', sha256: sha, selection: { anchor: 1, head: 3 } });
  // A teammate shown passes WCAG A/AA (the name tag's contrast, the drawn marks hidden from it).
  expect(await axeViolations(browser)).toEqual([]);
  expect((await peersIn(browser, 'me')).map((p) => p.name)).toEqual(['alice']);

  // She leaves: gone.
  await teammateInFile(browser, { path: 'README.md', user_id: 'usr_alice', name: 'alice', sha256: sha, selection: null });
  await expect.poll(() => peersIn(browser, 'me')).toEqual([]);
});
