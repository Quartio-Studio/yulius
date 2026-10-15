import { test, type Browser } from '@e2e-dev/web';
import { expect } from 'e2e';
import { axeViolations, failOnPageErrors, mock, promptText } from './support.ts';

failOnPageErrors();

// SPEC-124 (task #105): "People often write 'this part [image] looks wrong, while here [image] ...'
// and the image's place in the text is the context." A file pasted or dropped into the new-task
// prompt sits in the text where it was put, moves and deletes like a character, and reaches the
// task at that place.

type Mocked = Window & { __yuliusMock: { tasks: { number: number; sandbox_id: string; prompt: string }[] } };

const DRAFTS = '/var/lib/yulius/sandboxes/sb_studio/attachments/drafts/usr_1';

/** Pastes an image file into the prompt, as a screenshot pasted from the clipboard. */
async function pasteImage(browser: Browser, name: string): Promise<void> {
  await browser.evaluate((file) => {
    const data = new DataTransfer();
    data.items.add(new File(['png'], file, { type: 'image/png' }));
    document.querySelector('.composer-prompt')!.dispatchEvent(new ClipboardEvent('paste', { clipboardData: data, bubbles: true, cancelable: true }));
  }, name);
}

/** The prompt the last task of the studio sandbox was created with. */
function lastPrompt(browser: Browser): Promise<string> {
  return browser.evaluate(() => {
    const tasks = (window as unknown as Mocked).__yuliusMock.tasks.filter((t) => t.sandbox_id === 'sb_studio');
    return tasks.sort((a, b) => b.number - a.number)[0].prompt;
  });
}

test('an image pasted mid-text stays there, deletes and comes back, moves, and reaches the task in place', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio'));
  const prompt = screen.getByRole('textbox', 'Task prompt');
  await prompt.click();
  await browser.keyboard.type('This part ');
  await pasteImage(browser, 'shot.png');
  await expect(browser.locator('.composer-prompt .pi-image')).toHaveCount(1);
  // SPEC-150: the upload shows in the tray above the text; the text holds its placeholder.
  await expect(browser.locator('.composer-attachments .composer-attachment')).toHaveCount(1);
  await browser.keyboard.type('looks wrong, while the header is fine.');
  expect(await promptText(browser)).toBe('This part [shot.png]looks wrong, while the header is fine.');
  expect(await axeViolations(browser)).toEqual([]);

  // One Backspace takes the whole image; undo brings it back where it was.
  for (let i = 0; i < 'looks wrong, while the header is fine.'.length; i += 1) await browser.keyboard.press('ArrowLeft');
  await browser.keyboard.press('Backspace');
  await expect(browser.locator('.composer-prompt .pi-atom')).toHaveCount(0);
  expect(await promptText(browser)).toBe('This part looks wrong, while the header is fine.');
  await browser.keyboard.press('ControlOrMeta+z');
  await expect(browser.locator('.composer-prompt .pi-image')).toHaveCount(1);
  expect(await promptText(browser)).toBe('This part [shot.png]looks wrong, while the header is fine.');
  await browser.keyboard.press('ControlOrMeta+Shift+z');
  await expect(browser.locator('.composer-prompt .pi-atom')).toHaveCount(0);
  await browser.keyboard.press('ControlOrMeta+z');
  await expect(browser.locator('.composer-prompt .pi-atom')).toHaveCount(1);

  // The keyboard way to move it: select it, cut, paste where it belongs.
  await browser.keyboard.press('Home');
  for (let i = 0; i < 'This part '.length; i += 1) await browser.keyboard.press('ArrowRight');
  await browser.keyboard.press('Shift+ArrowRight');
  await browser.keyboard.press('ControlOrMeta+x');
  await expect(browser.locator('.composer-prompt .pi-atom')).toHaveCount(0);
  for (let i = 0; i < 'looks wrong, while the header'.length; i += 1) await browser.keyboard.press('ArrowRight');
  await browser.keyboard.press('ControlOrMeta+v');
  await expect(browser.locator('.composer-prompt .pi-image')).toHaveCount(1);
  expect(await promptText(browser)).toBe('This part looks wrong, while the header[shot.png] is fine.');

  await browser.keyboard.press('ControlOrMeta+Enter');
  await expect(screen.getByRole('heading', { level: 1 })).toHaveText(/This part looks wrong, while the header is fine\./);
  // The task got the image's mention exactly where it was moved to.
  expect(await lastPrompt(browser)).toBe(`This part looks wrong, while the header@'${DRAFTS}/shot.png' is fine.`);

  // The original prompt shows it there too.
  await browser.locator('.taskview .stepstrip-main [aria-label="Task actions"]').click();
  await screen.getByRole('menuitem', 'See the original prompt').click();
  await expect(browser.locator('.prompt-modal .prompt-text').getByRole('img', { name: 'shot.png' })).toBeVisible();
  const inView = await browser.evaluate(() => {
    const copy = document.querySelector('.prompt-modal .prompt-text')!.cloneNode(true) as HTMLElement;
    for (const img of copy.querySelectorAll('img')) img.replaceWith(`[${img.getAttribute('alt')}]`);
    return copy.textContent;
  });
  expect(inView).toMatch(/while the header\s*\[shot\.png\]\s*is fine\./);
});

