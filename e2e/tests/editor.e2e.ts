import { test, type Browser } from '@e2e-dev/web';
import { expect } from 'e2e';
import { axeViolations, editorShown, failOnPageErrors, MONACO_LOAD_MS, mock, pageErrors } from './support.ts';

failOnPageErrors();

test('a file from the tree opens in an editor tab', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/t/12'));
  await screen.getByRole('button', /^Show tools/).click();
  await screen.getByRole('button', 'Editor').click();

  const tree = screen.getByRole('tree', 'Files');
  await tree.getByRole('treeitem', 'README.md').click();
  // The first file loads Monaco: a condition with the cold load's budget, not the tab's 5 s (#356).
  await editorShown(browser);

  await expect(screen.getByRole('tab', /^README\.md/)).toHaveAttribute('aria-selected', 'true');
  await expect(screen.getByRole('textbox', 'Editor content')).toBeAttached();

  const folder = tree.getByRole('treeitem', /src$/);
  await expect(folder).toHaveAttribute('aria-expanded', 'false');
  await folder.click();
  await expect(folder).toHaveAttribute('aria-expanded', 'true');
});

// #394: Monaco's main chunk sets `window.monaco` long before the chunk that registers five of its
// services (ICodeLensCache, IInlayHintsCache…) has run, and the first editor fixes the services
// every editor gets. A file opened while Monaco loads must wait for all of it, not create an
// editor from the half-loaded `window.monaco`.
test('a second file opened while the editor loads waits for all of Monaco: one editor, no errors', async ({ app, browser, screen }) => {
  let release = () => {};
  const held = new Promise<void>((resolve) => { release = resolve; });
  let holds = 0;
  // The chunk that registers the services (`grep -l ICodeLensCache vendor/monaco/vs/*.js`): if a
  // Monaco update renames it, `holds` stays 0 and this test says so instead of holding nothing.
  await browser.route(/\/vendor\/monaco\/vs\/toggleHighContrast-[^/]*\.js$/, async (route) => { holds += 1; await held; await route.continue(); });
  await app.open(mock('/s/studio/t/12'));
  await screen.getByRole('button', /^Show tools/).click();
  await screen.getByRole('button', 'Editor').click();
  const tree = screen.getByRole('tree', 'Files');
  await tree.getByRole('treeitem', 'README.md').click();
  // Monaco's main chunk has run (it sets `window.monaco`); its services chunk is held back.
  await expect.poll(() => browser.evaluate(() => 'monaco' in window), { timeout: MONACO_LOAD_MS }).toBe(true);
  expect(holds, 'the services chunk was held back').toBe(1);
  await tree.getByRole('treeitem', 'AGENTS.md').click();
  release();
  await editorShown(browser);
  await expect(screen.getByRole('tab', /^AGENTS\.md/)).toBeVisible();
  await expect(screen.getByRole('tab', /^README\.md/)).toBeVisible();
  expect(await browser.evaluate(() => document.querySelectorAll('.ed-host .monaco-editor').length)).toBe(1);
  // The code lens and inlay hint contributions are made when the browser is idle; asked for by id,
  // they are made at once, and throw without their services.
  expect(await browser.evaluate(() => {
    type Ed = { getContribution(id: string): unknown };
    const [ed] = (window as unknown as { monaco: { editor: { getEditors(): Ed[] } } }).monaco.editor.getEditors();
    return ['css.editor.codeLens', 'editor.contrib.InlayHints'].map((id) => {
      try { return ed.getContribution(id) ? 'made' : 'missing'; } catch (error) { return String(error); }
    });
  })).toEqual(['made', 'made']);
});

type HeldMock = Window & { __yuliusMock: { held: Promise<void>; release(): void; repoFile(sandbox: string, path: string, text: string, root?: string): void } };
const releaseReads = (browser: Browser) => browser.evaluate(() => (window as unknown as HeldMock).__yuliusMock.release());
/** Holds the file reads again (the page was opened with them held, then released). */
const holdReads = (browser: Browser) => browser.evaluate(() => {
  const mock = (window as unknown as HeldMock).__yuliusMock;
  mock.held = new Promise((resolve) => { mock.release = () => resolve(); });
});
const tabPaths = (browser: Browser) => browser.evaluate(() => [...document.querySelectorAll('.ed-tab')].map((tab) => (tab as HTMLElement).dataset.path));

// #534 (SPEC-166): a file opened again before its first open showed it (a double-click on its row)
// waits for that open: one tab, while Monaco loads and after. The file reads are held so both
// clicks land before either read ends; the next file's read starts after they are answered.
test('a file clicked twice before its tab shows opens one tab, while the editor loads and after', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/t/12', '&hold=/api/sandboxes/sb_studio/files/content'));
  await screen.getByRole('button', /^Show tools/).click();
  await screen.getByRole('button', 'Editor').click();
  const tree = screen.getByRole('tree', 'Files');
  await tree.getByRole('treeitem', 'AGENTS.md').dblclick();
  await releaseReads(browser);
  await editorShown(browser);
  await expect(screen.getByRole('tab', /^AGENTS\.md/)).toHaveAttribute('aria-selected', 'true');
  await tree.getByRole('treeitem', 'README.md').click();
  await expect(screen.getByRole('tab', /^README\.md/)).toHaveAttribute('aria-selected', 'true');
  expect(await tabPaths(browser)).toEqual(['AGENTS.md', 'README.md']);

  // Monaco loaded: a double-click is still two opens.
  await holdReads(browser);
  for (const folder of [/\bsrc$/, /\bapi$/]) {
    await tree.getByRole('treeitem', folder).click();
    await expect(tree.getByRole('treeitem', folder)).toHaveAttribute('aria-expanded', 'true');
  }
  await tree.getByRole('treeitem', 'checkout.rs').dblclick();
  await releaseReads(browser);
  await expect(screen.getByRole('tab', /^checkout\.rs/)).toHaveAttribute('aria-selected', 'true');
  for (const folder of [/\bdocs$/, /\bspecs$/]) {
    await tree.getByRole('treeitem', folder).click();
    await expect(tree.getByRole('treeitem', folder)).toHaveAttribute('aria-expanded', 'true');
  }
  await tree.getByRole('treeitem', '2026-10-10-improve-the-code.md').click();
  await expect(screen.getByRole('tab', /^2026-10-10-improve-the-code\.md/)).toHaveAttribute('aria-selected', 'true');
  expect(await tabPaths(browser)).toEqual(['AGENTS.md', 'README.md', 'src/api/checkout.rs', 'docs/specs/2026-10-10-improve-the-code.md']);
  expect(await browser.evaluate(() => document.querySelectorAll('.ed-host .monaco-editor').length)).toBe(1);
});

// SPEC-166 review: a path being opened in one folder is another file in the next folder. Its open,
// still on its way when the folder changes, neither shows nor stands in for the new folder's file.
test('a file opened while the folder switches shows the new folder’s file', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/t/12', '&hold=/api/sandboxes/sb_studio/files/content'));
  await screen.getByRole('button', /^Show tools/).click();
  await screen.getByRole('button', 'Editor').click();
  const tree = screen.getByRole('tree', 'Files');
  await tree.getByRole('treeitem', 'AGENTS.md').click();
  await releaseReads(browser);
  await editorShown(browser);
  await expect(screen.getByRole('tab', /^AGENTS\.md/)).toHaveAttribute('aria-selected', 'true');
  const task = await browser.evaluate(() => (document.querySelector('select[aria-label="Folder"] option[value^="task:"]') as HTMLOptionElement).value);
  await browser.evaluate((root) => (window as unknown as HeldMock).__yuliusMock.repoFile('sb_studio', 'README.md', '# The task worktree\n', root), task);

  await holdReads(browser);
  await tree.getByRole('treeitem', 'README.md').click();
  // The Folder select, as the person changes it (no unsaved files: no question asked).
  await browser.evaluate((root) => {
    const select = document.querySelector('select[aria-label="Folder"]') as HTMLSelectElement;
    select.value = root;
    select.dispatchEvent(new Event('change', { bubbles: true }));
  }, task);
  await expect(screen.getByRole('tab', /^AGENTS\.md/)).toHaveCount(0);
  await tree.getByRole('treeitem', 'README.md').click();
  await releaseReads(browser);
  await expect(screen.getByRole('tab', /^README\.md/)).toHaveAttribute('aria-selected', 'true');
  await expect.poll(() => browser.evaluate(() => (window as unknown as { monaco: { editor: { getEditors(): { getValue(): string }[] } } }).monaco.editor.getEditors()[0].getValue())).toBe('# The task worktree\n');
  // Its own text from the start: not the other folder's, reloaded as if changed on disk.
  expect(await browser.evaluate(() => document.body.innerText.includes('changed on disk and was reloaded'))).toBe(false);
  expect(await tabPaths(browser)).toEqual(['README.md']);
});

