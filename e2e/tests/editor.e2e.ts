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
