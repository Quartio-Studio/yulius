import { test } from '@e2e-dev/web';
import { expect } from 'e2e';
import { axeViolations, failOnPageErrors, mock, touchTask } from './support.ts';

failOnPageErrors();

// F 2026-10-12 (task #42, SPEC-043): "Here I want a button: 'see the original prompt' that shows
// the prompt I uploaded here, in the task window".

test('the task menu shows the original prompt with its attached files', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/t/12'));
  await expect(screen.getByRole('heading', { level: 1 })).toHaveText('#12 Checkout API');

  await browser.locator('.taskview .stepstrip-main [aria-label="Task actions"]').click();
  await screen.getByRole('menuitem', 'See the original prompt').click();

  const dialog = screen.getByRole('dialog', 'Original prompt · #12');
  await expect(dialog).toBeVisible();
  // The text, formatted as written; the attachment lines are files below it, not text.
  await expect(dialog).toHaveText(/Checkout API\s*Build the checkout endpoint:\s*Stripe payment intents\s*an idempotency key per order/);
  await expect(dialog).not.toHaveText(/@\//);

  // An image shows as a thumbnail with its number that opens full size (SPEC-180); another file
  // downloads; a file that is gone is named.
  const files = dialog.getByRole('list', 'Attached files');
  await expect(files.getByRole('img', 'checkout-error.png')).toBeVisible();
  await expect(files.getByRole('button', 'Download notes.txt')).toBeVisible();
  await expect(files).toHaveText(/old-mockup\.png\s*No longer on the server/);
  expect(await axeViolations(browser)).toEqual([]);

  // SPEC-325 UPL-003: a file can be as large as an upload (1 GiB): the browser downloads it from
  // its link, never read into the page with a total timeout.
  await files.getByRole('button', 'Download notes.txt').click();
  await expect.poll(() => browser.evaluate(() => (window as unknown as { __yuliusMock: { downloads?: string[][] } }).__yuliusMock.downloads ?? []))
    .toEqual([['/api/tasks/tsk_studio_12/prompt/attachments/1', 'notes.txt']]);
  // Asked once with HEAD, and read only by that download (the mock's stand-in for the browser,
  // whose read lands after the mock's latency: polled).
  await expect.poll(() => browser.evaluate(() => {
    const m = (window as unknown as { __yuliusMock: { sent?: Record<string, number>; promptFileReads?: Record<string, number> } }).__yuliusMock;
    return [m.sent?.['HEAD /api/tasks/tsk_studio_12/prompt/attachments/1'] ?? 0, m.promptFileReads?.['tsk_studio_12/1'] ?? 0];
  }), { timeout: 5000 }).toEqual([1, 1]);

  await files.getByRole('button', 'Open Image 1, checkout-error.png').click();
  const preview = screen.getByRole('dialog', 'checkout-error.png');
  await expect(preview.getByRole('img', 'checkout-error.png')).toBeVisible();
  await browser.keyboard.press('Escape');
  await expect(preview).toBeHidden();

  await browser.keyboard.press('Escape');
  await expect(dialog).toBeHidden();
});

// SPEC-325 review: a file removed after the list was read says so, as the queue card does, and
// nothing is downloaded.
test('a file removed after the list was shown says it is not on the server', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/t/12'));
  await expect(screen.getByRole('heading', { level: 1 })).toHaveText('#12 Checkout API');
  await browser.locator('.taskview .stepstrip-main [aria-label="Task actions"]').click();
  await screen.getByRole('menuitem', 'See the original prompt').click();
  const files = screen.getByRole('dialog', 'Original prompt · #12').getByRole('list', 'Attached files');
  await expect(files.getByRole('button', 'Download notes.txt')).toBeVisible();
  await browser.evaluate(() => (window as unknown as { __yuliusMock: { removePromptFile: (p: string) => void } }).__yuliusMock
    .removePromptFile('/srv/yulius/sandboxes/sb_studio/attachments/drafts/usr_mkarp/notes.txt'));
  await files.getByRole('button', 'Download notes.txt').click();
  await expect(browser.locator('.toast-msg:has-text("notes.txt is not on the server")')).toBeVisible();
  expect(await browser.evaluate(() => (window as unknown as { __yuliusMock: { downloads?: string[][] } }).__yuliusMock.downloads ?? [])).toEqual([]);
});