test('an image dragged to another place in the text lands at the drop point', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio'));
  await screen.getByRole('textbox', 'Task prompt').click();
  await browser.keyboard.type('Before the image, ');
  await pasteImage(browser, 'drag.png');
  await expect(browser.locator('.composer-prompt .pi-image')).toHaveCount(1);
  await browser.keyboard.type(' after it.');

  // Dragged onto the space after "Before": the browser's own drag events, as a mouse makes them.
  await browser.evaluate(() => {
    const input = document.querySelector('.composer-prompt')!;
    const atom = input.querySelector('.pi-atom')!;
    const text = [...input.childNodes].find((n) => n.nodeType === 3 && (n as Text).data.startsWith('Before'))!;
    const range = document.createRange();
    range.setStart(text, 'Before'.length);
    range.setEnd(text, 'Before'.length + 1);
    const box = range.getBoundingClientRect();
    const data = new DataTransfer();
    const at = { clientX: box.left + 1, clientY: box.top + box.height / 2 };
    atom.dispatchEvent(new DragEvent('dragstart', { bubbles: true, cancelable: true, dataTransfer: data }));
    input.dispatchEvent(new DragEvent('dragover', { bubbles: true, cancelable: true, dataTransfer: data, ...at }));
    input.dispatchEvent(new DragEvent('drop', { bubbles: true, cancelable: true, dataTransfer: data, ...at }));
    atom.dispatchEvent(new DragEvent('dragend', { bubbles: true, dataTransfer: data }));
  });
  expect(await promptText(browser)).toBe('Before[drag.png] the image,  after it.');
  // A move is one undo step.
  await browser.keyboard.press('ControlOrMeta+z');
  expect(await promptText(browser)).toBe('Before the image, [drag.png] after it.');
});

// #535 (SPEC-166): a press selects a placeholder (so Delete, cut or a drag takes it) and a drop
// selects it where it landed; it looks chosen as a whole, and its label is never painted as
// selected text. A real mouse: Chromium's own press, drag and drop.
test('a placeholder pressed or dragged with the mouse looks selected as a whole, its label never as selected text', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio'));
  await screen.getByRole('textbox', 'Task prompt').click();
  await browser.keyboard.type('Before the image, ');
  await pasteImage(browser, 'drag.png');
  await expect(browser.locator('.composer-prompt .pi-image')).toHaveCount(1);
  await browser.keyboard.type(' after it.');
  const look = () => browser.evaluate(() => {
    const atom = document.querySelector('.composer-prompt .pi-atom')!;
    return { selected: atom.classList.contains('pi-selected'), paint: getComputedStyle(atom.querySelector('.pi-file-name')!, '::selection').backgroundColor };
  });
  const chosen = { selected: true, paint: 'rgba(0, 0, 0, 0)' };
  const points = () => browser.evaluate(() => {
    const input = document.querySelector('.composer-prompt')!;
    const atom = input.querySelector('.pi-atom')!.getBoundingClientRect();
    const text = [...input.childNodes].find((n) => n.nodeType === 3 && (n as Text).data.length > 1)! as Text;
    const range = document.createRange();
    range.setStart(text, 0);
    range.setEnd(text, 1);
    const first = range.getBoundingClientRect();
    const end = range.cloneRange();
    end.setStart(text, text.data.length - 1);
    end.setEnd(text, text.data.length);
    const last = end.getBoundingClientRect();
    return { atom: { x: atom.left + atom.width / 2, y: atom.top + atom.height / 2 }, start: { x: first.left + 1, y: first.top + first.height / 2 }, end: { x: last.right - 1, y: last.top + last.height / 2 } };
  });

  const from = await points();
  await browser.mouse.move(from.atom.x, from.atom.y);
  await browser.mouse.down();
  await expect.poll(look).toEqual(chosen);
  for (let i = 1; i <= 8; i += 1) await browser.mouse.move(from.atom.x + ((from.start.x - from.atom.x) * i) / 8, from.start.y);
  await browser.mouse.up();
  await expect.poll(() => promptText(browser)).toBe('[drag.png]Before the image,  after it.');
  await expect.poll(look).toEqual(chosen);

  // A click in the text leaves it; a press chooses it again, and Delete takes it.
  const to = await points();
  await browser.mouse.click(to.end.x, to.end.y);
  await expect.poll(look).toEqual({ ...chosen, selected: false });
  await browser.mouse.click(to.atom.x, to.atom.y);
  await expect.poll(look).toEqual(chosen);
  await browser.keyboard.press('Delete');
  await expect(browser.locator('.composer-prompt .pi-atom')).toHaveCount(0);
  expect(await promptText(browser)).toBe('Before the image,  after it.');
});