// Review of SPEC-148: one creation shared by every opening must not keep a failure: the next
// file opened makes the editor again, as before the creation was shared. Made once: a failure
// after Monaco made the editor (its setup threw) leaves no second editor behind.
for (const [where, when] of [['the editor could not be made', 'create'], ['the made editor could not be set up', 'setup']] as const) {
  test(`a file opened after ${where} makes it then, once`, async ({ app, browser, screen }) => {
    await browser.addInitScript((point) => {
      type Editor = { addCommand: (...args: unknown[]) => unknown };
      type Api = { editor: { create: (...args: unknown[]) => Editor; e2eOnce?: boolean } };
      let api: Api | undefined;
      let failed = false;
      const fail = () => { failed = true; throw new Error('the editor could not be made (on purpose)'); };
      // Monaco sets `window.monaco` (twice: its main chunk, then editor.main): its first editor fails.
      Object.defineProperty(window, 'monaco', { configurable: true, get: () => api, set: (next: Api) => {
        api = next;
        if (next.editor.e2eOnce) return;
        const create = next.editor.create;
        next.editor.create = (...args: unknown[]) => {
          if (!failed && point === 'create') fail();
          const made = create.apply(next.editor, args);
          if (!failed && point === 'setup') {
            const addCommand = made.addCommand;
            made.addCommand = () => { made.addCommand = addCommand; return fail(); };
          }
          return made;
        };
        next.editor.e2eOnce = true;
      } });
    }, when);
    await app.open(mock('/s/studio/t/12'));
    await screen.getByRole('button', /^Show tools/).click();
    await screen.getByRole('button', 'Editor').click();
    const readme = screen.getByRole('tree', 'Files').getByRole('treeitem', 'README.md');
    await readme.click();
    // The failure is the page's (an unhandled rejection of the click); expected here, then cleared.
    const errors = () => browser.evaluate(() => (window as unknown as { __e2eErrors: string[] }).__e2eErrors);
    await expect.poll(errors, { timeout: MONACO_LOAD_MS }).toEqual([expect.stringContaining('the editor could not be made (on purpose)')]);
    await browser.evaluate(() => { (window as unknown as { __e2eErrors: string[] }).__e2eErrors.splice(0); });
    await readme.click();
    await editorShown(browser);
    await expect(screen.getByRole('tab', /^README\.md/)).toHaveAttribute('aria-selected', 'true');
    expect(await browser.evaluate(() => document.querySelectorAll('.ed-host .monaco-editor').length)).toBe(1);
  });
}

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
  await editorShown(browser);
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
  await editorShown(browser);
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
  await editorShown(browser);
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
  await browser.evaluate((text) => (window as unknown as { __yuliusMock: { repoFile(s: string, p: string, t: string): void } }).__yuliusMock.repoFile('sb_studio', 'long.md', text), LONG);
  await screen.getByRole('button', /^Show tools/).click();
  await screen.getByRole('button', 'Editor').click();
  await screen.getByRole('tree', 'Files').getByRole('treeitem', 'long.md').click();
  await editorShown(browser);
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

/**
 * Waits until Monaco shows `boxes` boxes (a count, or a test of it) and the drawn shapes lie
 * exactly on them: a new selection is rendered in Monaco's next frame, so the old, equal drawing
 * must not pass for it.
 */
async function drawnOnMonaco(browser: Browser, boxes: number | ((n: number) => boolean)) {
  const wanted = typeof boxes === 'number' ? (n: number) => n === boxes : boxes;
  await expect.poll(async () => { const s = await ownSelection(browser); return wanted(s.monaco.length) && JSON.stringify(s.drawn) === JSON.stringify(s.monaco); }).toBe(true);
}