test('the sidebar menu opens it too, and a failed load says why', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/t/12', '&fail=/api/tasks/tsk_studio_12/prompt'));
  await expect(screen.getByRole('heading', { level: 1 })).toHaveText('#12 Checkout API');

  const row = browser.locator('.task-row:has-text("Checkout API")');
  await row.hover();
  await browser.locator('.task-row:has-text("Checkout API") [aria-label="Task actions"]').click();
  await screen.getByRole('menuitem', 'See the original prompt').click();
  const dialog = screen.getByRole('dialog', 'Original prompt · #12');
  await expect(dialog).toHaveText(/The mock server failed on purpose/);
});

// #303: a prompt is shown as written, and only the task's own attachment folders are served.
test('a prompt starting with --- is text, and files outside the task’s folders are not served', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/t/12'));
  await expect(screen.getByRole('heading', { level: 1 })).toHaveText('#12 Checkout API');
  const attachments = '/srv/yulius/sandboxes/sb_studio/attachments';
  await touchTask(browser, 'sb_studio', 12, {
    prompt: [
      '---', 'title: not properties', '---', 'Body of the prompt', '',
      `@${attachments}/drafts/usr_mkarp/own.png`,
      `@${attachments}/tasks/tsk_studio_12/dropped.png`,
      `@${attachments}/drafts/usr_other/theirs.png`,
      `@${attachments}/drafts/usr_mkarp/../usr_other/climb.png`,
      // In an owned folder by name, but `..` itself.
      `@${attachments}/drafts/usr_mkarp/..`,
      `@${attachments}/tasks/tsk_studio_3/other-task.png`,
    ].join('\n'),
  });

  await browser.locator('.taskview .stepstrip-main [aria-label="Task actions"]').click();
  await screen.getByRole('menuitem', 'See the original prompt').click();
  const dialog = screen.getByRole('dialog', 'Original prompt · #12');
  await expect(dialog).toHaveText(/title: not properties\s*Body of the prompt/);
  // Not frontmatter: no property named title.
  await expect(dialog.getByRole('textbox', 'title')).toBeHidden();

  const files = dialog.getByRole('list', 'Attached files');
  await expect(files.getByRole('img', 'own.png')).toBeVisible();
  await expect(files.getByRole('img', 'dropped.png')).toBeVisible();
  for (const name of ['theirs.png', 'climb.png', 'other-task.png']) {
    await expect(files.getByRole('listitem').filter({ hasText: name })).toHaveText(/No longer on the server/);
  }
  await expect(files.getByRole('listitem').filter({ hasText: 'No longer on the server' })).toHaveCount(4);
});

