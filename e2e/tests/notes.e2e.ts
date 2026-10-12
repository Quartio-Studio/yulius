import { test, type Browser, type Screen } from '@e2e-dev/web';
import { expect } from 'e2e';
import { axeViolations, failOnPageErrors, mock } from './support.ts';

failOnPageErrors();

// Auto-saved notes (SPEC-041): the rail's Notes tool holds a notepad per sandbox and one for the
// team. They save by themselves, a teammate's edit shows up, and edits made at the same time end
// in a choice instead of a lost text.

type MockNotes = {
  __yuliusMock: {
    notes: Record<string, { body: string; revision: number }>;
    editNote: (scope: string, body: string, by: string, options?: { announce?: boolean }) => void;
    failNoteSaves: boolean;
    failNoteLoads: boolean;
    loseNoteResponse: boolean;
    keepaliveRequests: string[];
    noteLoads: number;
  };
};

declare function typeInto(text: string): void;

/** In the page (`typeInto`): the visible notepad's text replaced as typing would, then the test acts at once. */
const pageHelpers = () => {
  (window as unknown as { typeInto: (text: string) => void }).typeInto = (text: string) => {
    const box = [...document.querySelectorAll<HTMLTextAreaElement>('.notes-pane:not([hidden]) textarea')][0];
    box.value = text;
    box.dispatchEvent(new Event('input', { bubbles: true }));
  };
  // Once a test sets the flag, the person has confirmed leaving (the prompt is not drawn in a test).
  window.addEventListener('beforeunload', (event) => {
    if ((window as unknown as { __leaveConfirmed?: boolean }).__leaveConfirmed) event.stopImmediatePropagation();
  });
};

async function openNotes(screen: Screen): Promise<void> {
  await screen.getByRole('button', /^Show tools/).click();
  await screen.getByRole('navigation', 'Tools').getByRole('button', 'Notes').click();
}

/** The text stored by the mock server for `scope` (`team` or `sandbox:<sid>`). */
const stored = (browser: Browser, scope: string) =>
  browser.evaluate((key) => (window as unknown as MockNotes).__yuliusMock.notes[key]?.body ?? '', scope);

const editAs = (browser: Browser, scope: string, body: string, announce = true) =>
  browser.evaluate(([key, text, loud]) => (window as unknown as MockNotes).__yuliusMock.editNote(key as string, text as string, 'alice', { announce: loud as boolean }), [scope, body, announce] as const);

test('a notepad saves by itself and comes back after a reload', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/t/12'));
  await openNotes(screen);

  const tabs = screen.getByRole('tablist', 'Notes');
  await expect(tabs.getByRole('tab', 'This sandbox')).toHaveAttribute('aria-selected', 'true');
  const notes = screen.getByRole('textbox', 'Sandbox notes');
  await expect(notes).not.toHaveAttribute('readonly', '');
  await notes.click();
  await browser.keyboard.type('Staging DB password rotates on Friday');
  await expect(screen.getByRole('status', 'Save state')).toHaveText('Saved');
  expect(await stored(browser, 'sandbox:sb_studio')).toBe('Staging DB password rotates on Friday');
  expect(await axeViolations(browser)).toEqual([]);

  await browser.reload();
  await screen.getByRole('navigation', 'Tools').getByRole('button', 'Notes').click();
  await expect(screen.getByRole('textbox', 'Sandbox notes')).toHaveValue('Staging DB password rotates on Friday');
  await expect(screen.getByRole('status', 'Save state')).toHaveText('Saved');
  await expect(screen.getByText(/Edited by mkarp/)).toBeVisible();
});

test('team notes are the same in every sandbox; sandbox notes are each its own', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio'));
  await openNotes(screen);
  await screen.getByRole('textbox', 'Sandbox notes').fill('Studio only');
  await screen.getByRole('tab', 'Team').click();
  const team = screen.getByRole('textbox', 'Team notes');
  await team.fill('Standup at 10:30');
  await expect(screen.getByRole('status', 'Save state')).toHaveText('Saved');
  expect(await stored(browser, 'sandbox:sb_studio')).toBe('Studio only');

  // Another sandbox: the panel stays on Notes, with the tab used last.
  await app.open(mock('/s/product'));
  await expect(screen.getByRole('textbox', 'Team notes')).toHaveValue('Standup at 10:30');
  await screen.getByRole('tab', 'This sandbox').click();
  await expect(screen.getByRole('textbox', 'Sandbox notes')).toHaveValue('');
  expect(await stored(browser, 'team')).toBe('Standup at 10:30');
});