test('a selection in the editor is drawn rounded where Monaco lays it out, gliding, wrapped or not, scrolled', async ({ app, browser, screen }) => {
  await openLong(app, browser, screen);

  // Three lines: three rounded shapes on Monaco's three boxes, which are not painted themselves.
  await select(browser, [1, 3, 3, 5]);
  await drawnOnMonaco(browser, 3);
  let seen = await ownSelection(browser);
  expect(seen.nativePainted).toBe(false);
  expect(seen.radius).toBe('4px');
  expect(seen.transition).toContain('transform');
  expect(seen.blurred).toBe(false);

  // It moves: the shapes glide to the new boxes.
  await select(browser, [2, 1, 6, 3]);
  await drawnOnMonaco(browser, 5);

  // Word wrap off: the long line is one row; on: it is several, each its own shape.
  await select(browser, [5, 1, 5, 400]);
  await drawnOnMonaco(browser, 1);
  await screen.getByRole('button', 'Word wrap').click();
  await expect(screen.getByRole('button', 'Word wrap')).toHaveAttribute('aria-pressed', 'true');
  await select(browser, [5, 1, 5, 400]);
  await drawnOnMonaco(browser, (n) => n > 1);
  await screen.getByRole('button', 'Word wrap').click();
  await expect(screen.getByRole('button', 'Word wrap')).toHaveAttribute('aria-pressed', 'false');

  // Scrolled: a selection far down is drawn on its lines, and stays on them as the view scrolls.
  await select(browser, [120, 2, 140, 4]);
  await browser.evaluate(() => (window as unknown as EdWindow).monaco.editor.getEditors()[0].revealLine(130));
  await drawnOnMonaco(browser, (n) => n > 5);
  await browser.evaluate(() => (window as unknown as EdWindow).monaco.editor.getEditors()[0].setScrollTop(2600));
  await drawnOnMonaco(browser, (n) => n > 0);
  await browser.evaluate(() => (window as unknown as EdWindow).monaco.editor.getEditors()[0].setScrollTop(2200));
  await drawnOnMonaco(browser, (n) => n > 0);

  // Without the keyboard the selection is softer.
  await screen.getByRole('tree', 'Files').getByRole('treeitem', 'long.md').focus();
  await expect.poll(async () => (await ownSelection(browser)).blurred).toBe(true);
  await drawnOnMonaco(browser, (n) => n > 0);
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

/**
 * Bob's editor shows `name` (opened from the tree, in his own signed-in window). His first opening
 * (`cold`) loads Monaco in his window and gets its budget; later ones the default.
 */
async function bobOpens(browser: Browser, name: string, { cold = false } = {}) {
  await bobClick(browser, '.ed-tree [role="treeitem"]', name);
  await expect.poll(() => browser.evaluate((file) => {
    const w = (window as unknown as { __bob: EdWindow & Window }).__bob;
    // Monaco's API only once his editor shows (`editorShown`, #394); his first opening loads it.
    return w.document.querySelector('.ed-tab.active')?.getAttribute('data-path') === file
      && w.document.querySelector('.ed-host .monaco-editor') !== null
      && Boolean(w.monaco.editor.getEditors()[0]?.getModel()?.getValue());
  }, name), cold ? { timeout: MONACO_LOAD_MS } : {}).toBe(true);
}

// SPEC-127 AC-127-1: two signed-in people (this page as mkarp, a second window as bob, the mock
// servers relaying to each other like the real one) see each other in the same file.
test('two people in one file see each other’s caret, name and selection in their colour, until one leaves', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/t/12'));
  await screen.getByRole('button', /^Show tools/).click();
  await screen.getByRole('button', 'Editor').click();
  await screen.getByRole('tree', 'Files').getByRole('treeitem', 'README.md').click();
  await editorShown(browser);
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
  await bobOpens(browser, 'README.md', { cold: true });

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

  // The person closes the Editor (Bob's shown again first): gone for Bob.
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
  await editorShown(browser);
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

/** The Editor's open files, in the strip's order. */
const editorTabs = (browser: Browser) => browser.evaluate(() => [...document.querySelectorAll('.ed-tabs [role="tab"]')].map((tab) => (tab as HTMLElement).dataset.path ?? ''));

// SPEC-127 EDT-011, AC-127-2: the open files' tabs move by drag, keyboard and menu.
test('the editor’s file tabs move by drag, Alt+arrows and the menu', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/t/12'));
  await screen.getByRole('button', /^Show tools/).click();
  await screen.getByRole('button', 'Editor').click();
  const tree = screen.getByRole('tree', 'Files');
  for (const name of ['README.md', 'AGENTS.md']) {
    await tree.getByRole('treeitem', name).click();
    await editorShown(browser);
    await expect(screen.getByRole('tab', new RegExp(`^${name.replace('.', '\\.')}`))).toHaveAttribute('aria-selected', 'true');
  }
  await tree.getByRole('treeitem', /src$/).click();
  await tree.getByRole('treeitem', /api$/).click();
  await tree.getByRole('treeitem', 'checkout.rs').click();
  await expect.poll(() => editorTabs(browser)).toEqual(['README.md', 'AGENTS.md', 'src/api/checkout.rs']);

  // Drag checkout.rs before README.md: the press does not choose it, the open file stays.
  await screen.getByRole('tab', /^AGENTS\.md/).click();
  const from = await screen.getByRole('tab', /^checkout\.rs/).boundingBox();
  const to = await screen.getByRole('tab', /^README\.md/).boundingBox();
  if (!from || !to) throw new Error('tabs not laid out');
  await browser.mouse.move(from.x + from.width / 2, from.y + from.height / 2);
  await browser.mouse.down();
  for (const x of [from.x - 10, to.x + to.width, to.x + to.width / 2, to.x + 4]) await browser.mouse.move(x, from.y + from.height / 2);
  await browser.mouse.up();
  await expect.poll(() => editorTabs(browser)).toEqual(['src/api/checkout.rs', 'README.md', 'AGENTS.md']);
  await expect(screen.getByRole('tab', /^AGENTS\.md/)).toHaveAttribute('aria-selected', 'true');

  // Alt+→ / Alt+← move the focused tab one place, the keyboard goes with it; the ends stop it.
  await screen.getByRole('tab', /^AGENTS\.md/).focus();
  await expect(screen.getByRole('tab', /^AGENTS\.md/)).toHaveAttribute('aria-keyshortcuts', 'Delete Backspace Alt+ArrowLeft Alt+ArrowRight');
  await browser.keyboard.press('Alt+ArrowLeft');
  expect(await editorTabs(browser)).toEqual(['src/api/checkout.rs', 'AGENTS.md', 'README.md']);
  await expect(screen.getByRole('tab', /^AGENTS\.md/)).toBeFocused();
  await browser.keyboard.press('Alt+ArrowLeft');
  await browser.keyboard.press('Alt+ArrowLeft');
  expect(await editorTabs(browser)).toEqual(['AGENTS.md', 'src/api/checkout.rs', 'README.md']);
  await browser.keyboard.press('Alt+ArrowRight');
  expect(await editorTabs(browser)).toEqual(['src/api/checkout.rs', 'AGENTS.md', 'README.md']);
  // Nothing was typed into the file or closed.
  await expect(screen.getByRole('tab', /^AGENTS\.md$/)).toHaveAttribute('aria-selected', 'true');

  // The tab's menu moves it without dragging (WCAG 2.5.7) and closes it.
  await browser.evaluate(() => {
    const tab = document.querySelector('.ed-tab[data-path="README.md"]') as HTMLElement;
    const box = tab.getBoundingClientRect();
    tab.dispatchEvent(new MouseEvent('contextmenu', { bubbles: true, cancelable: true, button: 2, clientX: box.left + 8, clientY: box.bottom }));
  });
  await expect(screen.getByRole('menuitem', 'Move right')).toHaveAttribute('aria-disabled', 'true');
  await screen.getByRole('menuitem', 'Move left').click();
  expect(await editorTabs(browser)).toEqual(['src/api/checkout.rs', 'README.md', 'AGENTS.md']);
  expect(await axeViolations(browser)).toEqual([]);
  await browser.evaluate(() => {
    const tab = document.querySelector('.ed-tab[data-path="src/api/checkout.rs"]') as HTMLElement;
    const box = tab.getBoundingClientRect();
    tab.dispatchEvent(new MouseEvent('contextmenu', { bubbles: true, cancelable: true, button: 2, clientX: box.left + 8, clientY: box.bottom }));
  });
  await screen.getByRole('menuitem', 'Close').click();
  expect(await editorTabs(browser)).toEqual(['README.md', 'AGENTS.md']);
});

type HoldMock = { __yuliusMock: { holdPresence: boolean; releasePresence(): void; keepaliveRequests: string[] } };

// Review round 1: closing a file, switching folder or leaving the page while an update is still on
// its way must still tell the others the person left.
test('a file closed, a folder switched or a page gone says the person left, even with an update on its way', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/t/12'));
  await screen.getByRole('button', /^Show tools/).click();
  await screen.getByRole('button', 'Editor').click();
  const tree = screen.getByRole('tree', 'Files');
  await tree.getByRole('treeitem', 'README.md').click();
  await editorShown(browser);
  await expect.poll(async () => (await fileSent(browser)).some((u) => u.path === 'README.md' && u.selection !== null)).toBe(true);
  const last = async (path: string) => (await fileSent(browser)).filter((u) => u.path === path).at(-1);

  // An update on its way (held), then the file is closed: once answered, `null` follows.
  await browser.evaluate(() => { (window as unknown as HoldMock).__yuliusMock.holdPresence = true; });
  await inEditor(browser, 'me', 'ed.focus(); ed.setSelection({ startLineNumber: 1, startColumn: 1, endLineNumber: 1, endColumn: 4 })');
  await expect.poll(async () => (await last('README.md'))?.selection).toEqual({ anchor: 0, head: 3 });
  await screen.getByRole('tab', /^README\.md/).focus();
  await browser.keyboard.press('Delete');
  await expect.poll(() => editorTabs(browser)).toEqual([]);
  await browser.evaluate(() => (window as unknown as HoldMock).__yuliusMock.releasePresence());
  await expect.poll(async () => (await last('README.md'))?.selection).toBe(null);

  // Another folder: the open file is left.
  await tree.getByRole('treeitem', 'AGENTS.md').click();
  await expect.poll(async () => (await last('AGENTS.md'))?.selection).toEqual({ anchor: 0, head: 0 });
  const folder = (index: number) => browser.evaluate((i) => {
    const select = document.querySelector('select[aria-label="Folder"]') as HTMLSelectElement;
    select.value = select.options[i].value;
    select.dispatchEvent(new Event('change', { bubbles: true }));
  }, index);
  await folder(1); // Task #12 worktree
  await expect.poll(async () => (await last('AGENTS.md'))?.selection).toBe(null);
  await folder(0);

  // The page goes: `null` with a request that outlives it.
  await screen.getByRole('tree', 'Files').getByRole('treeitem', 'README.md').click();
  await expect.poll(async () => (await last('README.md'))?.selection).toEqual({ anchor: 0, head: 0 });
  await browser.evaluate(() => window.dispatchEvent(new PageTransitionEvent('pagehide')));
  await expect.poll(async () => (await last('README.md'))?.selection).toBe(null);
  expect(await browser.evaluate(() => (window as unknown as HoldMock).__yuliusMock.keepaliveRequests.some((url) => url.includes('/files/presence')))).toBe(true);
});

type WorkerLog = { __workers: { errors: string[]; ready: number } };

// #509: Monaco ends its editor worker once the last file is closed, also while the worker still
// loads its script; the cut load threw an uncaught NetworkError on the page (the test above, under
// load). The worker script is held here so that the close always lands mid-load.
test('an editor worker ended while its script loads raises no error, and the next file starts a new one (#509)', async ({ app, browser, screen }) => {
  // Each worker's own events as the page's `Worker` object gets them (only listened to).
  await browser.addInitScript(() => {
    const log = { errors: [] as string[], ready: 0 };
    (window as unknown as WorkerLog).__workers = log;
    window.Worker = class extends Worker {
      constructor(url: string | URL, options?: WorkerOptions) {
        super(url, options);
        this.addEventListener('error', (event) => log.errors.push(event.message));
        this.addEventListener('message', (event) => { if (event.data?.type === 'vscode-worker-ready') log.ready += 1; });
      }
    };
  });
  let release = () => {};
  const held = new Promise<void>((resolve) => { release = resolve; });
  let requests = 0;
  // If a Monaco update renames the worker script, `requests` stays 0 and the test says so.
  await browser.route(/\/vendor\/monaco\/vs\/assets\/editor\.worker-[^/]*\.js$/, async (route) => {
    requests += 1;
    if (requests === 1) await held;
    await route.continue();
  });
  const workers = () => browser.evaluate(() => (window as unknown as WorkerLog).__workers);
  await app.open(mock('/s/studio/t/12'));
  await screen.getByRole('button', /^Show tools/).click();
  await screen.getByRole('button', 'Editor').click();
  const tree = screen.getByRole('tree', 'Files');
  await tree.getByRole('treeitem', 'README.md').click();
  await editorShown(browser);
  await expect.poll(() => requests, { message: 'the editor worker asked for its script' }).toBe(1);

  // The last file closed: Monaco ends the worker while its script is held.
  await screen.getByRole('tab', /^README\.md/).focus();
  await browser.keyboard.press('Delete');
  await expect.poll(() => editorTabs(browser)).toEqual([]);
  await expect.poll(async () => (await workers()).errors.map((e) => (e.includes('NetworkError') ? 'NetworkError' : e))).toEqual(['NetworkError']);
  release();

  // The next file gets a new worker, which loads.
  await tree.getByRole('treeitem', 'README.md').click();
  await editorShown(browser);
  await expect.poll(async () => (await workers()).ready, { timeout: MONACO_LOAD_MS }).toBe(1);
  expect(requests).toBe(2);
});

// #509, the other side: only an ended worker is quiet. A worker in use whose script fails to load
// is an error the page still reports.
test('an editor worker in use whose script fails to load is still an uncaught error (#509)', async ({ app, browser, screen }) => {
  let requests = 0;
  await browser.route(/\/vendor\/monaco\/vs\/assets\/editor\.worker-[^/]*\.js$/, async (route) => { requests += 1; await route.abort(); });
  await app.open(mock('/s/studio/t/12'));
  await screen.getByRole('button', /^Show tools/).click();
  await screen.getByRole('button', 'Editor').click();
  await screen.getByRole('tree', 'Files').getByRole('treeitem', 'README.md').click();
  await editorShown(browser);
  await expect.poll(() => requests, { message: 'the editor worker asked for its script' }).toBe(1);
  // The browser's report of the worker's error, then Monaco's own (`onUnexpectedError`).
  await expect.poll(() => pageErrors(browser)).toEqual([
    expect.stringContaining("uncaught: Uncaught NetworkError: Failed to execute 'importScripts'"),
    'uncaught: Uncaught [object ErrorEvent]',
  ]);
  // Expected here: these two are taken off the list `failOnPageErrors()` checks after the test.
  await browser.evaluate(() => { (window as unknown as { __e2eErrors: string[] }).__e2eErrors.splice(0, 2); });
});

// Review round 1: the strip drawn again during a drag (a save ending, a tab closed) must not leave
// the dragged tab behind, detached and frozen.
test('a tab closed while another is dragged keeps the drag going, and the order applies on release', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/t/12'));
  await screen.getByRole('button', /^Show tools/).click();
  await screen.getByRole('button', 'Editor').click();
  const tree = screen.getByRole('tree', 'Files');
  for (const name of ['README.md', 'AGENTS.md']) {
    await tree.getByRole('treeitem', name).click();
    await editorShown(browser);
    await expect(screen.getByRole('tab', new RegExp(`^${name.replace('.', '\\.')}`))).toHaveAttribute('aria-selected', 'true');
  }
  await tree.getByRole('treeitem', /src$/).click();
  await tree.getByRole('treeitem', /api$/).click();
  await tree.getByRole('treeitem', 'checkout.rs').click();
  await expect.poll(() => editorTabs(browser)).toEqual(['README.md', 'AGENTS.md', 'src/api/checkout.rs']);

  const from = await screen.getByRole('tab', /^checkout\.rs/).boundingBox();
  const to = await screen.getByRole('tab', /^AGENTS\.md/).boundingBox();
  if (!from || !to) throw new Error('tabs not laid out');
  await browser.mouse.move(from.x + from.width / 2, from.y + from.height / 2);
  await browser.mouse.down();
  await browser.mouse.move(from.x - 10, from.y + from.height / 2);
  // README closes meanwhile (its close mark, as a click elsewhere would).
  await browser.evaluate(() => (document.querySelector('.ed-tab[data-path="README.md"] .ed-tab-close') as HTMLElement).click());
  const dragged = () => browser.evaluate(() => {
    const tab = document.querySelector('.ed-tab.dragging') as HTMLElement | null;
    return tab ? { path: tab.dataset.path, connected: tab.isConnected, moved: tab.style.transform } : null;
  });
  await browser.mouse.move(to.x + 4, from.y + from.height / 2);
  expect(await dragged()).toEqual({ path: 'src/api/checkout.rs', connected: true, moved: expect.stringMatching(/^translateX\(-\d/) });
  await browser.mouse.up();
  await expect.poll(() => editorTabs(browser)).toEqual(['src/api/checkout.rs', 'AGENTS.md']);
});

// #477: a redraw held back during a plain click on a tab runs after the click, so the element
// pressed is the one clicked (Firefox drops a click whose pressed element was replaced).
test('a tab clicked while a redraw is held back is the one clicked, and is chosen', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/t/12'));
  await screen.getByRole('button', /^Show tools/).click();
  await screen.getByRole('button', 'Editor').click();
  const tree = screen.getByRole('tree', 'Files');
  for (const name of ['README.md', 'AGENTS.md']) {
    await tree.getByRole('treeitem', name).click();
    await editorShown(browser);
    await expect(screen.getByRole('tab', new RegExp(`^${name.replace('.', '\\.')}`))).toHaveAttribute('aria-selected', 'true');
  }
  await tree.getByRole('treeitem', /src$/).click();
  await tree.getByRole('treeitem', /api$/).click();
  await tree.getByRole('treeitem', 'checkout.rs').click();
  await expect.poll(() => editorTabs(browser)).toEqual(['README.md', 'AGENTS.md', 'src/api/checkout.rs']);

  // What the pressed tab and the clicked one are when the click arrives.
  await browser.evaluate(() => {
    const w = window as unknown as { __pressed: Element | null; __clicks: { same: boolean; connected: boolean; path: string }[] };
    w.__clicks = [];
    document.addEventListener('pointerdown', (event) => { w.__pressed = (event.target as Element).closest('.ed-tab'); }, true);
    document.addEventListener('click', (event) => {
      if (!event.isTrusted) return; // the close mark's own click below
      const tab = (event.target as Element).closest('.ed-tab') as HTMLElement | null;
      if (tab) w.__clicks.push({ same: tab === w.__pressed, connected: Boolean(w.__pressed && w.__pressed.isConnected), path: tab.dataset.path ?? '' });
    }, true);
  });
  const box = await screen.getByRole('tab', /^AGENTS\.md/).boundingBox();
  if (!box) throw new Error('tab not laid out');
  await browser.mouse.move(box.x + box.width / 2, box.y + box.height / 2);
  await browser.mouse.down();
  // README closes meanwhile: the strip's redraw waits for the press to end.
  await browser.evaluate(() => (document.querySelector('.ed-tab[data-path="README.md"] .ed-tab-close') as HTMLElement).click());
  await browser.mouse.up();
  await expect.poll(() => editorTabs(browser)).toEqual(['AGENTS.md', 'src/api/checkout.rs']);
  expect(await browser.evaluate(() => (window as unknown as { __clicks: unknown[] }).__clicks)).toEqual([{ same: true, connected: true, path: 'AGENTS.md' }]);
  await expect(screen.getByRole('tab', /^AGENTS\.md/)).toHaveAttribute('aria-selected', 'true');
});

