import { test, type Browser, type Screen } from '@e2e-dev/web';
import { expect } from 'e2e';
import { failOnPageErrors, mock } from './support.ts';

failOnPageErrors();

// Dropped files (EDT-006, UI-048; F 2026-10-10): "also do drag and drop for files to both editor
// and claude (like images but also pdfs etc)". The tests dispatch the browser's own drag events
// with a DataTransfer holding files, as a drag from the computer does.

type Mocked = Window & {
  __yuliusMock: { repoTrees: Record<string, { files: Map<string, string> }> };
  __e2eDrop?: (selector: string, files: [string, string, string][], steps?: string[]) => boolean;
};

/** Installs a helper that dispatches a file drag (dragenter, dragover, drop) on an element. */
async function installDrop(browser: Browser): Promise<void> {
  await browser.evaluate(() => {
    (window as unknown as Mocked).__e2eDrop = (selector, files, steps = ['dragenter', 'dragover', 'drop']) => {
      const target = document.querySelector(selector);
      if (!target) throw new Error(`no ${selector}`);
      const data = new DataTransfer();
      for (const [name, text, type] of files) data.items.add(new File([text], name, { type }));
      for (const type of steps) target.dispatchEvent(new DragEvent(type, { bubbles: true, cancelable: true, composed: true, dataTransfer: data }));
      return true;
    };
  });
}

async function openEditor(screen: Screen): Promise<void> {
  await screen.getByRole('button', /^Show tools/).click();
  await screen.getByRole('button', 'Editor').click();
}

/** A file of the studio sandbox repository as the mock stores it (null when absent). */
async function repoFile(browser: Browser, path: string): Promise<string | null> {
  const files = await browser.evaluate(() => Object.fromEntries((window as unknown as Mocked).__yuliusMock.repoTrees['sb_studio|sandbox'].files));
  return (files as Record<string, string>)[path] ?? null;
}

test('files dropped on a folder of the explorer are added to it', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/t/12'));
  await openEditor(screen);
  const tree = screen.getByRole('tree', 'Files');
  await expect(tree.getByRole('treeitem', /src$/)).toBeVisible();
  await installDrop(browser);

  // While the files are over the folder, the overlay names it and the row is marked.
  await browser.evaluate(() => (window as unknown as Mocked).__e2eDrop!('.ed-row[data-path="src"]', [['notes.txt', 'hello from a drop\n', 'text/plain']], ['dragenter', 'dragover']));
  await expect(browser.locator('.ed-side .drop-overlay')).toHaveText('Drop to add to src/');
  expect(await browser.evaluate(() => document.querySelector('.ed-row[data-path="src"]')!.classList.contains('ed-drop-target'))).toBe(true);

  await browser.evaluate(() => (window as unknown as Mocked).__e2eDrop!('.ed-row[data-path="src"]', [['notes.txt', 'hello from a drop\n', 'text/plain']], ['drop']));
  await expect(browser.locator('.drop-overlay')).toBeHidden();
  await expect(tree.getByRole('treeitem', 'notes.txt')).toBeVisible();
  await expect(browser.locator('.ed-side .upload-text')).toHaveText('Added src/notes.txt');
  await expect.poll(() => repoFile(browser, 'src/notes.txt')).toBe('hello from a drop\n');
});

test('dropping a file that exists asks before replacing it', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/t/12'));
  await openEditor(screen);
  await expect(screen.getByRole('tree', 'Files').getByRole('treeitem', 'README.md')).toBeVisible();
  await installDrop(browser);
  const original = await repoFile(browser, 'README.md');

  await browser.evaluate(() => (window as unknown as Mocked).__e2eDrop!('.ed-tree', [['README.md', '# Dropped\n', 'text/markdown']]));
  const dialog = screen.getByRole('dialog', 'Replace README.md?');
  await expect(dialog).toBeVisible();
  // Nothing is replaced before the answer; keeping the file changes nothing.
  expect(await repoFile(browser, 'README.md')).toBe(original);
  await dialog.getByRole('button', 'Keep existing').click();
  await expect(dialog).toBeHidden();
  expect(await repoFile(browser, 'README.md')).toBe(original);

  await browser.evaluate(() => (window as unknown as Mocked).__e2eDrop!('.ed-tree', [['README.md', '# Dropped\n', 'text/markdown']]));
  await screen.getByRole('dialog', 'Replace README.md?').getByRole('button', 'Replace').click();
  await expect.poll(() => repoFile(browser, 'README.md')).toBe('# Dropped\n');
});