test("a teammate's edit appears in an idle notepad", async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio'));
  await openNotes(screen);
  const notes = screen.getByRole('textbox', 'Sandbox notes');
  await expect(notes).toHaveValue('');

  await editAs(browser, 'sandbox:sb_studio', 'Release freeze until Monday');
  await expect(notes).toHaveValue('Release freeze until Monday');
  await expect(screen.getByText(/Edited by alice/)).toBeVisible();
  // Another sandbox's edit does not reach this notepad.
  await editAs(browser, 'sandbox:sb_product', 'Product only');
  await editAs(browser, 'team', 'Team text');
  await screen.getByRole('tab', 'Team').click();
  await expect(screen.getByRole('textbox', 'Team notes')).toHaveValue('Team text');
  await screen.getByRole('tab', 'This sandbox').click();
  await expect(notes).toHaveValue('Release freeze until Monday');
});

test('edits made at the same time end in a choice: keep mine or use theirs', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio'));
  await openNotes(screen);
  const notes = screen.getByRole('textbox', 'Sandbox notes');
  await notes.fill('first');
  await expect(screen.getByRole('status', 'Save state')).toHaveText('Saved');

  // Alice saves while this notepad did not hear of it (her edit's event is still on its way).
  await editAs(browser, 'sandbox:sb_studio', 'alice wrote this', false);
  await notes.fill('mine');
  const banner = screen.getByRole('alert');
  await expect(banner).toHaveText(/alice changed these notes/);
  await expect(screen.getByRole('status', 'Save state')).toHaveText('Not saved');
  await expect(notes).toHaveValue('mine');
  expect(await stored(browser, 'sandbox:sb_studio')).toBe('alice wrote this');
  expect(await axeViolations(browser)).toEqual([]);

  await banner.getByRole('button', 'Keep mine').click();
  await expect(screen.getByRole('status', 'Save state')).toHaveText('Saved');
  await expect(banner).toBeHidden();
  expect(await stored(browser, 'sandbox:sb_studio')).toBe('mine');

  // Again, and this time Alice's text wins.
  await editAs(browser, 'sandbox:sb_studio', 'alice again', false);
  await notes.fill('mine again');
  await screen.getByRole('alert').getByRole('button', 'Use theirs').click();
  await expect(notes).toHaveValue('alice again');
  await expect(screen.getByRole('status', 'Save state')).toHaveText('Saved');
  expect(await stored(browser, 'sandbox:sb_studio')).toBe('alice again');
});

test('a failed save says so and saves once the server answers again', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio'));
  await openNotes(screen);
  await browser.evaluate(() => { (window as unknown as MockNotes).__yuliusMock.failNoteSaves = true; });
  await screen.getByRole('textbox', 'Sandbox notes').fill('survives an outage');
  await expect(screen.getByRole('status', 'Save state')).toHaveText('Not saved, retrying…');
  await browser.evaluate(() => { (window as unknown as MockNotes).__yuliusMock.failNoteSaves = false; });
  await expect(screen.getByRole('status', 'Save state')).toHaveText('Saved');
  expect(await stored(browser, 'sandbox:sb_studio')).toBe('survives an outage');
});

test('read access shows sandbox notes read-only and team notes editable', async ({ app, screen }) => {
  await app.open(mock('/s/sandbox-3'));
  await openNotes(screen);
  await expect(screen.getByRole('textbox', 'Sandbox notes')).toHaveAttribute('readonly', '');
  await expect(screen.getByText(/Editing this sandbox’s notes needs write access/)).toBeVisible();
  await screen.getByRole('tab', 'Team').click();
  await expect(screen.getByRole('textbox', 'Team notes')).not.toHaveAttribute('readonly', '');
});

// Review round 1 (SPEC-041): ways typed text could be lost.

const confirmLeaving = (browser: Browser) => browser.evaluate(() => { (window as unknown as { __leaveConfirmed?: boolean }).__leaveConfirmed = true; });

const setMock = (browser: Browser, key: 'failNoteSaves' | 'failNoteLoads' | 'loseNoteResponse', value: boolean) =>
  browser.evaluate(([k, v]) => { ((window as unknown as MockNotes).__yuliusMock as Record<string, unknown>)[k as string] = v; }, [key, value] as const);

