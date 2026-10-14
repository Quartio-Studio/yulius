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
  await expect(browser.locator('.composer-prompt .pi-image img')).toHaveCount(1);
  // No row of thumbnails above the text: the image is inside it, at the caret.
  await expect(browser.locator('.composer-attachments')).toHaveCount(0);
  await browser.keyboard.type('looks wrong, while the header is fine.');
  expect(await promptText(browser)).toBe('This part [shot.png]looks wrong, while the header is fine.');
  expect(await axeViolations(browser)).toEqual([]);

  // One Backspace takes the whole image; undo brings it back where it was.
  for (let i = 0; i < 'looks wrong, while the header is fine.'.length; i += 1) await browser.keyboard.press('ArrowLeft');
  await browser.keyboard.press('Backspace');
  await expect(browser.locator('.composer-prompt .pi-atom')).toHaveCount(0);
  expect(await promptText(browser)).toBe('This part looks wrong, while the header is fine.');
  await browser.keyboard.press('ControlOrMeta+z');
  await expect(browser.locator('.composer-prompt .pi-image img')).toHaveCount(1);
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
  await expect(browser.locator('.composer-prompt .pi-image img')).toHaveCount(1);
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
  await expect(browser.locator('.composer-prompt .pi-image img')).toHaveCount(1);
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

test('a draft keeps each image in its place across a reload; an older draft shows its files after the text', async ({ app, browser, screen }) => {
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
  await expect(browser.locator('.composer-prompt .pi-atom')).toHaveCount(1);
  expect(await promptText(browser)).toBe('Older words\n[old.png]');
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
  await expect(browser.locator('.composer-prompt .pi-image img')).toHaveCount(1);
  await browser.keyboard.press('ControlOrMeta+z');
  await expect(browser.locator('.composer-prompt .pi-atom')).toHaveCount(0);
  await browser.keyboard.press('ControlOrMeta+Shift+z');
  await expect(browser.locator('.composer-prompt .pi-image img')).toHaveCount(1);
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
  await expect(browser.locator('.composer-prompt .pi-image img')).toHaveCount(2);
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
  await expect(browser.locator('.composer-prompt .pi-image img')).toHaveCount(1);
  await expect(browser.locator('.composer-prompt .pi-pending')).toHaveCount(0);
});
