import { test, type Browser, type Screen } from '@e2e-dev/web';
import { expect } from 'e2e';
import { failOnPageErrors, mock, promptText } from './support.ts';

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

test('files dropped or pasted on the new-task prompt sit in its text, at the caret', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio'));
  const prompt = screen.getByRole('textbox', 'Task prompt');
  await prompt.fill('Fix the layout in ');
  await installDrop(browser);

  await browser.evaluate(() => (window as unknown as Mocked).__e2eDrop!('.composer-box', [['screen shot.png', 'png', 'image/png']], ['dragenter', 'dragover']));
  await expect(browser.locator('.composer-box .drop-overlay')).toHaveText('Drop to add to the prompt');
  await browser.evaluate(() => (window as unknown as Mocked).__e2eDrop!('.composer-box', [['screen shot.png', 'png', 'image/png']], ['drop']));
  await expect(browser.locator('.composer-prompt .pi-image img')).toHaveCount(1);
  await expect.poll(() => promptText(browser)).toBe('Fix the layout in [screen-shot.png]');

  await browser.evaluate(() => {
    const data = new DataTransfer();
    data.items.add(new File(['img'], 'image.png', { type: 'image/png' }));
    const input = document.querySelector('.composer-prompt') as HTMLElement;
    input.focus();
    input.dispatchEvent(new ClipboardEvent('paste', { clipboardData: data, bubbles: true, cancelable: true }));
  });
  await expect(browser.locator('.composer-prompt .pi-image img')).toHaveCount(2);
  await expect.poll(() => promptText(browser)).toMatch(/^Fix the layout in \[screen-shot\.png\]\[pasted-\d{8}-\d{6}\.png\]$/);

  // SPEC-025 ATT-001: a file opens from the prompt's preview; an image previews in a dialog that
  // Esc closes.
  await screen.getByRole('button', 'Preview').click();
  // The keyboard way: Enter on the file's button opens it (it does not leave the Preview).
  await screen.getByRole('button', 'Open screen-shot.png').press('Enter');
  await expect(screen.getByRole('dialog', 'screen-shot.png').getByRole('img', 'screen-shot.png')).toBeVisible();
  await browser.locator('dialog.modal').press('Escape');
  await expect(browser.locator('dialog.modal')).toHaveCount(0);
  await screen.getByRole('button', 'Preview').click();
  await expect(browser.locator('.composer-prompt .pi-atom')).toHaveCount(2);
  // ⌘/Ctrl+Enter still starts the task from a file's button in the Preview.
  await screen.getByRole('button', 'Preview').click();
  await screen.getByRole('button', 'Open screen-shot.png').press('ControlOrMeta+Enter');
  await expect(browser).toHaveURL(/#\/s\/studio\/t\/\d+$/);
  await browser.locator('.btn-newtask').click();
  await installDrop(browser);
  const prompt2 = screen.getByRole('textbox', 'Task prompt');
  await prompt2.fill('Fix the layout in ');
  await browser.evaluate(() => (window as unknown as Mocked).__e2eDrop!('.composer-box', [['screen shot.png', 'png', 'image/png'], ['shot2.png', 'png', 'image/png']], ['drop']));
  await expect(browser.locator('.composer-prompt .pi-atom:not(.pi-pending)')).toHaveCount(2);
  // Backspace takes the file before the caret, whole.
  await prompt.press('End');
  await prompt.press('Backspace');
  await expect(browser.locator('dialog.modal')).toHaveCount(0);
  await expect(browser.locator('.composer-prompt .pi-atom')).toHaveCount(1);
});

/** Opens a file of the new-task prompt from its preview. */
async function openFromPreview(screen: Screen, name: string): Promise<void> {
  await screen.getByRole('button', 'Preview').click();
  await screen.getByRole('button', `Open ${name}`).click();
}