// SPEC-124 INL-008 (task #105): a file inside the text shows where it sits; every file is listed
// below the text (SPEC-180).
test('a file inside the prompt shows in place, every file below it', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/t/12'));
  await expect(screen.getByRole('heading', { level: 1 })).toHaveText('#12 Checkout API');
  const drafts = '/srv/yulius/sandboxes/sb_studio/attachments/drafts/usr_mkarp';
  await touchTask(browser, 'sb_studio', 12, {
    // The text's own private-use marker characters never place a file (SPEC-124 review).
    prompt: `This part @'${drafts}/inline.png' looks wrong,\nwhile here\uE0000\uE001 it is fine.\n\n@${drafts}/after.png`,
  });

  await browser.locator('.taskview .stepstrip-main [aria-label="Task actions"]').click();
  await screen.getByRole('menuitem', 'See the original prompt').click();
  const dialog = screen.getByRole('dialog', 'Original prompt · #12');
  const text = browser.locator('.prompt-modal .prompt-text');
  await expect(text.getByRole('button', { name: 'Open Image 1, inline.png' })).toHaveCount(1);
  // In its place: between the words around it.
  const shown = await browser.evaluate(() => document.querySelector('.prompt-modal .prompt-text')!.textContent);
  // The forged marker left its digit as text and placed no file.
  expect(shown).toMatch(/This part\s*Image 1\s*looks wrong,\s*while here0 it is fine\./);
  expect(shown).not.toContain('@');
  // The file after the text is listed below it, not inside it; the list holds both.
  const files = dialog.getByRole('list', 'Attached files');
  await expect(files.getByRole('img', 'after.png')).toBeVisible();
  await expect(files.getByRole('img', 'inline.png')).toBeVisible();
  expect(await browser.evaluate(() => document.querySelectorAll('.prompt-modal .prompt-text [aria-label*="after.png"], .prompt-modal .prompt-text [title*="after.png"]').length)).toBe(0);
  expect(await axeViolations(browser)).toEqual([]);

  await text.getByRole('button', { name: 'Open Image 1, inline.png' }).click();
  await expect(screen.getByRole('dialog', 'inline.png').getByRole('img', 'inline.png')).toBeVisible();
});

// Task #167 (SPEC-180, #565): an image inside the prompt is a one-line `Image N` placeholder, as in
// the composer (SPEC-150), not a thumbnail that makes its line 160 px high; the pictures are in the
// list below the text, numbered.
test('an image inside the prompt is a one-line Image N placeholder, its picture in the list', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/t/12'));
  await expect(screen.getByRole('heading', { level: 1 })).toHaveText('#12 Checkout API');
  const drafts = '/srv/yulius/sandboxes/sb_studio/attachments/drafts/usr_mkarp';
  await touchTask(browser, 'sb_studio', 12, {
    prompt: `First @'${drafts}/a.png' then @'${drafts}/notes.txt' and @'${drafts}/a.png' again,\nsecond @'${drafts}/b.png' line.\n\n@${drafts}/after.png`,
  });

  await browser.locator('.taskview .stepstrip-main [aria-label="Task actions"]').click();
  await screen.getByRole('menuitem', 'See the original prompt').click();
  const dialog = screen.getByRole('dialog', 'Original prompt · #12');
  const text = browser.locator('.prompt-modal .prompt-text');
  await expect(text.getByRole('button', { name: 'Open Image 1, a.png' })).toHaveCount(2);
  await expect(text.getByRole('button', { name: 'Open Image 2, b.png' })).toBeVisible();
  await expect(text.getByRole('button', { name: 'Download notes.txt' })).toBeVisible();
  // Labels, not pictures: the text holds no image, and reads as written.
  await expect(text.getByRole('img')).toHaveCount(0);
  await expect(text).toHaveText(/First\s*Image 1\s*then\s*notes\.txt\s*and\s*Image 1\s*again,\s*second\s*Image 2\s*line\./);
  // One line high: the placeholder is no taller than a line of the text around it.
  const sizes = await browser.evaluate(() => {
    const atom = document.querySelector('.prompt-modal .prompt-text [aria-label="Open Image 2, b.png"]')!;
    return { atom: atom.getBoundingClientRect().height, line: parseFloat(getComputedStyle(atom.parentElement!).lineHeight) || 24 };
  });
  expect(sizes.atom).toBeLessThanOrEqual(sizes.line + 2);

  // The list holds every file once, in order, an image's thumbnail with its number.
  const files = dialog.getByRole('list', 'Attached files');
  await expect(files.getByRole('listitem')).toHaveCount(4);
  await expect(files).toHaveText(/1\s*a\.png\s*notes\.txt\s*2\s*b\.png\s*3\s*after\.png/);
  await expect(files.getByRole('img', 'a.png')).toBeVisible();
  await expect(files.getByRole('button', 'Open Image 3, after.png')).toBeVisible();
  expect(await axeViolations(browser)).toEqual([]);

  // A placeholder opens its image full size.
  await text.getByRole('button', { name: 'Open Image 2, b.png' }).click();
  const preview = screen.getByRole('dialog', 'b.png');
  await expect(preview.getByRole('img', 'b.png')).toBeVisible();
  await browser.keyboard.press('Escape');
  await expect(preview).toBeHidden();

  // The image named twice is one file: its second placeholder opens what the list read (review
  // round 2), with no read of its own.
  await browser.locator('.prompt-modal .prompt-text [aria-label="Open Image 1, a.png"] >> nth=1').click();
  const first = screen.getByRole('dialog', 'a.png');
  await expect(first.getByRole('img', 'a.png')).toBeVisible();
  type Mock = { __yuliusMock: { promptFileReads?: Record<string, number> } };
  expect(await browser.evaluate(() => (window as unknown as Mock).__yuliusMock.promptFileReads ?? null)).toMatchObject({ 'tsk_studio_12/0': 1 });
  expect(await browser.evaluate(() => (window as unknown as Mock).__yuliusMock.promptFileReads?.['tsk_studio_12/2'] ?? 0)).toBe(0);
});