test('a notepad is read-only until it is loaded; a failed load says so and retries', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio'));
  await editAs(browser, 'sandbox:sb_studio', 'already here');
  await setMock(browser, 'failNoteLoads', true);
  await openNotes(screen);
  const notes = screen.getByRole('textbox', 'Sandbox notes');
  await expect(screen.getByRole('status', 'Save state')).toHaveText('Couldn’t load the notes, retrying…');
  await expect(notes).toHaveAttribute('readonly', '');
  await setMock(browser, 'failNoteLoads', false);
  await expect(notes).toHaveValue('already here');
  await expect(notes).not.toHaveAttribute('readonly', '');
  await expect(screen.getByRole('status', 'Save state')).toHaveText('Saved');
});

test('text typed just before a reload is kept and saved', async ({ app, browser, screen }) => {
  await browser.addInitScript(pageHelpers);
  await app.open(mock('/s/studio'));
  await openNotes(screen);
  const notes = screen.getByRole('textbox', 'Sandbox notes');
  await expect(screen.getByRole('status', 'Save state')).toHaveText('Saved');
  // Typed, then at once: unsaved text asks the browser to confirm leaving the page, and hiding the
  // page saves at once, with a request that outlives the page.
  expect(await browser.evaluate(() => {
    typeInto('typed in a hurry');
    const event = new Event('beforeunload', { cancelable: true });
    window.dispatchEvent(event);
    window.dispatchEvent(new Event('pagehide'));
    return event.defaultPrevented;
  })).toBe(true);
  await expect.poll(() => browser.evaluate(() => (window as unknown as MockNotes).__yuliusMock.keepaliveRequests)).toEqual(['/api/sandboxes/sb_studio/notes']);
  await expect(screen.getByRole('status', 'Save state')).toHaveText('Saved');

  await notes.fill('typed in a bigger hurry');
  await confirmLeaving(browser);
  await browser.reload();
  await screen.getByRole('navigation', 'Tools').getByRole('button', 'Notes').click();
  await expect(screen.getByRole('textbox', 'Sandbox notes')).toHaveValue('typed in a bigger hurry');
  await expect(screen.getByRole('status', 'Save state')).toHaveText('Saved');
  expect(await stored(browser, 'sandbox:sb_studio')).toBe('typed in a bigger hurry');
});

test('undoing back to the saved text after a failure reads Saved', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio'));
  await openNotes(screen);
  const notes = screen.getByRole('textbox', 'Sandbox notes');
  await notes.fill('kept');
  await expect(screen.getByRole('status', 'Save state')).toHaveText('Saved');
  await setMock(browser, 'failNoteSaves', true);
  await notes.fill('kept, then more');
  await expect(screen.getByRole('status', 'Save state')).toHaveText('Not saved, retrying…');
  await notes.fill('kept');
  await expect(screen.getByRole('status', 'Save state')).toHaveText('Saved');
});

test('a save whose answer was lost is not a conflict with oneself', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio'));
  await openNotes(screen);
  const notes = screen.getByRole('textbox', 'Sandbox notes');
  await expect(screen.getByRole('status', 'Save state')).toHaveText('Saved');
  await setMock(browser, 'loseNoteResponse', true);
  await notes.fill('stored, answer lost');
  await expect(screen.getByRole('status', 'Save state')).toHaveText('Not saved, retrying…');
  await setMock(browser, 'loseNoteResponse', false);
  await notes.fill('stored, answer lost, and more');
  await expect(screen.getByRole('status', 'Save state')).toHaveText('Saved');
  await expect(screen.getByRole('alert')).toBeHidden();
  expect(await stored(browser, 'sandbox:sb_studio')).toBe('stored, answer lost, and more');
});