test('an attached page opens as plain text and an unknown file downloads, never running as the site', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio'));
  await expect(screen.getByRole('textbox', 'Task prompt')).toBeVisible();
  await installDrop(browser);
  await browser.evaluate(() => {
    const w = window as unknown as { __opened: string[]; __downloads: string[][] };
    w.__opened = [];
    w.__downloads = [];
    window.open = (url?: string | URL) => { w.__opened.push(String(url)); return null; };
    HTMLAnchorElement.prototype.click = function click(this: HTMLAnchorElement) { w.__downloads.push([this.href, this.download]); };
  });
  await screen.getByRole('textbox', 'Task prompt').fill('See the files');
  await browser.evaluate(() => (window as unknown as Mocked).__e2eDrop!('.composer-box', [['bug.html', '<script>alert(1)</script>', 'text/html'], ['logs.zip', 'PK', 'application/zip']], ['drop']));
  await expect(browser.locator('.composer-prompt .pi-file')).toHaveCount(2);
  await openFromPreview(screen, 'bug.html');
  await screen.getByRole('button', 'Open logs.zip').click();
  const result = await browser.evaluate(async () => {
    const w = window as unknown as { __opened: string[]; __downloads: string[][] };
    const types = await Promise.all(w.__opened.map(async (url) => (await (await fetch(url)).blob()).type.split(';')[0]));
    return { types, downloads: w.__downloads.map(([href, name]) => [href.slice(0, 5), name]) };
  });
  expect(result.types).toEqual(['text/plain']);
  expect(result.downloads).toEqual([['blob:', 'logs.zip']]);
});

test('a file attached on the new-task prompt that is not an image opens in a new tab', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio'));
  await expect(screen.getByRole('textbox', 'Task prompt')).toBeVisible();
  await installDrop(browser);
  await browser.evaluate(() => {
    (window as unknown as { __opened: unknown[] }).__opened = [];
    window.open = (...args: unknown[]) => { (window as unknown as { __opened: unknown[] }).__opened.push(args); return null; };
  });
  await screen.getByRole('textbox', 'Task prompt').fill('See the notes');
  await browser.evaluate(() => (window as unknown as Mocked).__e2eDrop!('.composer-box', [['notes.txt', 'hello', 'text/plain']], ['drop']));
  await expect(browser.locator('.composer-prompt .pi-file')).toHaveCount(1);
  await openFromPreview(screen, 'notes.txt');
  const opened = await browser.evaluate(() => (window as unknown as { __opened: string[][] }).__opened);
  expect(opened).toHaveLength(1);
  expect(opened[0][0]).toMatch(/^blob:/);
  expect(opened[0][1]).toBe('_blank');
  await expect(browser.locator('dialog.modal')).toHaveCount(0);
});

// #259: the URL made for a file opened in a tab goes with the composer, like the thumbnails'.
test('leaving the new-task composer revokes the URL of a file opened in a tab', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio'));
  await expect(screen.getByRole('textbox', 'Task prompt')).toBeVisible();
  await installDrop(browser);
  await browser.evaluate(() => {
    const w = window as unknown as { __opened: string[]; __revoked: string[] };
    w.__opened = [];
    w.__revoked = [];
    window.open = (url?: string | URL) => { w.__opened.push(String(url)); return null; };
    const revoke = URL.revokeObjectURL.bind(URL);
    URL.revokeObjectURL = (url: string) => { w.__revoked.push(url); revoke(url); };
  });
  await screen.getByRole('textbox', 'Task prompt').fill('See the notes');
  await browser.evaluate(() => (window as unknown as Mocked).__e2eDrop!('.composer-box', [['notes.txt', 'hello', 'text/plain']], ['drop']));
  await expect(browser.locator('.composer-prompt .pi-file')).toHaveCount(1);
  await openFromPreview(screen, 'notes.txt');
  const [opened] = await browser.evaluate(() => (window as unknown as { __opened: string[] }).__opened);
  expect(opened).toMatch(/^blob:/);

  await screen.getByRole('link', /Checkout API/).first().click();
  await expect(screen.getByRole('heading', { level: 1 })).toHaveText(/#12/);
  await expect.poll(() => browser.evaluate(() => (window as unknown as { __revoked: string[] }).__revoked)).toContain(opened);
});