// #477 review: the redraw held back during a press still comes when no click follows (released
// off the tab), and a click on a tab closed during the press chooses nothing.
test('a press released off its tab still redraws the strip, and a tab closed while pressed is not chosen', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/t/12'));
  await screen.getByRole('button', /^Show tools/).click();
  await screen.getByRole('button', 'Editor').click();
  const tree = screen.getByRole('tree', 'Files');
  for (const name of ['README.md', 'AGENTS.md']) {
    await tree.getByRole('treeitem', name).click();
    await editorShown(browser);
    await expect(screen.getByRole('tab', new RegExp(`^${name.replace('.', '\\.')}`))).toHaveAttribute('aria-selected', 'true');
  }
  await tree.getByRole('treeitem', /src$/).click();
  await tree.getByRole('treeitem', /api$/).click();
  await tree.getByRole('treeitem', 'checkout.rs').click();
  await expect.poll(() => editorTabs(browser)).toEqual(['README.md', 'AGENTS.md', 'src/api/checkout.rs']);
  const close = (path: string) => browser.evaluate((p) => (document.querySelector(`.ed-tab[data-path="${p}"] .ed-tab-close`) as HTMLElement).click(), path);

  // Pressed, README closed meanwhile, released below the strip: no click, the strip redraws.
  let box = await screen.getByRole('tab', /^AGENTS\.md/).boundingBox();
  if (!box) throw new Error('tab not laid out');
  await browser.mouse.move(box.x + box.width / 2, box.y + box.height / 2);
  await browser.mouse.down();
  await close('README.md');
  await browser.mouse.move(box.x + box.width / 2, box.y + box.height * 4);
  await browser.mouse.up();
  await expect.poll(() => editorTabs(browser)).toEqual(['AGENTS.md', 'src/api/checkout.rs']);
  await expect(screen.getByRole('tab', /^checkout\.rs/)).toHaveAttribute('aria-selected', 'true');

  // The pressed tab itself closes during the press: the click on it chooses nothing.
  box = await screen.getByRole('tab', /^AGENTS\.md/).boundingBox();
  if (!box) throw new Error('tab not laid out');
  await browser.mouse.move(box.x + box.width / 2, box.y + box.height / 2);
  await browser.mouse.down();
  await close('AGENTS.md');
  await browser.mouse.up();
  await expect.poll(() => editorTabs(browser)).toEqual(['src/api/checkout.rs']);
  await expect(screen.getByRole('tab', /^checkout\.rs/)).toHaveAttribute('aria-selected', 'true');
  expect(await inEditor(browser, 'me', 'return Boolean(ed.getModel()) && !ed.getModel().isDisposed()'), 'the open file’s model').toBe(true);
});