// SPEC-180 OPP-002 (review round 1): a placeholder reads its image once, shared with the list,
// opens one preview however often it is clicked, and tries again after a failed read.
test('a placeholder opens its image once, and tries again after a failed read', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/t/12'));
  await expect(screen.getByRole('heading', { level: 1 })).toHaveText('#12 Checkout API');
  const drafts = '/srv/yulius/sandboxes/sb_studio/attachments/drafts/usr_mkarp';
  await touchTask(browser, 'sb_studio', 12, { prompt: `See @'${drafts}/a.png' here.` });
  type Mock = { __yuliusMock: { failPath: string | null; promptFileReads?: Record<string, number> } };
  await browser.evaluate(() => { (window as unknown as Mock).__yuliusMock.failPath = '/api/tasks/tsk_studio_12/prompt/attachments/0'; });

  await browser.locator('.taskview .stepstrip-main [aria-label="Task actions"]').click();
  await screen.getByRole('menuitem', 'See the original prompt').click();
  const dialog = screen.getByRole('dialog', 'Original prompt · #12');
  const files = dialog.getByRole('list', 'Attached files');
  await expect(files).toHaveText(/a\.png\s*Couldn’t load it/);
  const text = browser.locator('.prompt-modal .prompt-text');
  await text.getByRole('button', { name: 'Open Image 1, a.png' }).click();
  await expect(browser.locator('.toast-msg:has-text("Couldn’t open a.png")')).toBeVisible();

  // The server answers again: the next click reads it and opens it.
  await browser.evaluate(() => { (window as unknown as Mock).__yuliusMock.failPath = null; });
  await text.getByRole('button', { name: 'Open Image 1, a.png' }).click();
  const preview = screen.getByRole('dialog', 'a.png');
  await expect(preview.getByRole('img', 'a.png')).toBeVisible();
  await browser.keyboard.press('Escape');
  await expect(preview).toBeHidden();
  // The list's tile shows the picture it could not load before (review round 2).
  await expect(files.getByRole('img', 'a.png')).toBeVisible();
  await expect(files).not.toHaveText(/Couldn’t load it/);
});