test('a draft keeps each image in its place across a reload; an older draft shows its files in the tray', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio'));
  await screen.getByRole('textbox', 'Task prompt').click();
  await browser.keyboard.type('Here ');
  await pasteImage(browser, 'kept.png');
  await expect(browser.locator('.composer-prompt .pi-atom')).toHaveCount(1);
  await browser.keyboard.type(' and there.');
  await expect.poll(() => browser.evaluate(() => {
    const key = Object.keys(localStorage).find((k) => k.startsWith('yulius.draft.') && k.endsWith('.sb_studio'));
    return key ? JSON.parse(localStorage.getItem(key)!).text : '';
  })).toBe(`Here @'${DRAFTS}/kept.png' and there.`);
  await browser.reload();
  await expect(browser.locator('.composer-prompt .pi-atom')).toHaveCount(1);
  expect(await promptText(browser)).toBe('Here [kept.png] and there.');

  // Stored before this change: the files only in `attached`.
  await browser.evaluate((drafts) => {
    const key = Object.keys(localStorage).find((k) => k.startsWith('yulius.draft.') && k.endsWith('.sb_studio'))!;
    localStorage.setItem(key, JSON.stringify({ text: 'Older words', attached: [{ path: `${drafts}/old.png`, name: 'old.png', type: 'image/png', at: Date.now() }] }));
  }, DRAFTS);
  await browser.reload();
  // SPEC-150: they show in the tray, not in the text, and are sent after it.
  await expect(browser.locator('.composer-attachments .composer-attachment')).toHaveCount(1);
  await expect(browser.locator('.composer-prompt .pi-atom')).toHaveCount(0);
  expect(await promptText(browser)).toBe('Older words');
});

test('a file dropped on the prompt lands at the drop point, and the prompt fits a phone', async ({ app, browser, screen }) => {
  await browser.setViewport({ width: 360, height: 800 });
  await app.open(mock('/s/studio'));
  await screen.getByRole('textbox', 'Task prompt').click();
  await browser.keyboard.type('Start of the text, end of it.');
  await browser.evaluate(() => {
    const input = document.querySelector('.composer-prompt')!;
    const text = input.firstChild as Text;
    const range = document.createRange();
    range.setStart(text, 'Start'.length);
    range.setEnd(text, 'Start'.length + 1);
    const box = range.getBoundingClientRect();
    const data = new DataTransfer();
    data.items.add(new File(['x'.repeat(4000)], 'a-very-long-screenshot-name-that-would-not-fit-a-phone.png', { type: 'image/png' }));
    data.items.add(new File(['notes'], 'notes.txt', { type: 'text/plain' }));
    const at = { clientX: box.left + 1, clientY: box.top + box.height / 2 };
    for (const type of ['dragenter', 'dragover', 'drop']) document.querySelector('.composer-box')!.dispatchEvent(new DragEvent(type, { bubbles: true, cancelable: true, dataTransfer: data, ...at }));
  });
  await expect(browser.locator('.composer-prompt .pi-atom:not(.pi-pending)')).toHaveCount(2);
  expect(await promptText(browser)).toBe('Start[a-very-long-screenshot-name-that-would-not-fit-a-phone.png][notes.txt] of the text, end of it.');
  const fits = await browser.evaluate(() => {
    const box = document.querySelector('.composer-box')!.getBoundingClientRect();
    const atoms = [...document.querySelectorAll('.composer-prompt .pi-atom')].map((a) => a.getBoundingClientRect());
    return {
      page: document.documentElement.scrollWidth <= document.documentElement.clientWidth,
      atoms: atoms.every((a) => a.left >= box.left - 1 && a.right <= box.right + 1),
    };
  });
  expect(fits).toEqual({ page: true, atoms: true });
});

// SPEC-124 review: an upload's placeholder never comes back through undo; the insert is one step.
test('undo right after a paste, or after typing while it uploaded, never shows the placeholder again', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio'));
  await screen.getByRole('textbox', 'Task prompt').click();
  await browser.keyboard.type('One ');
  await pasteImage(browser, 'a.png');
  await expect(browser.locator('.composer-prompt .pi-image')).toHaveCount(1);
  await browser.keyboard.press('ControlOrMeta+z');
  await expect(browser.locator('.composer-prompt .pi-atom')).toHaveCount(0);
  await browser.keyboard.press('ControlOrMeta+Shift+z');
  await expect(browser.locator('.composer-prompt .pi-image')).toHaveCount(1);
  await expect(browser.locator('.composer-prompt .pi-pending')).toHaveCount(0);

  // Typed while the next one uploads: undoing the typing keeps the file, never the placeholder.
  await browser.evaluate(() => {
    const w = window as unknown as Window & { __yuliusMock: { holdUpload?: Promise<void> }; __release?: () => void };
    w.__yuliusMock.holdUpload = new Promise<void>((resolve) => { w.__release = resolve; });
  });
  await pasteImage(browser, 'b.png');
  await expect(browser.locator('.composer-prompt .pi-pending')).toHaveCount(1);
  await browser.keyboard.type(' two three');
  await browser.evaluate(() => (window as unknown as { __release: () => void }).__release());
  await expect(browser.locator('.composer-prompt .pi-image')).toHaveCount(2);
  expect(await promptText(browser)).toBe('One [a.png][b.png] two three');
  // Undo walks the typing back word by word to the inserted file, never to a placeholder…
  for (let i = 0; i < 6 && (await promptText(browser)) !== 'One [a.png][b.png]'; i += 1) {
    await browser.keyboard.press('ControlOrMeta+z');
    await expect(browser.locator('.composer-prompt .pi-pending')).toHaveCount(0);
  }
  expect(await promptText(browser)).toBe('One [a.png][b.png]');
  // …and one more step takes the file.
  await browser.keyboard.press('ControlOrMeta+z');
  await expect(browser.locator('.composer-prompt .pi-pending')).toHaveCount(0);
  expect(await promptText(browser)).toBe('One [a.png]');
});