// #477 (SPEC-127 review round 2): the presence loop itself (unit, in the page). A file closed with
// an update on its way still leaves once that is answered, and nothing is armed after `destroy()`;
// refusals of access or input are not sent again, while too many requests and a lost one are.
test('a destroyed presence sends its waiting leave and arms no timer; only lost or throttled updates go again', async ({ app, browser }) => {
  await app.open(mock('/s/studio'));
  const result = await browser.evaluate(async () => {
    type Presence = { setShown(shown: boolean): void; soon(): void; destroy(options?: { going?: boolean }): void };
    type Create = (options: { put(body: unknown, options: { keepalive: boolean }): Promise<void>; state(): unknown; draw(): void; myId: string }) => Presence;
    const { createPresence } = await import('/assets/presence.js' as string) as { createPresence: Create };
    // The timers presence.js arms and has not cleared or run, told apart by where they were armed.
    const live = new Map<number, number>();
    const realSet = window.setTimeout;
    const realClear = window.clearTimeout;
    window.setTimeout = ((fn: () => void, ms?: number) => {
      if (!(new Error().stack ?? '').includes('/assets/presence.js')) return realSet(fn, ms);
      const id = realSet(() => { live.delete(id); fn(); }, ms);
      live.set(id, ms ?? 0);
      return id;
    }) as typeof window.setTimeout;
    window.clearTimeout = ((id?: number) => { if (id !== undefined) live.delete(id); realClear(id); }) as typeof window.clearTimeout;
    const settle = () => new Promise((resolve) => realSet(resolve, 0));
    try {
      // Destroyed with an update on its way: the leave waits for its answer, then goes once.
      const sent: { selection: unknown; keepalive: boolean }[] = [];
      const answers: { resolve(): void; reject(error: unknown): void }[] = [];
      const presence = createPresence({
        put: (body, { keepalive }) => {
          sent.push({ selection: (body as { selection: unknown }).selection, keepalive });
          return new Promise<void>((resolve, reject) => answers.push({ resolve, reject }));
        },
        state: () => ({ revision: 1, selection: { anchor: 0, head: 1 } }),
        draw: () => {},
        myId: 'me',
      });
      presence.setShown(true);
      presence.soon(); // a change while the hello is on its way: a timer armed
      const armedBefore = live.size;
      presence.destroy({ going: false });
      const afterDestroy = { sent: sent.length, timers: live.size };
      answers[0].resolve();
      await settle();
      const afterAnswer = { sent: sent.slice(1), timers: live.size };
      answers[1].reject({ status: 503 }); // the leave is not sent again
      await settle();
      const destroyed = { armedBefore, afterDestroy, afterAnswer, finally: { sent: sent.length, timers: live.size } };

      // Each refusal of the hello: is a retry armed?
      const retried: Record<string, number[]> = {};
      for (const status of [403, 404, 409, 422, 429, 503, 0]) {
        live.clear();
        const refused = createPresence({
          put: () => Promise.reject(status ? { status } : new Error('lost')),
          state: () => ({ revision: 1, selection: { anchor: 0, head: 0 } }),
          draw: () => {},
          myId: 'me',
        });
        refused.setShown(true);
        await settle();
        retried[status || 'lost'] = [...live.values()];
        for (const id of live.keys()) realClear(id);
        live.clear();
      }
      return { destroyed, retried };
    } finally {
      window.setTimeout = realSet;
      window.clearTimeout = realClear;
    }
  });
  expect(result.destroyed).toEqual({
    armedBefore: 1,
    afterDestroy: { sent: 1, timers: 0 },
    afterAnswer: { sent: [{ selection: null, keepalive: false }], timers: 0 },
    finally: { sent: 2, timers: 0 },
  });
  expect(result.retried).toEqual({ 403: [], 404: [], 409: [], 422: [], 429: [1000], 503: [1000], lost: [1000] });
});

// SPEC-181 EDT-013: at the Explorer's narrowest the label "Explorer" kept its width, so the header
// overflowed the Explorer and its Refresh button painted over the first file tab (task #168).
test('the Explorer header keeps its buttons inside the Explorer at its narrowest widths', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/t/12'));
  await screen.getByRole('button', /^Show tools/).click();
  await screen.getByRole('button', 'Editor').click();
  await screen.getByRole('tree', 'Files').getByRole('treeitem', 'README.md').click();
  await editorShown(browser);
  await expect(screen.getByRole('tab', /^README\.md/)).toBeVisible();
  const geometry = () => browser.evaluate(() => {
    const side = document.querySelector('.ed-side')!.getBoundingClientRect();
    const tab = document.querySelector('.ed-tab')!.getBoundingClientRect();
    const buttons = [...document.querySelectorAll('.ed-tree-head button')].map((button) => button.getBoundingClientRect());
    return {
      width: Math.round(side.width),
      buttons: buttons.length,
      outside: buttons.filter((box) => box.left < side.left - 0.5 || box.right > side.right + 0.5).length,
      overTab: buttons.filter((box) => box.right > tab.left + 0.5).length,
    };
  });
  // The separator's Home: the narrowest the person can drag it (140 px).
  await screen.getByRole('separator', 'Resize explorer').focus();
  await browser.keyboard.press('Home');
  await expect.poll(geometry).toEqual({ width: 140, buttons: 3, outside: 0, overTab: 0 });
  // The stylesheet's floor (a narrow panel): 120 px.
  await browser.evaluate(() => (document.querySelector('.ed-root') as HTMLElement).style.setProperty('--ed-side-w', '1px'));
  await expect.poll(geometry).toEqual({ width: 120, buttons: 3, outside: 0, overTab: 0 });
});

// SPEC-181 EDT-015: the file in front follows the disk. It used to be looked at only when its tab
// came to the front, so an agent's or a teammate's writes never showed while the person watched.
type LiveModel = {
  getValue(): string;
  getEOL(): string;
  getOffsetAt(position: { lineNumber: number; column: number }): number;
};
type LiveEditor = {
  getModel(): LiveModel | null;
  getPosition(): { lineNumber: number; column: number } | null;
  setPosition(position: { lineNumber: number; column: number }): void;
  trigger(source: string, handler: string, payload: unknown): void;
  focus(): void;
};
type LiveWindow = Window & {
  monaco: { editor: { getEditors(): LiveEditor[] } };
  __yuliusMock: {
    repoFile(sandbox: string, path: string, text: string): void;
    removeRepoFile(sandbox: string, path: string): void;
    repoFileTooLarge(sandbox: string, path: string): void;
    fileReads?: { path: string; known: string | null; whileSaving: boolean }[];
    holdSave: Promise<void> | null;
    holdReads?: (() => void)[] | null;
  };
};
const ON_DISK = 'alpha\nbeta\ngamma\n';
const writeDisk = (browser: Browser, text: string) => browser.evaluate((t) => (window as unknown as LiveWindow).__yuliusMock.repoFile('sb_studio', 'notes.txt', t), text);
const editorText = (browser: Browser) => browser.evaluate(() => (window as unknown as LiveWindow).monaco.editor.getEditors()[0]?.getModel()?.getValue() ?? '');
/** The file in front is looked at every 2 s: a condition with room for a few looks. */
const LOOKS_MS = 10_000;

async function openLiveFile(app: { open(url: string): Promise<void> }, browser: Browser, screen: import('@e2e-dev/web').Screen) {
  await app.open(mock('/s/studio/t/12'));
  await writeDisk(browser, ON_DISK);
  await screen.getByRole('button', /^Show tools/).click();
  await screen.getByRole('button', 'Editor').click();
  await screen.getByRole('tree', 'Files').getByRole('treeitem', 'notes.txt').click();
  await editorShown(browser);
  await expect.poll(() => editorText(browser)).toBe(ON_DISK);
  // The open's read and the activation's look have both been answered: what the disk does next is new.
  await expect.poll(() => browser.evaluate(() => ((window as unknown as LiveWindow).__yuliusMock.fileReads ?? []).filter((read) => read.path === 'notes.txt').length)).toBeGreaterThanOrEqual(2);
}

