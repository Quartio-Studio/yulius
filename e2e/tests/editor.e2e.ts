import { test, type Browser } from '@e2e-dev/web';
import { expect } from 'e2e';
import { axeViolations, failOnPageErrors, mock } from './support.ts';

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