test('a selection dragged inside the prompt moves, it is not copied', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio'));
  await screen.getByRole('textbox', 'Task prompt').fill('alpha beta gamma');
  await browser.evaluate(() => {
    const input = document.querySelector('.composer-prompt')!;
    const text = input.firstChild as Text;
    const range = document.createRange();
    range.setStart(text, 5);
    range.setEnd(text, 10);
    window.getSelection()!.removeAllRanges();
    window.getSelection()!.addRange(range);
    const end = document.createRange();
    end.setStart(text, 15);
    end.setEnd(text, 16);
    const box = end.getBoundingClientRect();
    const at = { clientX: box.right, clientY: box.top + box.height / 2 };
    const data = new DataTransfer();
    text.dispatchEvent(new DragEvent('dragstart', { bubbles: true, cancelable: true, dataTransfer: data }));
    input.dispatchEvent(new DragEvent('dragover', { bubbles: true, cancelable: true, dataTransfer: data, ...at }));
    input.dispatchEvent(new DragEvent('drop', { bubbles: true, cancelable: true, dataTransfer: data, ...at }));
    input.dispatchEvent(new DragEvent('dragend', { bubbles: true, dataTransfer: data }));
  });
  expect(await promptText(browser)).toBe('alpha gamma beta');
});

// SPEC-124 review: the prompt's length cap stops typing and pastes, never an uploaded file.
test('at the length cap typing stops, and a pasted image still lands', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio'));
  const prompt = screen.getByRole('textbox', 'Task prompt');
  await prompt.fill('x'.repeat(100000));
  await prompt.press('End');
  await browser.keyboard.type('yz');
  await expect.poll(async () => (await promptText(browser)).length).toBe(100000);
  await pasteImage(browser, 'capped.png');
  await expect(browser.locator('.composer-prompt .pi-image')).toHaveCount(1);
  await expect(browser.locator('.composer-prompt .pi-pending')).toHaveCount(0);
});

type Holding = Window & { __yuliusMock: { holdUpload?: Promise<void>; failUpload?: boolean; tasks: unknown[] }; __release?: () => void; __fail?: () => void };

/** Holds the next uploads until `__release()`, or fails them with `__fail()`. */
async function holdUploads(browser: Browser): Promise<void> {
  await browser.evaluate(() => {
    const w = window as unknown as Holding;
    w.__yuliusMock.failUpload = false;
    w.__yuliusMock.holdUpload = new Promise<void>((resolve) => {
      w.__release = resolve;
      w.__fail = () => { w.__yuliusMock.failUpload = true; resolve(); };
    });
  });
}

// SPEC-124 review round 3: a failed upload leaves no undo step behind.
test('a failed upload leaves no empty undo step, typed text around it included', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio'));
  await screen.getByRole('textbox', 'Task prompt').click();
  await browser.keyboard.type('alpha beta');
  await holdUploads(browser);
  await pasteImage(browser, 'lost.png');
  await expect(browser.locator('.composer-prompt .pi-pending')).toHaveCount(1);
  await browser.evaluate(() => (window as unknown as Holding).__fail!());
  await expect(browser.locator('.composer-prompt .pi-pending')).toHaveCount(0);
  await browser.keyboard.press('ControlOrMeta+z');
  await expect.poll(() => promptText(browser)).not.toBe('alpha beta');

  await screen.getByRole('textbox', 'Task prompt').fill('');
  await screen.getByRole('textbox', 'Task prompt').click();
  await browser.keyboard.type('One');
  await holdUploads(browser);
  await pasteImage(browser, 'lost.png');
  await expect(browser.locator('.composer-prompt .pi-pending')).toHaveCount(1);
  await browser.keyboard.type(' more');
  await browser.evaluate(() => (window as unknown as Holding).__fail!());
  await expect(browser.locator('.composer-prompt .pi-pending')).toHaveCount(0);
  expect(await promptText(browser)).toBe('One more');
  // Every undo changes the text; redo brings it all back.
  let before = await promptText(browser);
  for (let i = 0; i < 3 && before; i += 1) {
    await browser.keyboard.press('ControlOrMeta+z');
    const now = await promptText(browser);
    expect(now).not.toBe(before);
    before = now;
  }
  for (let i = 0; i < 3; i += 1) await browser.keyboard.press('ControlOrMeta+Shift+z');
  expect(await promptText(browser)).toBe('One more');
});