// Review round 2: clicks while the image is still being read open one preview, from one read
// shared with the list's thumbnail.
test('clicks while the image is read open one preview from one read', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/t/12', '&hold=/api/tasks/tsk_studio_12/prompt/attachments/0'));
  await expect(screen.getByRole('heading', { level: 1 })).toHaveText('#12 Checkout API');
  const drafts = '/srv/yulius/sandboxes/sb_studio/attachments/drafts/usr_mkarp';
  await touchTask(browser, 'sb_studio', 12, { prompt: `See @'${drafts}/a.png' here.` });

  await browser.locator('.taskview .stepstrip-main [aria-label="Task actions"]').click();
  await screen.getByRole('menuitem', 'See the original prompt').click();
  const text = browser.locator('.prompt-modal .prompt-text');
  await text.getByRole('button', { name: 'Open Image 1, a.png' }).click();
  await text.getByRole('button', { name: 'Open Image 1, a.png' }).click();
  type Mock = { __yuliusMock: { release: () => void; promptFileReads?: Record<string, number> } };
  await browser.evaluate(() => { (window as unknown as Mock).__yuliusMock.release(); });
  await expect(screen.getByRole('dialog', 'a.png').getByRole('img', 'a.png')).toBeVisible();
  expect(await browser.evaluate(() => document.querySelectorAll('dialog[open]').length)).toBe(2);
  expect(await browser.evaluate(() => (window as unknown as Mock).__yuliusMock.promptFileReads?.['tsk_studio_12/0'] ?? 0)).toBe(1);
});

// SPEC-180 (review round 1): a gone image keeps its number, as in the queue card (SPEC-168), and a
// gone file never hides an available one of the same name from the list.
test('a gone image keeps its number, and the list keeps a file named like a gone one', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/t/12'));
  await expect(screen.getByRole('heading', { level: 1 })).toHaveText('#12 Checkout API');
  const attachments = '/srv/yulius/sandboxes/sb_studio/attachments';
  await touchTask(browser, 'sb_studio', 12, {
    prompt: `Old @'${attachments}/drafts/usr_other/old.png', gone @'${attachments}/drafts/usr_other/a.png', new @'${attachments}/drafts/usr_mkarp/a.png'.`,
  });

  await browser.locator('.taskview .stepstrip-main [aria-label="Task actions"]').click();
  await screen.getByRole('menuitem', 'See the original prompt').click();
  const dialog = screen.getByRole('dialog', 'Original prompt · #12');
  const text = browser.locator('.prompt-modal .prompt-text');
  await expect(text).toHaveText(/Old\s*old\.png\s*No longer on the server\s*,\s*gone\s*a\.png\s*No longer on the server\s*,\s*new\s*Image 2\s*\./);
  await expect(text.getByRole('button', { name: 'Open Image 2, a.png' })).toBeVisible();

  const files = dialog.getByRole('list', 'Attached files');
  await expect(files.getByRole('listitem')).toHaveCount(3);
  await expect(files.getByRole('listitem').filter({ hasText: 'No longer on the server' })).toHaveCount(2);
  await expect(files.getByRole('img', 'a.png')).toBeVisible();
  await expect(files.getByRole('button', 'Open Image 2, a.png')).toBeVisible();
});