test('a text file dropped on the editor opens in a tab', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/t/12'));
  await openEditor(screen);
  const tree = screen.getByRole('tree', 'Files');
  await expect(tree.getByRole('treeitem', 'README.md')).toBeVisible();
  // The editor (Monaco, many files) loads when the first file opens. That cold load is not what
  // this test is about and takes however long the machine needs: wait for it as a condition, so
  // the drop below is judged against a ready editor.
  await tree.getByRole('treeitem', 'README.md').click();
  await expect.poll(() => browser.evaluate(() => Boolean((window as unknown as { monaco?: unknown }).monaco)), { timeout: 60000 }).toBe(true);
  await expect(screen.getByRole('tab', 'README.md')).toHaveAttribute('aria-selected', 'true');
  await installDrop(browser);
  await browser.evaluate(() => (window as unknown as Mocked).__e2eDrop!('.ed-stage', [['hello.md', '# Hello\n', 'text/markdown'], ['logo.png', 'not really a png', 'image/png']]));
  // The drop is listed, uploaded, listed again and then opened: several round trips, each as slow
  // as the page is starved. The condition is the tab, not a time.
  await expect(screen.getByRole('tab', 'hello.md')).toHaveAttribute('aria-selected', 'true', { timeout: 20000 });
  // The image is added, not opened.
  await expect(screen.getByRole('tree', 'Files').getByRole('treeitem', 'logo.png')).toBeVisible();
  await expect(screen.getByRole('tab', 'logo.png')).toBeHidden();
});

/** The task terminal's text. */
const taskTerminal = (browser: Browser) => browser.locator('.taskview-term .xterm-rows');

/** What task #12's mock session has received as typed input so far (before Enter). */
const typedIntoTask = (browser: Browser) =>
  browser.evaluate(() => (window as unknown as { __yuliusMock: { sessions: Map<string, { input: string }> } }).__yuliusMock.sessions.get('tsk_studio_12')?.input ?? '');

test('files dropped on the task terminal are typed into it by path', async ({ app, browser }) => {
  await app.open(mock('/s/studio/t/12'));
  await expect(taskTerminal(browser)).toHaveText(/Welcome to Claude Code/);
  await installDrop(browser);

  await browser.evaluate(() => (window as unknown as Mocked).__e2eDrop!('.taskview-term .term', [['screen shot.png', 'png', 'image/png']], ['dragenter', 'dragover']));
  await expect(browser.locator('.taskview-term .drop-overlay')).toHaveText('Drop to send to Claude');
  await browser.evaluate(() => (window as unknown as Mocked).__e2eDrop!('.taskview-term .term', [['screen shot.png', 'png', 'image/png'], ['brief.pdf', '%PDF-1.4', 'application/pdf']], ['drop']));
  // Both paths, space-separated, in names that need no quoting: what the session received as
  // input (the screen may wrap or redraw a long line).
  await expect.poll(() => typedIntoTask(browser)).toBe(
    '/var/lib/yulius/sandboxes/sb_studio/attachments/tasks/tsk_studio_12/screen-shot.png /var/lib/yulius/sandboxes/sb_studio/attachments/tasks/tsk_studio_12/brief.pdf ',
  );
  await expect(taskTerminal(browser)).toHaveText(/attachments\/tasks\/tsk_studio_12\/screen-shot\.png/);
});

test('an image pasted into the task terminal is uploaded and typed by path; text pastes stay text', async ({ app, browser }) => {
  await app.open(mock('/s/studio/t/12'));
  await expect(taskTerminal(browser)).toHaveText(/Welcome to Claude Code/);
  await browser.evaluate(() => {
    const data = new DataTransfer();
    data.items.add(new File(['img'], 'image.png', { type: 'image/png' }));
    const input = document.querySelector('.taskview-term .xterm-helper-textarea') as HTMLTextAreaElement;
    input.focus();
    input.dispatchEvent(new ClipboardEvent('paste', { clipboardData: data, bubbles: true, cancelable: true }));
  });
  await expect(taskTerminal(browser)).toHaveText(/attachments\/tasks\/tsk_studio_12\/pasted-\d{8}-\d{6}\.png/);

  await browser.evaluate(() => {
    const data = new DataTransfer();
    data.setData('text/plain', 'plain words');
    document.querySelector('.taskview-term .xterm-helper-textarea')!.dispatchEvent(new ClipboardEvent('paste', { clipboardData: data, bubbles: true, cancelable: true }));
  });
  await expect(taskTerminal(browser)).toHaveText(/plain words/);
});