// SPEC-124 review round 3: a drag that ended elsewhere, or whose text was redrawn, is no move.
test('a drag that ended off the prompt, or outlived a redraw, moves no stale range', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio'));
  await screen.getByRole('textbox', 'Task prompt').fill('alpha beta gamma');
  const dragOut = (redraw: boolean) => browser.evaluate((mid) => {
    const input = document.querySelector('.composer-prompt')!;
    const text = input.firstChild as Text;
    const range = document.createRange();
    range.setStart(text, 0);
    range.setEnd(text, 5);
    window.getSelection()!.removeAllRanges();
    window.getSelection()!.addRange(range);
    const data = new DataTransfer();
    text.dispatchEvent(new DragEvent('dragstart', { bubbles: true, cancelable: true, dataTransfer: data }));
    if (mid) {
      // The text changes under the drag (an upload ending): its saved range no longer holds.
      const caret = document.createRange();
      caret.setStart(text, text.data.length);
      window.getSelection()!.removeAllRanges();
      window.getSelection()!.addRange(caret);
      input.dispatchEvent(new InputEvent('beforeinput', { inputType: 'insertParagraph', bubbles: true, cancelable: true }));
    } else {
      document.body.dispatchEvent(new DragEvent('dragend', { bubbles: true, dataTransfer: data }));
    }
    const last = input.lastChild!.previousSibling as Text;
    const end = document.createRange();
    end.setStart(last, last.data.length - 1);
    end.setEnd(last, last.data.length);
    const box = end.getBoundingClientRect();
    const outside = new DataTransfer();
    outside.setData('text/plain', 'X');
    const at = { clientX: box.right, clientY: box.top + box.height / 2 };
    input.dispatchEvent(new DragEvent('dragover', { bubbles: true, cancelable: true, dataTransfer: outside, ...at }));
    input.dispatchEvent(new DragEvent('drop', { bubbles: true, cancelable: true, dataTransfer: outside, ...at }));
  }, redraw);
  await dragOut(false);
  // The drop brought text from outside: it is inserted, nothing of the old selection moved.
  expect(await promptText(browser)).toBe('alpha beta gammaX');
  await dragOut(true);
  expect((await promptText(browser)).replace(/\n/g, '')).toBe('alpha beta gammaXX');
});

// SPEC-124 review round 4: a placeholder deleted before its upload fails leaves no empty steps.
test('a placeholder deleted before its upload fails leaves no empty undo steps', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio'));
  await screen.getByRole('textbox', 'Task prompt').click();
  await browser.keyboard.type('One ');
  await holdUploads(browser);
  await pasteImage(browser, 'gone.png');
  await expect(browser.locator('.composer-prompt .pi-pending')).toHaveCount(1);
  await browser.keyboard.press('Backspace');
  await expect(browser.locator('.composer-prompt .pi-pending')).toHaveCount(0);
  await browser.keyboard.type('two');
  await browser.evaluate(() => (window as unknown as Holding).__fail!());
  // The upload has ended (it failed) before the undo.
  await expect(browser.locator('.composer-box .upload-status-error')).toHaveCount(1);
  expect(await promptText(browser)).toBe('One two');
  // Every undo changes the text until it is empty, and the placeholder never comes back.
  let before = await promptText(browser);
  for (let i = 0; i < 6 && before; i += 1) {
    await browser.keyboard.press('ControlOrMeta+z');
    await expect(browser.locator('.composer-prompt .pi-pending')).toHaveCount(0);
    const now = await promptText(browser);
    expect(now).not.toBe(before);
    before = now;
  }
  expect(before).toBe('');
});

// SPEC-150 (task #144): "i'd expect uploading images and then positioning placeholders. Moreover
// i still want to zoom the image". Uploads sit in a tray above the text, where an image opens
// large; the text holds a one-line placeholder per file, which the person positions.

/** The tray's files, as their tiles' names, in order. */
const trayNames = (browser: Browser) => browser.evaluate(() => [...document.querySelectorAll('.composer-attachments .composer-attachment-name')].map((name) => name.textContent));