test('notes fit a phone', async ({ app, browser, screen }) => {
  await browser.setViewport({ width: 390, height: 844 });
  await app.open(mock('/s/studio'));
  await openNotes(screen);
  const notes = screen.getByRole('textbox', 'Sandbox notes');
  await notes.fill('on the go');
  await expect(screen.getByRole('status', 'Save state')).toHaveText('Saved');
  await expect(screen.getByRole('tab', 'Team')).toBeVisible();
  expect(await browser.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(true);
  expect(await axeViolations(browser)).toEqual([]);
});

// Review round 2 (SPEC-041).

test('a conflict kept over a reload is still a choice after another reload', async ({ app, browser, screen }) => {
  await browser.addInitScript(pageHelpers);
  await app.open(mock('/s/studio'));
  await openNotes(screen);
  const notes = screen.getByRole('textbox', 'Sandbox notes');
  await notes.fill('agreed text');
  await expect(screen.getByRole('status', 'Save state')).toHaveText('Saved');
  // Offline: the hide's save fails and the text waits in the tab; meanwhile Alice saves.
  await setMock(browser, 'failNoteSaves', true);
  await browser.evaluate(() => { typeInto('mine, offline'); window.dispatchEvent(new Event('pagehide')); });
  await editAs(browser, 'sandbox:sb_studio', 'theirs', false);
  for (let round = 0; round < 2; round += 1) {
    await confirmLeaving(browser);
    await browser.reload();
    await screen.getByRole('navigation', 'Tools').getByRole('button', 'Notes').click();
    await expect(screen.getByRole('alert')).toHaveText(/alice changed these notes/);
    await expect(screen.getByRole('textbox', 'Sandbox notes')).toHaveValue('mine, offline');
    expect(await stored(browser, 'sandbox:sb_studio')).toBe('theirs');
  }
});

test("a failed reload after a teammate's edit backs off", async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio'));
  await openNotes(screen);
  await expect(screen.getByRole('status', 'Save state')).toHaveText('Saved');
  await setMock(browser, 'failNoteLoads', true);
  const before = await browser.evaluate(() => (window as unknown as MockNotes).__yuliusMock.noteLoads);
  await editAs(browser, 'sandbox:sb_studio', 'from alice');
  await browser.evaluate(() => new Promise((resolve) => setTimeout(resolve, 2500)));
  const tries = await browser.evaluate(() => (window as unknown as MockNotes).__yuliusMock.noteLoads);
  expect(tries - before).toBeLessThanOrEqual(3);
  await setMock(browser, 'failNoteLoads', false);
  await expect(screen.getByRole('textbox', 'Sandbox notes')).toHaveValue('from alice');
});

test('undoing a save whose answer was lost puts the undone text back on the server', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio'));
  await openNotes(screen);
  const notes = screen.getByRole('textbox', 'Sandbox notes');
  await expect(screen.getByRole('status', 'Save state')).toHaveText('Saved');
  await setMock(browser, 'loseNoteResponse', true);
  await notes.fill('typed by mistake');
  await expect(screen.getByRole('status', 'Save state')).toHaveText('Not saved, retrying…');
  await setMock(browser, 'loseNoteResponse', false);
  await notes.fill('');
  await expect(screen.getByRole('status', 'Save state')).toHaveText('Saved');
  await expect.poll(() => stored(browser, 'sandbox:sb_studio')).toBe('');
  await expect(screen.getByRole('alert')).toBeHidden();
});

test('text undone after a hide does not come back after a reload', async ({ app, browser, screen }) => {
  await browser.addInitScript(pageHelpers);
  await app.open(mock('/s/studio'));
  await openNotes(screen);
  await expect(screen.getByRole('status', 'Save state')).toHaveText('Saved');
  await setMock(browser, 'failNoteSaves', true);
  await browser.evaluate(() => { typeInto('deleted later'); window.dispatchEvent(new Event('pagehide')); });
  await screen.getByRole('textbox', 'Sandbox notes').fill('');
  await expect(screen.getByRole('status', 'Save state')).toHaveText('Saved');
  await confirmLeaving(browser);
  await browser.reload();
  await screen.getByRole('navigation', 'Tools').getByRole('button', 'Notes').click();
  await expect(screen.getByRole('status', 'Save state')).toHaveText('Saved');
  await expect(screen.getByRole('textbox', 'Sandbox notes')).toHaveValue('');
});

test('a save stored just before a reload is not a conflict with oneself after it', async ({ app, browser, screen }) => {
  await browser.addInitScript(pageHelpers);
  await app.open(mock('/s/studio'));
  await openNotes(screen);
  const notes = screen.getByRole('textbox', 'Sandbox notes');
  await expect(screen.getByRole('status', 'Save state')).toHaveText('Saved');
  await setMock(browser, 'loseNoteResponse', true);
  await notes.fill('stored');
  await expect(screen.getByRole('status', 'Save state')).toHaveText('Not saved, retrying…');
  await browser.evaluate(() => { typeInto('stored, and more'); window.dispatchEvent(new Event('pagehide')); });
  await confirmLeaving(browser);
  await browser.reload();
  await screen.getByRole('navigation', 'Tools').getByRole('button', 'Notes').click();
  await expect(screen.getByRole('textbox', 'Sandbox notes')).toHaveValue('stored, and more');
  await expect(screen.getByRole('status', 'Save state')).toHaveText('Saved');
  await expect(screen.getByRole('alert')).toBeHidden();
  expect(await stored(browser, 'sandbox:sb_studio')).toBe('stored, and more');
});