test('the file in front shows what others write on disk, with the cursor on its text, quietly and undoably', async ({ app, browser, screen }) => {
  await openLiveFile(app, browser, screen);
  // The cursor on "gamma"; a line arrives above it.
  await browser.evaluate(() => (window as unknown as LiveWindow).monaco.editor.getEditors()[0].setPosition({ lineNumber: 3, column: 3 }));
  await writeDisk(browser, 'zero\nalpha\nbeta\ngamma\n');
  await expect.poll(() => editorText(browser), { timeout: LOOKS_MS }).toBe('zero\nalpha\nbeta\ngamma\n');
  expect(await browser.evaluate(() => (window as unknown as LiveWindow).monaco.editor.getEditors()[0].getPosition())).toEqual({ lineNumber: 4, column: 3 });
  expect(await browser.evaluate(() => document.querySelector('.ed-status')?.textContent ?? '')).toContain('Saved');
  expect(await browser.evaluate(() => document.body.innerText.includes('changed on disk and was reloaded'))).toBe(false);
  // While nothing changes, the looks carry the known version and get no body.
  const reads = () => browser.evaluate(() => ((window as unknown as LiveWindow).__yuliusMock.fileReads ?? []).filter((read) => read.path === 'notes.txt'));
  const before = (await reads()).length;
  await expect.poll(async () => (await reads()).length, { timeout: LOOKS_MS }).toBeGreaterThan(before);
  expect((await reads()).slice(before).every((read) => typeof read.known === 'string' && read.known.length === 64)).toBe(true);
  // The reload is its own undo step.
  await browser.evaluate(() => { const ed = (window as unknown as LiveWindow).monaco.editor.getEditors()[0]; ed.focus(); ed.trigger('keyboard', 'undo', null); });
  expect(await editorText(browser)).toBe(ON_DISK);
});

test('unsaved edits are kept when the disk moves on, and saving asks to overwrite or reload', async ({ app, browser, screen }) => {
  await openLiveFile(app, browser, screen);
  await browser.evaluate(() => { const ed = (window as unknown as LiveWindow).monaco.editor.getEditors()[0]; ed.focus(); ed.setPosition({ lineNumber: 1, column: 1 }); ed.trigger('keyboard', 'type', { text: 'mine ' }); });
  await expect.poll(() => editorText(browser)).toBe(`mine ${ON_DISK}`);
  await writeDisk(browser, 'theirs\n');
  await expect.poll(() => browser.evaluate(() => document.querySelector('.ed-status')?.textContent ?? ''), { timeout: LOOKS_MS }).toContain('Changed on disk');
  expect(await editorText(browser)).toBe(`mine ${ON_DISK}`);
  await browser.keyboard.press('Control+s');
  const dialog = screen.getByRole('dialog', /changed on disk/);
  await expect(dialog.getByRole('button', 'Overwrite')).toBeVisible();
  await dialog.getByRole('button', 'Reload from disk').click();
  await expect.poll(() => editorText(browser)).toBe('theirs\n');
  await expect.poll(() => browser.evaluate(() => document.querySelector('.ed-status')?.textContent ?? '')).not.toContain('Changed on disk');
});

// #571: "Reload from disk" cleared the modified mark before it read the disk, so a read that found
// nothing to load left the person's text in the editor, marked Saved.
const editorStatus = (browser: Browser) => browser.evaluate(() => document.querySelector('.ed-status')?.textContent ?? '');

/** Types into the file in front, has the disk move on, and saves: the conflict dialog opens. */
async function conflictDialog(app: { open(url: string): Promise<void> }, browser: Browser, screen: import('@e2e-dev/web').Screen) {
  await openLiveFile(app, browser, screen);
  await browser.evaluate(() => { const ed = (window as unknown as LiveWindow).monaco.editor.getEditors()[0]; ed.focus(); ed.setPosition({ lineNumber: 1, column: 1 }); ed.trigger('keyboard', 'type', { text: 'mine ' }); });
  await expect.poll(() => editorText(browser)).toBe(`mine ${ON_DISK}`);
  await writeDisk(browser, 'theirs\n');
  await browser.keyboard.press('Control+s');
  const dialog = screen.getByRole('dialog', /changed on disk/);
  await expect(dialog.getByRole('button', 'Reload from disk')).toBeVisible();
  return dialog;
}

test('reloading a file deleted after a save conflict keeps the text modified and says it is gone', async ({ app, browser, screen }) => {
  const dialog = await conflictDialog(app, browser, screen);
  await browser.evaluate(() => (window as unknown as LiveWindow).__yuliusMock.removeRepoFile('sb_studio', 'notes.txt'));
  await dialog.getByRole('button', 'Reload from disk').click();
  await expect(browser.locator('.toast-msg:has-text("The file was deleted")')).toBeVisible();
  expect(await editorText(browser)).toBe(`mine ${ON_DISK}`);
  await expect.poll(() => editorStatus(browser)).toContain('Modified');
  await expect(screen.getByRole('tab', /^notes\.txt ?, modified/)).toBeVisible();
});

test('reloading a file back at the tab\'s version after a save conflict shows the disk\'s text, saved, undoably', async ({ app, browser, screen }) => {
  const dialog = await conflictDialog(app, browser, screen);
  await writeDisk(browser, ON_DISK);
  await dialog.getByRole('button', 'Reload from disk').click();
  await expect.poll(() => editorText(browser)).toBe(ON_DISK);
  await expect.poll(() => editorStatus(browser)).toContain('Saved');
  expect(await editorStatus(browser)).not.toContain('Modified');
  // The reload is one undo step: the person's text comes back, modified again.
  await browser.evaluate(() => { const ed = (window as unknown as LiveWindow).monaco.editor.getEditors()[0]; ed.focus(); ed.trigger('keyboard', 'undo', null); });
  expect(await editorText(browser)).toBe(`mine ${ON_DISK}`);
  await expect.poll(() => editorStatus(browser)).toContain('Modified');
});

test('reloading a file grown too large after a save conflict keeps the text modified and says so', async ({ app, browser, screen }) => {
  const dialog = await conflictDialog(app, browser, screen);
  await browser.evaluate(() => (window as unknown as LiveWindow).__yuliusMock.repoFileTooLarge('sb_studio', 'notes.txt'));
  await dialog.getByRole('button', 'Reload from disk').click();
  await expect(browser.locator('.toast-msg:has-text("notes.txt is too large for the editor")')).toBeVisible();
  expect(await editorText(browser)).toBe(`mine ${ON_DISK}`);
  await expect.poll(() => editorStatus(browser)).toContain('Modified');
});

// Review of SPEC-298: a look started while the reload was on its way (the page back in front) made
// the reload moot, so the person's text stayed without the disk's and without a word.
test('a look started while "Reload from disk" is on its way reloads in its place', async ({ app, browser, screen }) => {
  const dialog = await conflictDialog(app, browser, screen);
  const reads = () => browser.evaluate(() => ((window as unknown as LiveWindow).__yuliusMock.fileReads ?? []).filter((read) => read.path === 'notes.txt'));
  await browser.evaluate(() => { (window as unknown as LiveWindow).__yuliusMock.holdReads = []; });
  const before = (await reads()).length;
  await dialog.getByRole('button', 'Reload from disk').click();
  await expect.poll(async () => (await reads()).length).toBe(before + 1);
  await browser.evaluate(() => document.dispatchEvent(new Event('visibilitychange')));
  await expect.poll(async () => (await reads()).length).toBe(before + 2);
  // The newer look asks for the whole file, as the reload did.
  expect((await reads()).slice(before).map((read) => read.known)).toEqual([null, null]);
  // The reload's answer comes first, moot: the save ends and draws the tab strip, still modified.
  await browser.evaluate(() => { (document.querySelector('[role="tab"][data-path="notes.txt"]') as HTMLElement).dataset.before = 'reload'; });
  await browser.evaluate(() => (window as unknown as LiveWindow).__yuliusMock.holdReads?.[0]());
  await expect.poll(() => browser.evaluate(() => (document.querySelector('[role="tab"][data-path="notes.txt"]') as HTMLElement | null)?.dataset.before ?? 'redrawn')).toBe('redrawn');
  // Then the newer look's: the disk's text, and the status bar and the tab say it is saved.
  await browser.evaluate(() => { const held = (window as unknown as LiveWindow).__yuliusMock; const release = held.holdReads?.[1]; held.holdReads = null; release?.(); });
  await expect.poll(() => editorText(browser)).toBe('theirs\n');
  await expect.poll(() => editorStatus(browser)).toContain('Saved');
  await expect(screen.getByRole('tab', /^notes\.txt$/)).toBeVisible();
});