test('an uploaded image shows in the tray, opens large from there, and sits in the text as a one-line placeholder', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio'));
  await screen.getByRole('textbox', 'Task prompt').click();
  await browser.keyboard.type('This part ');
  await browser.keyboard.press('Enter');
  await browser.keyboard.type('next');
  await browser.keyboard.press('ArrowUp');
  await browser.keyboard.press('End');
  /** Where the second line starts: it moves down when the first line grows. */
  const secondLine = () => browser.evaluate(() => {
    const input = document.querySelector('.composer-prompt')!;
    const text = [...input.childNodes].find((n) => n.nodeType === 3 && (n as Text).data.includes('next')) as Text;
    const range = document.createRange();
    range.setStart(text, text.data.indexOf('next'));
    range.setEnd(text, text.data.indexOf('next') + 1);
    return Math.round(range.getBoundingClientRect().top - input.getBoundingClientRect().top);
  });
  const textOnly = await secondLine();
  await pasteImage(browser, 'shot.png');
  await expect(browser.locator('ul.composer-attachments[aria-label="Attached files"] > li')).toHaveCount(1);
  // The placeholder leaves the line as high as the text alone, and its tooltip is the file's name.
  expect(await secondLine()).toBe(textOnly);
  expect(await browser.evaluate(() => document.querySelector('.composer-prompt .pi-atom')!.getAttribute('title'))).toBe('shot.png');
  await browser.keyboard.type(' looks wrong, and ');
  await pasteImage(browser, 'other.png');
  await expect.poll(() => trayNames(browser)).toEqual(['shot.png', 'other.png']);
  // The tray shows each image, numbered; the text holds "Image N", never the picture.
  await expect(browser.locator('.composer-attachments img')).toHaveCount(2);
  await expect(browser.locator('.composer-attachment-number')).toHaveText(['1', '2']);
  await expect(browser.locator('.composer-prompt img')).toHaveCount(0);
  await expect(browser.locator('.composer-prompt .pi-image')).toHaveText(['Image 1', 'Image 2']);
  expect(await promptText(browser)).toBe('This part [shot.png] looks wrong, and [other.png]\nnext');
  expect(await axeViolations(browser)).toEqual([]);

  // A double-click on the placeholder opens the file.
  await browser.evaluate(() => document.querySelector('.composer-prompt .pi-atom')!.dispatchEvent(new MouseEvent('dblclick', { bubbles: true, detail: 2 })));
  await expect(screen.getByRole('dialog', 'shot.png').getByRole('img', 'shot.png')).toBeVisible();
  await browser.locator('dialog.modal').press('Escape');
  await expect(browser.locator('dialog.modal')).toHaveCount(0);
  // A click on the thumbnail opens it large; its name says which placeholder it is.
  await screen.getByRole('button', 'Open Image 1, shot.png').click();
  await expect(screen.getByRole('dialog', 'shot.png').getByRole('img', 'shot.png')).toBeVisible();
  await browser.locator('dialog.modal').press('Escape');
  await expect(browser.locator('dialog.modal')).toHaveCount(0);
});

test('× takes a file out of the tray and the text in one undo step; a deleted placeholder keeps the file, sent after the text', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio'));
  const prompt = screen.getByRole('textbox', 'Task prompt');
  await prompt.click();
  await browser.keyboard.type('Look at ');
  await pasteImage(browser, 'a.png');
  await expect.poll(() => trayNames(browser)).toEqual(['a.png']);
  await browser.keyboard.type(' here');

  await screen.getByRole('button', 'Remove a.png').click();
  await expect.poll(() => trayNames(browser)).toEqual([]);
  expect(await promptText(browser)).toBe('Look at  here');
  await prompt.click();
  await browser.keyboard.press('ControlOrMeta+z');
  await expect.poll(() => trayNames(browser)).toEqual(['a.png']);
  expect(await promptText(browser)).toBe('Look at [a.png] here');

  // Deleting the placeholder leaves the file in the tray, marked as not placed.
  await prompt.press('End');
  for (let i = 0; i < ' here'.length; i += 1) await prompt.press('ArrowLeft');
  await prompt.press('Backspace');
  await expect(browser.locator('.composer-prompt .pi-atom')).toHaveCount(0);
  await expect.poll(() => trayNames(browser)).toEqual(['a.png']);
  await expect(browser.locator('.composer-attachment.unplaced')).toHaveCount(1);
  await expect(browser.locator('.composer-attachment.unplaced .sr-only')).toHaveText('Not in the text: sent after it.');
  // The Preview shows it too, after the formatted text, as it will be sent.
  await screen.getByRole('button', 'Preview').click();
  await expect(browser.locator('.composer-preview .pi-preview-after').getByRole('button', { name: 'Open a.png' })).toBeVisible();
  await screen.getByRole('button', 'Preview').click();
  await prompt.press('ControlOrMeta+Enter');
  await expect(browser).toHaveURL(/#\/s\/studio\/t\/\d+$/);
  // Sent after the text, as older prompts were: the agent still gets it.
  expect(await lastPrompt(browser)).toBe(`Look at  here\n\n@${DRAFTS}/a.png`);
});

test('a tray thumbnail dragged into the text puts its placeholder there; the keyboard way puts it at the caret', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio'));
  const prompt = screen.getByRole('textbox', 'Task prompt');
  await prompt.click();
  await browser.keyboard.type('Before the image, ');
  await pasteImage(browser, 'drag.png');
  await expect.poll(() => trayNames(browser)).toEqual(['drag.png']);
  await browser.keyboard.type(' after it.');

  /** Drags the tray tile onto the space after "Before", as a mouse does. */
  const dragTile = () => browser.evaluate(() => {
    const input = document.querySelector('.composer-prompt')!;
    const tile = document.querySelector('.composer-attachments .composer-attachment')!;
    const text = [...input.childNodes].find((n) => n.nodeType === 3 && (n as Text).data.startsWith('Before'))!;
    const range = document.createRange();
    range.setStart(text, 'Before'.length);
    range.setEnd(text, 'Before'.length + 1);
    const box = range.getBoundingClientRect();
    const data = new DataTransfer();
    const at = { clientX: box.left + 1, clientY: box.top + box.height / 2 };
    tile.dispatchEvent(new DragEvent('dragstart', { bubbles: true, cancelable: true, dataTransfer: data }));
    input.dispatchEvent(new DragEvent('dragover', { bubbles: true, cancelable: true, dataTransfer: data, ...at }));
    input.dispatchEvent(new DragEvent('drop', { bubbles: true, cancelable: true, dataTransfer: data, ...at }));
    tile.dispatchEvent(new DragEvent('dragend', { bubbles: true, dataTransfer: data }));
  });
  // Placed already: the placeholder moves there, one undo step.
  await dragTile();
  expect(await promptText(browser)).toBe('Before[drag.png] the image,  after it.');
  await browser.keyboard.press('ControlOrMeta+z');
  expect(await promptText(browser)).toBe('Before the image, [drag.png] after it.');

  // Not placed: the drag inserts it.
  await screen.getByRole('textbox', 'Task prompt').click();
  await prompt.press('End');
  for (let i = 0; i < ' after it.'.length; i += 1) await prompt.press('ArrowLeft');
  await prompt.press('Backspace');
  expect(await promptText(browser)).toBe('Before the image,  after it.');
  await dragTile();
  expect(await promptText(browser)).toBe('Before[drag.png] the image,  after it.');

  // The keyboard way: the caret where it belongs, then the tile's button.
  await prompt.press('ControlOrMeta+z');
  await expect(browser.locator('.composer-prompt .pi-atom')).toHaveCount(0);
  await prompt.press('Home');
  await screen.getByRole('button', 'Put drag.png in the text').click();
  expect(await promptText(browser)).toBe('[drag.png]Before the image,  after it.');
  await expect(browser.locator('[aria-label="Put drag.png in the text"]')).toHaveCount(0);
});