test('a read-only terminal refuses dropped files', async ({ app, browser }) => {
  await app.open(mock('/s/sandbox-3/t/2'));
  await expect(taskTerminal(browser)).toHaveText(/Welcome to Claude Code/);
  await installDrop(browser);
  await browser.evaluate(() => (window as unknown as Mocked).__e2eDrop!('.taskview-term .term', [['x.txt', 'x', 'text/plain']], ['dragenter', 'dragover']));
  await expect(browser.locator('.taskview-term .drop-overlay')).toHaveText(/Read-only/);
});

test('files dropped or pasted on the new-task prompt show as thumbnails and join it by path when the task starts', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio'));
  const prompt = screen.getByRole('textbox', 'Task prompt');
  await prompt.fill('Fix the layout in');
  await installDrop(browser);

  await browser.evaluate(() => (window as unknown as Mocked).__e2eDrop!('.composer-box', [['screen shot.png', 'png', 'image/png']], ['dragenter', 'dragover']));
  await expect(browser.locator('.composer-box .drop-overlay')).toHaveText('Drop to add to the prompt');
  await browser.evaluate(() => (window as unknown as Mocked).__e2eDrop!('.composer-box', [['screen shot.png', 'png', 'image/png']], ['drop']));
  await expect(browser.locator('.composer-attachment img')).toHaveCount(1);
  await expect(prompt).toHaveValue('Fix the layout in');

  await browser.evaluate(() => {
    const data = new DataTransfer();
    data.items.add(new File(['img'], 'image.png', { type: 'image/png' }));
    const input = document.querySelector('.composer-prompt') as HTMLTextAreaElement;
    input.focus();
    input.dispatchEvent(new ClipboardEvent('paste', { clipboardData: data, bubbles: true, cancelable: true }));
  });
  await expect(browser.locator('.composer-attachment img')).toHaveCount(2);
  await expect(prompt).toHaveValue('Fix the layout in');

  // SPEC-025 ATT-001: a chip opens its file; an image previews in a dialog that Esc closes.
  await screen.getByRole('button', 'Open screen-shot.png').click();
  await expect(screen.getByRole('dialog', 'screen-shot.png').getByRole('img', 'screen-shot.png')).toBeVisible();
  await browser.locator('dialog.modal').press('Escape');
  await expect(browser.locator('dialog.modal')).toHaveCount(0);
  await expect(browser.locator('.composer-attachment')).toHaveCount(2);
  // Remove only removes.
  await screen.getByRole('button', 'Remove screen-shot.png').click();
  await expect(browser.locator('dialog.modal')).toHaveCount(0);
  await expect(browser.locator('.composer-attachment')).toHaveCount(1);
});

test('a file attached on the new-task prompt that is not an image opens in a new tab', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio'));
  await expect(screen.getByRole('textbox', 'Task prompt')).toBeVisible();
  await installDrop(browser);
  await browser.evaluate(() => {
    (window as unknown as { __opened: unknown[] }).__opened = [];
    window.open = (...args: unknown[]) => { (window as unknown as { __opened: unknown[] }).__opened.push(args); return null; };
  });
  await browser.evaluate(() => (window as unknown as Mocked).__e2eDrop!('.composer-box', [['notes.txt', 'hello', 'text/plain']], ['drop']));
  await screen.getByRole('button', 'Open notes.txt').click();
  const opened = await browser.evaluate(() => (window as unknown as { __opened: string[][] }).__opened);
  expect(opened).toHaveLength(1);
  expect(opened[0][0]).toMatch(/^blob:/);
  expect(opened[0][1]).toBe('_blank');
  await expect(browser.locator('dialog.modal')).toHaveCount(0);
});