test('the Editor looks at the disk only while it is shown', async ({ app, browser, screen }) => {
  // The look's timer is held here, so each look happens when the test says.
  await app.open(mock('/s/studio/t/12'));
  await browser.evaluate(async () => {
    const { LOOK_MS } = await import('/assets/editorview.js');
    const w = window as unknown as Window & { __looks: (() => void)[] };
    w.__looks = [];
    const real = window.setInterval.bind(window);
    window.setInterval = ((fn: () => void, delay?: number, ...rest: unknown[]) => {
      if (delay === LOOK_MS) { w.__looks.push(fn); return 0; }
      return real(fn, delay, ...rest);
    }) as typeof window.setInterval;
  });
  await writeDisk(browser, ON_DISK);
  await screen.getByRole('button', /^Show tools/).click();
  await screen.getByRole('button', 'Editor').click();
  await screen.getByRole('tree', 'Files').getByRole('treeitem', 'notes.txt').click();
  await editorShown(browser);
  await expect.poll(() => editorText(browser)).toBe(ON_DISK);
  const reads = () => browser.evaluate(() => ((window as unknown as LiveWindow).__yuliusMock.fileReads ?? []).filter((read) => read.path === 'notes.txt').length);
  const look = () => browser.evaluate(() => (window as unknown as { __looks: (() => void)[] }).__looks.forEach((fn) => fn()));
  expect(await browser.evaluate(() => (window as unknown as { __looks: unknown[] }).__looks.length)).toBe(1);

  // Shown: the timer looks (it skips while the activation's look is still on its way).
  const shown = await reads();
  await expect.poll(async () => { await look(); return reads(); }).toBeGreaterThan(shown);

  await screen.getByRole('navigation', 'Tools').getByRole('button', 'Notes').click();
  const hidden = await reads();
  await writeDisk(browser, 'while hidden\n');
  await look();
  await look();
  expect(await reads()).toBe(hidden);

  // Back in front, it looks at once, before the timer's next look: that one look, no other.
  await screen.getByRole('navigation', 'Tools').getByRole('button', 'Editor').click();
  await expect.poll(() => editorText(browser)).toBe('while hidden\n');
  expect(await reads()).toBe(hidden + 1);
});

// Review of SPEC-181: a look started while the file saves (its tab clicked) could be answered with
// the text from before the save after the save ended, and put that text back as if saved.
test('no look at the disk starts while the file in front saves, whichever way it is asked for', async ({ app, browser, screen }) => {
  await openLiveFile(app, browser, screen);
  await browser.evaluate(() => { const ed = (window as unknown as LiveWindow).monaco.editor.getEditors()[0]; ed.focus(); ed.setPosition({ lineNumber: 1, column: 1 }); ed.trigger('keyboard', 'type', { text: 'mine ' }); });
  await browser.evaluate(() => {
    const w = window as unknown as LiveWindow & { __releaseSave(): void };
    const m = w.__yuliusMock;
    m.holdSave = new Promise((resolve) => { w.__releaseSave = () => { m.holdSave = null; resolve(); }; });
  });
  await browser.keyboard.press('Control+s');
  await expect.poll(() => browser.evaluate(() => document.querySelector('.ed-status')?.textContent ?? '')).toContain('Saving');
  await screen.getByRole('tab', /^notes\.txt/).click();
  await screen.getByRole('tree', 'Files').getByRole('treeitem', 'notes.txt').click();
  await browser.evaluate(() => document.dispatchEvent(new Event('visibilitychange')));
  await browser.evaluate(() => (window as unknown as LiveWindow & { __releaseSave(): void }).__releaseSave());
  await expect.poll(() => browser.evaluate(() => document.querySelector('.ed-status')?.textContent ?? '')).toContain('Saved');
  const during = await browser.evaluate(() => ((window as unknown as LiveWindow).__yuliusMock.fileReads ?? []).filter((read) => read.path === 'notes.txt' && read.whileSaving).length);
  expect(during).toBe(0);
  expect(await editorText(browser)).toBe(`mine ${ON_DISK}`);
});

// Review of SPEC-181: Monaco gives inserted text the model's line ending, so a file that went from
// LF to CRLF on disk kept LF in the editor, and its next save rewrote every line ending.
test('a change of line endings on disk reaches the editor, and a BOM file keeps its cursor', async ({ app, browser, screen }) => {
  await openLiveFile(app, browser, screen);
  const eol = () => browser.evaluate(() => (window as unknown as LiveWindow).monaco.editor.getEditors()[0].getModel()?.getEOL());
  expect(await eol()).toBe('\n');
  await writeDisk(browser, 'alpha\r\nbeta\r\ngamma\r\n');
  await expect.poll(eol, { timeout: LOOKS_MS }).toBe('\r\n');
  expect(await editorText(browser)).toBe('alpha\r\nbeta\r\ngamma\r\n');

  // Monaco keeps a BOM out of its text: a line added above moves the cursor down, nothing more.
  await writeDisk(browser, '\uFEFFone\ntwo\n');
  await expect.poll(() => editorText(browser), { timeout: LOOKS_MS }).toBe('one\ntwo\n');
  expect(await eol()).toBe('\n');
  await browser.evaluate(() => (window as unknown as LiveWindow).monaco.editor.getEditors()[0].setPosition({ lineNumber: 2, column: 2 }));
  await writeDisk(browser, '\uFEFFzero\none\ntwo\n');
  await expect.poll(() => editorText(browser), { timeout: LOOKS_MS }).toBe('zero\none\ntwo\n');
  expect(await browser.evaluate(() => (window as unknown as LiveWindow).monaco.editor.getEditors()[0].getPosition())).toEqual({ lineNumber: 3, column: 2 });
});

// Review round 2 of SPEC-181: an Overwrite after a conflict saved with `saving` already cleared, so
// a look could start while its write was on its way and put the other text back as saved.
test('no look at the disk starts while an Overwrite after a conflict saves', async ({ app, browser, screen }) => {
  await openLiveFile(app, browser, screen);
  await browser.evaluate(() => { const ed = (window as unknown as LiveWindow).monaco.editor.getEditors()[0]; ed.focus(); ed.setPosition({ lineNumber: 1, column: 1 }); ed.trigger('keyboard', 'type', { text: 'mine ' }); });
  await writeDisk(browser, 'theirs\n');
  await browser.keyboard.press('Control+s');
  const dialog = screen.getByRole('dialog', /changed on disk/);
  await expect(dialog.getByRole('button', 'Overwrite')).toBeVisible();
  await browser.evaluate(() => {
    const w = window as unknown as LiveWindow & { __releaseSave(): void };
    const m = w.__yuliusMock;
    m.holdSave = new Promise((resolve) => { w.__releaseSave = () => { m.holdSave = null; resolve(); }; });
  });
  await dialog.getByRole('button', 'Overwrite').click();
  await expect.poll(() => browser.evaluate(() => (window as unknown as { __yuliusMock: { savesOnTheirWay?: number } }).__yuliusMock.savesOnTheirWay ?? 0)).toBe(1);
  await screen.getByRole('tab', /^notes\.txt/).click();
  await browser.evaluate(() => (window as unknown as LiveWindow & { __releaseSave(): void }).__releaseSave());
  await expect.poll(() => browser.evaluate(() => document.querySelector('.ed-status')?.textContent ?? '')).toContain('Saved');
  const during = await browser.evaluate(() => ((window as unknown as LiveWindow).__yuliusMock.fileReads ?? []).filter((read) => read.path === 'notes.txt' && read.whileSaving).length);
  expect(during).toBe(0);
  expect(await editorText(browser)).toBe(`mine ${ON_DISK}`);
});

// SPEC-340: the folder picker offers the sandbox and every task worktree, each named by its branch,
// also with no task selected; one owned by another person opens read-only.
type FolderMock = Window & { __yuliusMock: { tasks: { number: number; sandbox_id: string; owner?: { login: string } }[] } };
const folderOptions = (browser: Browser) => browser.evaluate(() => [...(document.querySelector('select[aria-label="Folder"]') as HTMLSelectElement).options].map((o) => o.textContent));
const chooseFolder = (browser: Browser, label: string) => browser.evaluate((wanted) => {
  const select = document.querySelector('select[aria-label="Folder"]') as HTMLSelectElement;
  const option = [...select.options].find((o) => o.textContent === wanted);
  if (!option) throw new Error(`no folder ${wanted}`);
  select.value = option.value;
  select.dispatchEvent(new Event('change', { bubbles: true }));
}, label);
const statusText = (browser: Browser) => browser.evaluate(() => document.querySelector('.ed-status')?.textContent ?? '');