test('the tray wraps at phone width and an older draft fills it', async ({ app, browser, screen }) => {
  await browser.setViewport({ width: 360, height: 800 });
  await browser.addInitScript(() => {
    const file = (name: string) => ({ path: `/var/lib/yulius/sandboxes/sb_studio/attachments/drafts/usr_1/${name}`, name, type: 'image/png', at: Date.now() });
    localStorage.setItem('yulius.draft.usr_1.sb_studio', JSON.stringify({ text: 'Older words', attached: ['a-very-long-screenshot-name.png', 'b.png', 'c.png', 'd.png', 'e.png'].map(file) }));
  });
  await app.open(mock('/s/studio'));
  await expect.poll(() => trayNames(browser)).toEqual(['a-very-long-screenshot-name.png', 'b.png', 'c.png', 'd.png', 'e.png']);
  expect(await promptText(browser)).toBe('Older words');
  const fits = await browser.evaluate(() => {
    const box = document.querySelector('.composer-box')!.getBoundingClientRect();
    const tiles = [...document.querySelectorAll('.composer-attachment')].map((a) => a.getBoundingClientRect());
    return { page: document.documentElement.scrollWidth <= document.documentElement.clientWidth, tiles: tiles.every((a) => a.left >= box.left - 1 && a.right <= box.right + 1) };
  });
  expect(fits).toEqual({ page: true, tiles: true });
  expect(await axeViolations(browser)).toEqual([]);
});

// SPEC-150 review round 1: the tray is part of the prompt's undo history.
test('the tray is part of undo: × on a file not in the text comes back, undo keeps the tray order, and a redone × is not sent', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio'));
  const prompt = screen.getByRole('textbox', 'Task prompt');
  await prompt.click();
  await browser.keyboard.type('See ');
  for (const name of ['a.png', 'b.png', 'c.png']) {
    await pasteImage(browser, name);
    await expect(browser.locator('.composer-prompt .pi-atom:not(.pi-pending)')).toHaveCount(['a.png', 'b.png', 'c.png'].indexOf(name) + 1);
  }
  await expect.poll(() => trayNames(browser)).toEqual(['a.png', 'b.png', 'c.png']);

  // c leaves the text, then the tray: undo brings it back to the tray, still not in the text.
  await prompt.press('End');
  await prompt.press('Backspace');
  await expect.poll(() => trayNames(browser)).toEqual(['a.png', 'b.png', 'c.png']);
  await screen.getByRole('button', 'Remove c.png').click();
  await expect.poll(() => trayNames(browser)).toEqual(['a.png', 'b.png']);
  // The keyboard stays in the tray, on the tile now in that place (the last one here).
  expect(await browser.evaluate(() => document.activeElement!.getAttribute('aria-label'))).toBe('Open Image 2, b.png');
  await prompt.click();
  await browser.keyboard.press('ControlOrMeta+z');
  await expect.poll(() => trayNames(browser)).toEqual(['a.png', 'b.png', 'c.png']);
  expect(await promptText(browser)).toBe('See [a.png][b.png]');

  // × a then undo: the same order and numbers as before.
  await screen.getByRole('button', 'Remove a.png').click();
  await expect.poll(() => trayNames(browser)).toEqual(['b.png', 'c.png']);
  await expect(browser.locator('.composer-prompt .pi-image')).toHaveText(['Image 1']);
  await prompt.click();
  await browser.keyboard.press('ControlOrMeta+z');
  await expect.poll(() => trayNames(browser)).toEqual(['a.png', 'b.png', 'c.png']);
  await expect(browser.locator('.composer-prompt .pi-image')).toHaveText(['Image 1', 'Image 2']);
  // Redo takes it out of both again: it is not sent after the text.
  await browser.keyboard.press('ControlOrMeta+Shift+z');
  await expect.poll(() => trayNames(browser)).toEqual(['b.png', 'c.png']);
  await prompt.press('ControlOrMeta+Enter');
  await expect(browser).toHaveURL(/#\/s\/studio\/t\/\d+$/);
  expect(await lastPrompt(browser)).toBe(`See @'${DRAFTS}/b.png'\n\n@${DRAFTS}/c.png`);
});