// Task #402 (SPEC-420, #1065): while an image is read, its tile shows a shimmer and a turning ring
// with its number, never the browser's broken-image icon and the file name as alt text; an
// `Image N` placeholder clicked meanwhile turns its icon into a ring until the preview opens.
test('an image being read shows an animated loading tile, not a broken image', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/t/12', '&hold=/api/tasks/tsk_studio_12/prompt/attachments/0'));
  await expect(screen.getByRole('heading', { level: 1 })).toHaveText('#12 Checkout API');
  const drafts = '/srv/yulius/sandboxes/sb_studio/attachments/drafts/usr_mkarp';
  await touchTask(browser, 'sb_studio', 12, { prompt: `See @'${drafts}/a.png' here.` });

  await browser.locator('.taskview .stepstrip-main [aria-label="Task actions"]').click();
  await screen.getByRole('menuitem', 'See the original prompt').click();
  const dialog = screen.getByRole('dialog', 'Original prompt · #12');
  const files = dialog.getByRole('list', 'Attached files');
  const tile = files.getByRole('button', 'Open Image 1, a.png');
  await expect(tile).toBeVisible();
  // Busy, numbered, a turning ring and no picture yet: nothing for the browser to draw as broken.
  const loading = () => browser.evaluate(() => {
    const thumb = document.querySelector('.prompt-modal .prompt-files .prompt-thumb')!;
    const ring = thumb.querySelector('.prompt-thumb-spinner');
    return {
      busy: thumb.getAttribute('aria-busy'),
      number: thumb.querySelector('.composer-attachment-number')?.textContent ?? null,
      imgs: thumb.querySelectorAll('img').length,
      ring: ring ? getComputedStyle(ring).animationName : null,
      shimmer: getComputedStyle(thumb).animationName,
    };
  });
  expect(await loading()).toEqual({ busy: 'true', number: '1', imgs: 0, ring: 'spin', shimmer: 'shimmer' });
  expect(await axeViolations(browser)).toEqual([]);

  // The placeholder in the text, clicked meanwhile, is busy with a turning ring.
  const text = browser.locator('.prompt-modal .prompt-text');
  await text.getByRole('button', { name: 'Open Image 1, a.png' }).click();
  const atom = () => browser.evaluate(() => {
    const el = document.querySelector('.prompt-modal .prompt-text [aria-label="Open Image 1, a.png"]')!;
    const ring = el.querySelector('.pi-spinner');
    return { busy: el.getAttribute('aria-busy'), ring: ring ? getComputedStyle(ring).animationName : null, icon: el.querySelectorAll('svg').length };
  });
  await expect.poll(atom).toEqual({ busy: 'true', ring: 'spin', icon: 0 });

  type Mock = { __yuliusMock: { release: () => void } };
  await browser.evaluate(() => { (window as unknown as Mock).__yuliusMock.release(); });
  const preview = screen.getByRole('dialog', 'a.png');
  await expect(preview.getByRole('img', 'a.png')).toBeVisible();
  await browser.keyboard.press('Escape');
  await expect(preview).toBeHidden();
  // The picture is drawn and nothing is busy any more.
  await expect(files.getByRole('img', 'a.png')).toBeVisible();
  expect(await loading()).toEqual({ busy: null, number: '1', imgs: 1, ring: null, shimmer: 'none' });
  expect(await atom()).toEqual({ busy: null, ring: null, icon: 1 });
});

// SPEC-420 PIL-003: a file named as an image that the browser cannot draw says so, as a failed
// read does, instead of a broken image.
test('an image the browser cannot draw says it could not be loaded', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/t/12'));
  await expect(screen.getByRole('heading', { level: 1 })).toHaveText('#12 Checkout API');
  const drafts = '/srv/yulius/sandboxes/sb_studio/attachments/drafts/usr_mkarp';
  // The mock serves only `.png` as a picture: this `.jpg` is text.
  await touchTask(browser, 'sb_studio', 12, { prompt: `Broken @'${drafts}/broken.jpg' and @'${drafts}/a.png'.` });
  // The page's object URLs still alive: a failed decode gives its blob back (review round 1).
  await browser.evaluate(() => {
    const live = new Set<string>();
    (window as unknown as { __liveUrls: Set<string> }).__liveUrls = live;
    const create = URL.createObjectURL.bind(URL);
    const revoke = URL.revokeObjectURL.bind(URL);
    URL.createObjectURL = (blob: Blob) => { const url = create(blob); live.add(url); return url; };
    URL.revokeObjectURL = (url: string) => { live.delete(url); revoke(url); };
  });
  const liveUrls = () => browser.evaluate(() => (window as unknown as { __liveUrls: Set<string> }).__liveUrls.size);

  await browser.locator('.taskview .stepstrip-main [aria-label="Task actions"]').click();
  await screen.getByRole('menuitem', 'See the original prompt').click();
  const files = screen.getByRole('dialog', 'Original prompt · #12').getByRole('list', 'Attached files');
  await expect(files.getByRole('img', 'a.png')).toBeVisible();
  await expect(files.getByRole('listitem').filter({ hasText: 'broken.jpg' })).toHaveText(/broken\.jpg\s*Couldn’t load it/);
  await expect(files.getByRole('img', 'broken.jpg')).toHaveCount(0);
  expect(await liveUrls()).toBe(1);
  // Its placeholder in the text says so too, in words that are the same in every browser, opens
  // no broken preview and is no longer busy.
  await browser.locator('.prompt-modal .prompt-text').getByRole('button', { name: 'Open Image 1, broken.jpg' }).click();
  await expect(browser.locator('.toast-msg:has-text("Couldn’t open broken.jpg: it is not a picture the browser can show.")')).toBeVisible();
  await expect(screen.getByRole('dialog', 'broken.jpg')).toHaveCount(0);
  await expect.poll(() => browser.evaluate(() => {
    const el = document.querySelector('.prompt-modal .prompt-text [aria-label="Open Image 1, broken.jpg"]')!;
    return { busy: el.getAttribute('aria-busy'), ring: el.querySelectorAll('.pi-spinner').length, icon: el.querySelectorAll('svg').length };
  })).toEqual({ busy: null, ring: 0, icon: 1 });
  expect(await liveUrls()).toBe(1);
});