test('the folder picker lists every worktree by its branch, with no task selected', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio'));
  await screen.getByRole('button', /^Show tools/).click();
  await screen.getByRole('button', 'Editor').click();
  await expect(screen.getByRole('tree', 'Files').getByRole('treeitem', 'README.md')).toBeVisible();

  // The sandbox first, then the task worktrees newest first; a queued task has none yet.
  await expect.poll(async () => (await folderOptions(browser)).slice(0, 4)).toEqual([
    'Sandbox · quartio', '#14 feat/start-the-preview-server', '#13 feat/improve-the-code', '#12 feat/checkout-api',
  ]);
  const options = await folderOptions(browser);
  expect(options.some((o) => o?.startsWith('#8 '))).toBe(false);
  expect(await statusText(browser)).toBe('Sandbox repository · quartio');

  await chooseFolder(browser, '#13 feat/improve-the-code');
  await expect.poll(() => statusText(browser)).toBe('Task #13 worktree · feat/improve-the-code');
  await expect(screen.getByRole('tree', 'Files').getByRole('treeitem', 'README.md')).toBeVisible();
  expect(await browser.evaluate(() => (document.querySelector('select[aria-label="Folder"]') as HTMLSelectElement).title)).toBe('#13 feat/improve-the-code');
});

test('a worktree another person owns opens read-only and says why', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio'));
  await browser.evaluate(() => {
    const task = (window as unknown as FolderMock).__yuliusMock.tasks.find((t) => t.sandbox_id === 'sb_studio' && t.number === 10);
    if (task) task.owner = { login: 'anna-q' };
  });
  await screen.getByRole('button', /^Show tools/).click();
  await screen.getByRole('button', 'Editor').click();
  await expect.poll(async () => (await folderOptions(browser)).find((o) => o?.startsWith('#10 '))).toBe('#10 feat/aws-login-for-deploy (read-only)');
  await chooseFolder(browser, '#10 feat/aws-login-for-deploy (read-only)');
  await expect.poll(() => statusText(browser)).toBe('Read-only: Task #10 is another person’s · feat/aws-login-for-deploy');
  await expect(screen.getByRole('button', 'New file or folder')).toHaveCount(0);
  await expect(screen.getByRole('button', 'Upload files')).toHaveCount(0);

  const tree = screen.getByRole('tree', 'Files');
  await tree.getByRole('treeitem', 'README.md').click();
  await editorShown(browser);
  expect(await browser.evaluate(() => (window as unknown as { monaco: { editor: { getEditors(): { getOption(id: number): boolean }[]; EditorOption: { readOnly: number } } } }).monaco.editor.getEditors()[0].getOption((window as unknown as { monaco: { editor: { EditorOption: { readOnly: number } } } }).monaco.editor.EditorOption.readOnly))).toBe(true);
  await expect.poll(() => statusText(browser)).toContain('Read-only: Task #10 is another person’s');
  expect(await axeViolations(browser)).toEqual([]);

  // Back in a folder they may change, the editor writes again.
  await chooseFolder(browser, '#12 feat/checkout-api');
  await tree.getByRole('treeitem', 'README.md').click();
  await expect.poll(() => browser.evaluate(() => (window as unknown as { monaco: { editor: { getEditors(): { getOption(id: number): boolean }[]; EditorOption: { readOnly: number } } } }).monaco.editor.getEditors()[0].getOption((window as unknown as { monaco: { editor: { EditorOption: { readOnly: number } } } }).monaco.editor.EditorOption.readOnly))).toBe(false);
  await expect(screen.getByRole('button', 'New file or folder')).toHaveCount(1);
  await expect(screen.getByRole('button', 'Upload files')).toHaveCount(1);
});

test('the folder picker keeps the sandbox and the selected task when the worktrees cannot be read', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/t/12', '&fail=/api/sandboxes/sb_studio/files/roots'));
  await screen.getByRole('button', /^Show tools/).click();
  await screen.getByRole('button', 'Editor').click();
  await expect(screen.getByRole('tree', 'Files').getByRole('treeitem', 'README.md')).toBeVisible();
  await expect.poll(() => browser.evaluate(() => (window as unknown as { __yuliusMock: { failedReads: number } }).__yuliusMock.failedReads)).toBeGreaterThan(0);
  expect(await folderOptions(browser)).toEqual(['Sandbox', 'Task #12 worktree']);
});

// Review of SPEC-340: in another person's worktree Ctrl+S sends nothing, the explorer's menu offers
// nothing that writes, and a drop adds nothing and says why.
type ReadOnlyMock = FolderMock & { __yuliusMock: { saveRequests?: number; uploads?: unknown }; __e2eDrop?: (selector: string, files: [string, string, string][]) => void };
test('another person’s worktree saves nothing, offers no writing menu items and takes no drop', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio'));
  await browser.evaluate(() => {
    const task = (window as unknown as FolderMock).__yuliusMock.tasks.find((t) => t.sandbox_id === 'sb_studio' && t.number === 10);
    if (task) task.owner = { login: 'anna-q' };
    (window as unknown as ReadOnlyMock).__e2eDrop = (selector, files) => {
      const data = new DataTransfer();
      for (const [name, text, type] of files) data.items.add(new File([text], name, { type }));
      const target = document.querySelector(selector) as Element;
      for (const type of ['dragenter', 'dragover', 'drop']) target.dispatchEvent(new DragEvent(type, { bubbles: true, cancelable: true, composed: true, dataTransfer: data }));
    };
  });
  await screen.getByRole('button', /^Show tools/).click();
  await screen.getByRole('button', 'Editor').click();
  await expect.poll(async () => (await folderOptions(browser)).some((o) => o === '#10 feat/aws-login-for-deploy (read-only)')).toBe(true);
  await chooseFolder(browser, '#10 feat/aws-login-for-deploy (read-only)');
  const tree = screen.getByRole('tree', 'Files');
  await tree.getByRole('treeitem', 'README.md').click();
  await editorShown(browser);
  await screen.getByRole('textbox', 'Editor content').focus();
  await browser.keyboard.press('Control+s');
  // A save would go out before the next file's read: once that file shows, none was sent.
  await tree.getByRole('treeitem', 'AGENTS.md').click();
  await expect(screen.getByRole('tab', /^AGENTS\.md/)).toHaveAttribute('aria-selected', 'true');
  await expect.poll(() => statusText(browser)).toContain('Read-only: Task #10 is another person’s');
  expect(await browser.evaluate(() => (window as unknown as ReadOnlyMock).__yuliusMock.saveRequests ?? 0)).toBe(0);

  // The explorer's menu on a folder: nothing that writes.
  await browser.evaluate(() => document.querySelector('.ed-row[data-path="src"]')!.dispatchEvent(new MouseEvent('contextmenu', { bubbles: true, cancelable: true, clientX: 80, clientY: 120 })));
  await expect(screen.getByRole('menuitem', 'Copy path')).toBeVisible();
  await expect(screen.getByRole('menuitem', /^New file/)).toHaveCount(0);
  await expect(screen.getByRole('menuitem', /^New folder/)).toHaveCount(0);
  await expect(screen.getByRole('menuitem', /^Delete/)).toHaveCount(0);
  await browser.keyboard.press('Escape');

  // A drop on the explorer: nothing is sent, and it says why.
  await browser.evaluate(() => (window as unknown as ReadOnlyMock).__e2eDrop!('.ed-row[data-path="src"]', [['notes.txt', 'hello\n', 'text/plain']]));
  await expect(browser.locator('.ed-side')).toContainText('This folder is read-only: the task is another person’s.');
  await expect(tree.getByRole('treeitem', 'notes.txt')).toHaveCount(0);
});

// Review of SPEC-340: the folder list arriving while "Switch folder?" is asked must not turn the
// answer into a switch to the folder already open (the edits discarded, nothing switched).
test('a folder chosen while the worktrees are read switches to it once the question is answered', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/t/12', '&hold=/api/sandboxes/sb_studio/files/roots'));
  await screen.getByRole('button', /^Show tools/).click();
  await screen.getByRole('button', 'Editor').click();
  await screen.getByRole('tree', 'Files').getByRole('treeitem', 'README.md').click();
  await editorShown(browser);
  const content = screen.getByRole('textbox', 'Editor content');
  await content.focus();
  await browser.keyboard.type('unsaved ');
  await expect(screen.getByRole('tab', /^README\.md\s*, modified/)).toBeVisible();
  expect(await folderOptions(browser)).toEqual(['Sandbox', 'Task #12 worktree']);

  await chooseFolder(browser, 'Task #12 worktree');
  const dialog = screen.getByRole('dialog', /Switch folder/);
  await expect(dialog).toBeVisible();
  await browser.evaluate(() => (window as unknown as HeldMock).__yuliusMock.release());
  await expect.poll(async () => (await folderOptions(browser))[0]).toBe('Sandbox · quartio');
  await dialog.getByRole('button', 'Discard and switch').click();
  await expect.poll(() => statusText(browser)).toBe('Task #12 worktree · feat/checkout-api');
  expect(await browser.evaluate(() => (document.querySelector('select[aria-label="Folder"]') as HTMLSelectElement).value)).toMatch(/^task:/);
});