// SPEC-150 review round 2: an upload that ends after its paste was undone is not attached.
test('an upload ending after its paste was undone stays out of the tray until the paste is redone', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio'));
  const prompt = screen.getByRole('textbox', 'Task prompt');
  await prompt.click();
  await browser.keyboard.type('One ');
  await holdUploads(browser);
  await pasteImage(browser, 'b.png');
  await expect(browser.locator('.composer-prompt .pi-pending')).toHaveCount(1);
  await browser.keyboard.press('ControlOrMeta+z');
  await browser.evaluate(() => (window as unknown as Holding).__release!());
  // The upload has ended: its status says so.
  await expect(browser.locator('.composer-box .upload-status-float')).toHaveText(/Added b\.png/);
  expect(await trayNames(browser)).toEqual([]);
  expect(await promptText(browser)).toBe('One ');
  // Redo brings the paste back, the file with it; undo takes both again, and nothing is sent.
  await browser.keyboard.press('ControlOrMeta+Shift+z');
  await expect.poll(() => trayNames(browser)).toEqual(['b.png']);
  expect(await promptText(browser)).toBe('One [b.png]');
  await browser.keyboard.press('ControlOrMeta+z');
  await expect.poll(() => trayNames(browser)).toEqual([]);
  await prompt.press('ControlOrMeta+Enter');
  await expect(browser).toHaveURL(/#\/s\/studio\/t\/\d+$/);
  expect(await lastPrompt(browser)).toBe('One');
});

// SPEC-150 review round 3: typing after the undo drops the paste's redo step; the upload stays out.
test('an upload ending after its paste was undone and typed over stays out of the tray and is not sent', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio'));
  const prompt = screen.getByRole('textbox', 'Task prompt');
  await prompt.click();
  await browser.keyboard.type('One ');
  await holdUploads(browser);
  await pasteImage(browser, 'b.png');
  await expect(browser.locator('.composer-prompt .pi-pending')).toHaveCount(1);
  await browser.keyboard.press('ControlOrMeta+z');
  await browser.keyboard.type('two');
  await browser.evaluate(() => (window as unknown as Holding).__release!());
  await expect(browser.locator('.composer-box .upload-status-float')).toHaveText(/Added b\.png/);
  expect(await trayNames(browser)).toEqual([]);
  expect(await promptText(browser)).toBe('One two');
  await prompt.press('ControlOrMeta+Enter');
  await expect(browser).toHaveURL(/#\/s\/studio\/t\/\d+$/);
  expect(await lastPrompt(browser)).toBe('One two');
});

test('undoing a paste takes its upload out of the tray, and an upload whose placeholder went first stays in the tray only', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio'));
  await screen.getByRole('textbox', 'Task prompt').click();
  await browser.keyboard.type('One ');
  await pasteImage(browser, 'a.png');
  await expect.poll(() => trayNames(browser)).toEqual(['a.png']);
  await browser.keyboard.press('ControlOrMeta+z');
  await expect.poll(() => trayNames(browser)).toEqual([]);
  expect(await promptText(browser)).toBe('One ');
  await browser.keyboard.press('ControlOrMeta+Shift+z');
  await expect.poll(() => trayNames(browser)).toEqual(['a.png']);
  expect(await promptText(browser)).toBe('One [a.png]');

  await holdUploads(browser);
  await pasteImage(browser, 'b.png');
  await expect(browser.locator('.composer-prompt .pi-pending')).toHaveCount(1);
  await browser.keyboard.press('Backspace');
  await browser.evaluate(() => (window as unknown as Holding).__release!());
  await expect.poll(() => trayNames(browser)).toEqual(['a.png', 'b.png']);
  expect(await promptText(browser)).toBe('One [a.png]');
  await expect(browser.locator('.composer-attachment.unplaced')).toHaveCount(1);
});

test('a file put in the text after × lands at the caret the person left, which the × shifted', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio'));
  const prompt = screen.getByRole('textbox', 'Task prompt');
  await prompt.click();
  await browser.keyboard.type('A ');
  await pasteImage(browser, 'gone.png');
  await expect(browser.locator('.composer-prompt .pi-atom:not(.pi-pending)')).toHaveCount(1);
  await browser.keyboard.type(' B');
  await pasteImage(browser, 'put.png');
  await expect(browser.locator('.composer-prompt .pi-atom:not(.pi-pending)')).toHaveCount(2);
  await browser.keyboard.press('Backspace');
  await browser.keyboard.type('C');
  await browser.keyboard.press('ArrowLeft');
  expect(await promptText(browser)).toBe('A [gone.png] BC');
  // The caret is between B and C; × on the first file, then the second one goes to the caret.
  await screen.getByRole('button', 'Remove gone.png').click();
  expect(await promptText(browser)).toBe('A  BC');
  await screen.getByRole('button', 'Put put.png in the text').click();
  expect(await promptText(browser)).toBe('A  B[put.png]C');
});
