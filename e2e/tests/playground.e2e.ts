import { test, type Browser } from '@e2e-dev/web';
import { expect } from 'e2e';
import { axeViolations, failOnPageErrors, mock, mockReady } from './support.ts';

failOnPageErrors();

// Playgrounds (SPEC-099 PLY-009): "On onboarding, we want a mode to play around … connection
// github orgs accounts etc shouldnt be mandatory … the user can upload a repo or a folder but
// things stay in local git". A fresh instance without GitHub offers the playground first; a
// dropped folder becomes one and opens; a template makes one from the Add sandbox dialog.

type Mocked = Window & {
  __yuliusMock: { playgroundUploads?: { name: string; names: string[] }[]; sandboxes: { slug: string; playground?: boolean }[] };
};

/** Dispatches a drop of loose files (name, text) on the playground drop zone. */
async function dropFiles(browser: Browser, files: [string, string][]): Promise<void> {
  await browser.evaluate((list) => {
    const zone = document.querySelector('.playground-drop');
    if (!zone) throw new Error('no drop zone');
    const data = new DataTransfer();
    for (const [name, text] of list) data.items.add(new File([text], name, { type: 'text/plain' }));
    for (const type of ['dragover', 'drop']) zone.dispatchEvent(new DragEvent(type, { bubbles: true, cancelable: true, composed: true, dataTransfer: data }));
  }, files);
}

test('a fresh instance without GitHub offers a playground first', async ({ app, browser, screen }) => {
  await app.open(mock('/', '&sandboxes=0&github=0'));

  await expect(screen.getByRole('heading', { level: 1 })).toHaveText('Try Yulius on your own code');
  await expect(screen.getByRole('group', 'Project folder')).toBeVisible();
  await expect(screen.getByRole('button', 'Choose a folder…')).toBeEnabled();
  await expect(screen.getByRole('button', 'Start from a template')).toBeEnabled();
  // GitHub comes second and can wait: no toast insists on it.
  await expect(screen.getByRole('link', 'Connect GitHub')).toBeVisible();
  await expect(browser.locator('.toast')).toHaveCount(0);
  expect(await axeViolations(browser)).toEqual([]);
});

test('a dropped folder becomes a playground and opens', async ({ app, browser, screen }) => {
  await app.open(mock('/', '&sandboxes=0&github=0'));
  await expect(screen.getByRole('group', 'Project folder')).toBeVisible();
  await mockReady(browser);

  await dropFiles(browser, [['README.md', '# Lab\n'], ['app.py', "print('hi')\n"]]);
  await expect.poll(() => browser.evaluate(() => location.hash)).toBe('#/s/project');
  const uploads = await browser.evaluate(() => (window as unknown as Mocked).__yuliusMock.playgroundUploads);
  expect(uploads).toEqual([{ name: 'project', names: ['files/README.md', 'files/app.py'] }]);

  // Home names it a playground, not a GitHub repository.
  await browser.evaluate(() => { location.hash = '#/'; });
  const card = screen.getByRole('article', /^project$/);
  await expect(card).toContainText('Playground · local Git on this server');
  expect(await axeViolations(browser)).toEqual([]);
});

test('a template becomes a playground from the Add sandbox dialog', async ({ app, browser, screen }) => {
  await app.open(mock('/', '&sandboxes=0&github=0'));
  await screen.getByRole('button', 'Start from a template').click();
  const dialog = screen.getByRole('dialog', 'Add sandbox');
  await expect(dialog.getByRole('radio', 'Playground')).toBeChecked();
  // Without GitHub, the repositories of the organisation wait for it.
  await expect(dialog.getByRole('radio', 'Import repository')).toBeDisabled();
  await expect(dialog.getByRole('radio', 'Create from template')).toBeDisabled();
  await expect(dialog.getByRole('radio', /^Empty project/)).toBeChecked();
  expect(await axeViolations(browser)).toEqual([]);

  await dialog.getByRole('textbox', 'Name').fill('My Shop');
  await dialog.getByRole('button', 'Start playground').click();
  await expect.poll(() => browser.evaluate(() => location.hash)).toBe('#/s/my-shop');
  const made = await browser.evaluate(() => (window as unknown as Mocked).__yuliusMock.sandboxes.find((s) => s.slug === 'my-shop'));
  expect(made && made.playground).toBe(true);
});

test('a folder is packed with its history and without what stays behind (unit, in the page)', async ({ app, browser }) => {
  await app.open(mock('/'));
  const names = await browser.evaluate(async () => {
    const url = '/assets/playground.js';
    const { tarOf } = await import(url);
    const long = `src/${'deep/'.repeat(30)}file.txt`;
    const entries = [
      'README.md', long, '.git/HEAD', '.git/packed-refs', '.git/objects/ab/cdef', '.git/refs/heads/main',
      '.git/config', '.git/hooks/pre-commit', '.git/index', '.git/info/attributes', '.git/objects/info/alternates',
      'node_modules/x/index.js', 'web/node_modules/y.js', 'target/debug/app', 'sub/.git/config', '.DS_Store', 'docs/target.md',
    ].map((path) => ({ path, file: new File([path], path.split('/').pop() || 'x') }));
    const { blob, files } = tarOf(entries);
    const data = new Uint8Array(await blob.arrayBuffer());
    const text = (from: number, length: number) => new TextDecoder().decode(data.subarray(from, from + length)).replace(/\0.*$/s, '');
    const out: string[] = [];
    let pax: string | null = null;
    for (let at = 0; at + 512 <= data.length;) {
      if (data.subarray(at, at + 512).every((b) => b === 0)) break;
      const size = parseInt(text(at + 124, 12), 8);
      const type = text(at + 156, 1);
      if (type === 'x') pax = /path=([^\n]*)\n/.exec(text(at + 512, size))?.[1] ?? null;
      else { out.push(pax || text(at, 100)); pax = null; }
      at += 512 + Math.ceil(size / 512) * 512;
    }
    return { out, files, long };
  });
  expect(names.files).toBe(names.out.length);
  expect(names.out).toEqual([
    'files/README.md', `files/${names.long}`, 'git/HEAD', 'git/packed-refs', 'git/objects/ab/cdef', 'git/refs/heads/main', 'files/docs/target.md',
  ]);
});