// SPEC-420 review round 2: the dialog closed while a picture is decoded keeps nothing of it (its
// object URL is revoked) and opens no preview afterwards.
test('a picture decoded after the dialog closed is dropped, not opened', async ({ app, browser, screen }) => {
  // Without the browser's sign-in request 5 s after the start (`browserask=0`): its toast is not
  // this test's, and a slow machine reaches it before the end (#1120 CI run 2beeaf29).
  await app.open(mock('/s/studio/t/12', '&browserask=0'));
  await expect(screen.getByRole('heading', { level: 1 })).toHaveText('#12 Checkout API');
  const drafts = '/srv/yulius/sandboxes/sb_studio/attachments/drafts/usr_mkarp';
  await touchTask(browser, 'sb_studio', 12, { prompt: `See @'${drafts}/a.png' here.` });
  type Probe = { __liveUrls: Set<string>; __decoded: number; __releaseDecode: () => void };
  // Live object URLs, and decodes held until the test releases them.
  await browser.evaluate(() => {
    const w = window as unknown as Probe;
    const live = new Set<string>();
    w.__liveUrls = live;
    w.__decoded = 0;
    const create = URL.createObjectURL.bind(URL);
    const revoke = URL.revokeObjectURL.bind(URL);
    URL.createObjectURL = (blob: Blob) => { const url = create(blob); live.add(url); return url; };
    URL.revokeObjectURL = (url: string) => { live.delete(url); revoke(url); };
    const held = new Promise<void>((resolve) => { w.__releaseDecode = resolve; });
    const decode = HTMLImageElement.prototype.decode;
    HTMLImageElement.prototype.decode = async function heldDecode(this: HTMLImageElement) {
      await held;
      try { return await decode.call(this); } finally { w.__decoded += 1; }
    };
  });

  await browser.locator('.taskview .stepstrip-main [aria-label="Task actions"]').click();
  await screen.getByRole('menuitem', 'See the original prompt').click();
  const dialog = screen.getByRole('dialog', 'Original prompt · #12');
  await browser.locator('.prompt-modal .prompt-text').getByRole('button', { name: 'Open Image 1, a.png' }).click();
  // Read, and held in its decode.
  await expect.poll(() => browser.evaluate(() => (window as unknown as Probe).__liveUrls.size)).toBe(1);
  await browser.keyboard.press('Escape');
  await expect(dialog).toBeHidden();

  await browser.evaluate(() => { (window as unknown as Probe).__releaseDecode(); });
  await expect.poll(() => browser.evaluate(() => (window as unknown as Probe).__decoded)).toBe(1);
  await expect.poll(() => browser.evaluate(() => (window as unknown as Probe).__liveUrls.size)).toBe(0);
  await expect(screen.getByRole('dialog', 'a.png')).toHaveCount(0);
  await expect(browser.locator('.toast-msg')).toHaveCount(0);
});
