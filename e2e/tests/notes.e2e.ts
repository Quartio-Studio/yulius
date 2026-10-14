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

/** Opens Notes on `tab` (Team opens first by default, SPEC-112), or as it opens with `null`. */
async function openNotes(screen: Screen, tab: string | null = 'This sandbox'): Promise<void> {
  await screen.getByRole('button', /^Show tools/).click();
  await screen.getByRole('navigation', 'Tools').getByRole('button', 'Notes').click();
  if (tab) await screen.getByRole('tab', tab).click();
}

/** The Notes tabs' names, in order. */
const tabOrder = (browser: Browser) =>
  browser.evaluate(() => [...document.querySelectorAll('[role="tablist"][aria-label="Notes"] [role="tab"]')].map((tab) => tab.textContent));

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
  await expect(screen.getByRole('tab', 'Me')).toBeVisible();
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

// Review round 3 (SPEC-041).

test('text kept over a reload survives hides and tab switches while the notes cannot load', async ({ app, browser, screen }) => {
  await browser.addInitScript(pageHelpers);
  await app.open(mock('/s/studio'));
  await openNotes(screen);
  await expect(screen.getByRole('status', 'Save state')).toHaveText('Saved');
  await setMock(browser, 'failNoteSaves', true);
  await browser.evaluate(() => { typeInto('written during a deploy'); window.dispatchEvent(new Event('pagehide')); });
  // Reloaded while the server is still down: the notes cannot load yet.
  await confirmLeaving(browser);
  await app.open(mock('/s/studio', '&noteloads=fail'));
  await screen.getByRole('navigation', 'Tools').getByRole('button', 'Notes').click();
  await expect(screen.getByRole('status', 'Save state')).toHaveText('Couldn’t load the notes, retrying…');
  await browser.evaluate(() => window.dispatchEvent(new Event('pagehide')));
  await screen.getByRole('tab', 'Team').click();
  await screen.getByRole('tab', 'This sandbox').click();
  await setMock(browser, 'failNoteLoads', false);
  await expect(screen.getByRole('textbox', 'Sandbox notes')).toHaveValue('written during a deploy');
  await expect(screen.getByRole('status', 'Save state')).toHaveText('Saved');
  expect(await stored(browser, 'sandbox:sb_studio')).toBe('written during a deploy');
});

test('an undo after a lost answer is not Saved until the server is checked', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio'));
  await openNotes(screen);
  const notes = screen.getByRole('textbox', 'Sandbox notes');
  await expect(screen.getByRole('status', 'Save state')).toHaveText('Saved');
  await setMock(browser, 'loseNoteResponse', true);
  await notes.fill('typed by mistake');
  await expect(screen.getByRole('status', 'Save state')).toHaveText('Not saved, retrying…');
  await setMock(browser, 'loseNoteResponse', false);
  await setMock(browser, 'failNoteLoads', true);
  const loads = await browser.evaluate(() => (window as unknown as MockNotes).__yuliusMock.noteLoads);
  await notes.fill('');
  // Past the save delay: the undo looked at the server, and that look failed.
  await expect.poll(() => browser.evaluate(() => (window as unknown as MockNotes).__yuliusMock.noteLoads)).toBeGreaterThan(loads);
  await expect(screen.getByRole('status', 'Save state')).toHaveText('Saving…');
  expect(await browser.evaluate(() => {
    const event = new Event('beforeunload', { cancelable: true });
    window.dispatchEvent(event);
    return event.defaultPrevented;
  })).toBe(true);
  await setMock(browser, 'failNoteLoads', false);
  await expect(screen.getByRole('status', 'Save state')).toHaveText('Saved');
  await expect.poll(() => stored(browser, 'sandbox:sb_studio')).toBe('');
});

// #310: Notes shown again because the page changed keeps the keyboard where it was; only opening
// it focuses the notepad.
test('Notes beside a task does not take the keyboard when another task opens', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/t/12'));
  await openNotes(screen, null);
  const notes = screen.getByRole('textbox', 'Team notes');
  await expect(notes).toBeFocused();

  await screen.getByRole('link', /3D model viewer/).click();
  await expect(screen.getByRole('heading', { level: 1 })).toHaveText('#11 3D model viewer');
  await expect(notes).toBeVisible();
  // Past the timers the panel's show queued when the page changed (a focus is deferred by one).
  await browser.evaluate(() => new Promise((resolve) => requestAnimationFrame(() => setTimeout(resolve, 0))));
  await expect(notes).not.toBeFocused();
});

// SPEC-103: a personal notepad, "Me", after Team: only its owner's, the same in every sandbox.

test('Me after Team is the person’s own notepad, the same in every sandbox', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio'));
  await openNotes(screen, null);
  expect(await tabOrder(browser)).toEqual(['Team', 'This sandbox', 'Me']);

  await screen.getByRole('tab', 'Me').click();
  await expect(screen.getByRole('tab', 'Me')).toHaveAttribute('aria-selected', 'true');
  const mine = screen.getByRole('textbox', 'My notes');
  await expect(mine).not.toHaveAttribute('readonly', '');
  await expect(mine).toHaveAttribute('placeholder', /Only you see these notes/);
  await mine.fill('grep instead of read?');
  await expect(screen.getByRole('status', 'Save state')).toHaveText('Saved');
  expect(await stored(browser, 'me:usr_1')).toBe('grep instead of read?');
  expect(await stored(browser, 'team')).toBe('');
  expect(await stored(browser, 'sandbox:sb_studio')).toBe('');
  expect(await axeViolations(browser)).toEqual([]);

  // Another sandbox: the same notepad, still on the Me tab.
  await app.open(mock('/s/product'));
  await expect(screen.getByRole('textbox', 'My notes')).toHaveValue('grep instead of read?');

  // A reload keeps it.
  await browser.reload();
  await screen.getByRole('navigation', 'Tools').getByRole('button', 'Notes').click();
  await expect(screen.getByRole('textbox', 'My notes')).toHaveValue('grep instead of read?');
  await expect(screen.getByRole('status', 'Save state')).toHaveText('Saved');
});

test('an edit of Me from the person’s other tab appears in an idle notepad', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio'));
  await openNotes(screen);
  await screen.getByRole('tab', 'Me').click();
  const mine = screen.getByRole('textbox', 'My notes');
  await expect(mine).toHaveValue('');

  await browser.evaluate(() => (window as unknown as MockNotes).__yuliusMock.editNote('me:usr_1', 'From my laptop', 'mkarp'));
  await expect(mine).toHaveValue('From my laptop');
  // A teammate's notes do not reach this notepad.
  await editAs(browser, 'team', 'Team text');
  await expect(mine).toHaveValue('From my laptop');
});

test('a conflict with the person’s own other tab names them, not a teammate', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio'));
  await openNotes(screen);
  await screen.getByRole('tab', 'Me').click();
  const mine = screen.getByRole('textbox', 'My notes');
  await mine.fill('first');
  await expect(screen.getByRole('status', 'Save state')).toHaveText('Saved');

  // Saved from the person's laptop while this tab did not hear of it yet.
  await browser.evaluate(() => (window as unknown as MockNotes).__yuliusMock.editNote('me:usr_1', 'from my laptop', 'mkarp', { announce: false }));
  await mine.fill('from this tab');
  const banner = screen.getByRole('alert');
  await expect(banner).toHaveText(/You changed these notes in another tab or on another device/);
  await expect(banner).not.toHaveText(/mkarp/);
  expect(await axeViolations(browser)).toEqual([]);
  await banner.getByRole('button', 'Use that version').click();
  await expect(mine).toHaveValue('from my laptop');
  await expect(screen.getByRole('status', 'Save state')).toHaveText('Saved');
});

test('a conflict on Me is always with the person’s own other tab, even under an older login', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio'));
  await openNotes(screen);
  await screen.getByRole('tab', 'Me').click();
  const mine = screen.getByRole('textbox', 'My notes');
  await mine.fill('first');
  await expect(screen.getByRole('status', 'Save state')).toHaveText('Saved');

  // Saved before the person renamed their GitHub login: still nobody else's.
  await browser.evaluate(() => (window as unknown as MockNotes).__yuliusMock.editNote('me:usr_1', 'from before the rename', 'old-login', { announce: false }));
  await mine.fill('from this tab');
  const banner = screen.getByRole('alert');
  await expect(banner).toHaveText(/You changed these notes in another tab or on another device/);
  await expect(banner.getByRole('button', 'Use that version')).toBeVisible();
});

// SPEC-112: the tabs in any order, Team first by default.

test('Team comes first and opens first; the tabs move by drag, keyboard and menu and stay so', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio'));
  await openNotes(screen, null);
  expect(await tabOrder(browser)).toEqual(['Team', 'This sandbox', 'Me']);
  await expect(screen.getByRole('tab', 'Team')).toHaveAttribute('aria-selected', 'true');
  await expect(screen.getByRole('textbox', 'Team notes')).toBeVisible();

  // Drag Me before Team: the press does not choose Me.
  const me = await screen.getByRole('tab', 'Me').boundingBox();
  const team = await screen.getByRole('tab', 'Team').boundingBox();
  if (!me || !team) throw new Error('tabs not laid out');
  await browser.mouse.move(me.x + me.width / 2, me.y + me.height / 2);
  await browser.mouse.down();
  for (const x of [me.x - 10, team.x + team.width, team.x + team.width / 2, team.x + 4]) await browser.mouse.move(x, me.y + me.height / 2);
  await browser.mouse.up();
  await expect.poll(() => tabOrder(browser)).toEqual(['Me', 'Team', 'This sandbox']);
  await expect(screen.getByRole('tab', 'Team')).toHaveAttribute('aria-selected', 'true');

  // Alt+← moves the focused tab one place and keeps the keyboard on it; the first goes no further.
  await screen.getByRole('tab', 'Team').focus();
  await expect(screen.getByRole('tab', 'Team')).toHaveAttribute('aria-keyshortcuts', 'Alt+ArrowLeft Alt+ArrowRight');
  await browser.keyboard.press('Alt+ArrowLeft');
  expect(await tabOrder(browser)).toEqual(['Team', 'Me', 'This sandbox']);
  await expect(screen.getByRole('tab', 'Team')).toBeFocused();
  await browser.keyboard.press('Alt+ArrowLeft');
  expect(await tabOrder(browser)).toEqual(['Team', 'Me', 'This sandbox']);
  await browser.keyboard.press('Alt+ArrowRight');
  expect(await tabOrder(browser)).toEqual(['Me', 'Team', 'This sandbox']);

  // The context menu moves it without dragging (WCAG 2.5.7); the first tab cannot go left.
  await browser.evaluate(() => {
    const tab = document.getElementById('notes-tab-me') as HTMLElement;
    const box = tab.getBoundingClientRect();
    tab.dispatchEvent(new MouseEvent('contextmenu', { bubbles: true, cancelable: true, button: 2, clientX: box.left + 8, clientY: box.bottom }));
  });
  await expect(screen.getByRole('menuitem', 'Move left')).toHaveAttribute('aria-disabled', 'true');
  await screen.getByRole('menuitem', 'Move right').click();
  expect(await tabOrder(browser)).toEqual(['Team', 'Me', 'This sandbox']);
  expect(await axeViolations(browser)).toEqual([]);

  // Kept after a reload and in another sandbox.
  await browser.reload();
  await screen.getByRole('navigation', 'Tools').getByRole('button', 'Notes').click();
  expect(await tabOrder(browser)).toEqual(['Team', 'Me', 'This sandbox']);
  await app.open(mock('/s/product'));
  expect(await tabOrder(browser)).toEqual(['Team', 'Me', 'This sandbox']);

  // A stored order that is not the three tabs is ignored.
  await browser.evaluate(() => localStorage.setItem('yulius.notesOrder', JSON.stringify(['team', 'team', 'me'])));
  await browser.reload();
  await screen.getByRole('navigation', 'Tools').getByRole('button', 'Notes').click();
  expect(await tabOrder(browser)).toEqual(['Team', 'This sandbox', 'Me']);
});

test('a remembered tab still opens first', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio'));
  await openNotes(screen, 'Me');
  await browser.reload();
  await screen.getByRole('navigation', 'Tools').getByRole('button', 'Notes').click();
  await expect(screen.getByRole('tab', 'Me')).toHaveAttribute('aria-selected', 'true');
  expect(await tabOrder(browser)).toEqual(['Team', 'This sandbox', 'Me']);
});

/** The drawn shapes of the visible notepad's own selection, relative to its text box. */
const ownShapes = (browser: Browser) => browser.evaluate(() => {
  const pane = document.querySelector('.notes-pane:not([hidden])') as HTMLElement;
  const box = (pane.querySelector('textarea') as HTMLTextAreaElement).getBoundingClientRect();
  return [...pane.querySelectorAll<HTMLElement>('.text-layer .tl-own .tl-shape:not(.tl-gone)')].map((shape) => {
    const r = shape.getBoundingClientRect();
    return { left: Math.round(r.left - box.left), top: Math.round(r.top - box.top), width: Math.round(r.width), height: Math.round(r.height), radius: getComputedStyle(shape).borderTopLeftRadius };
  });
});

/** Selects `start..end` in the visible notepad, as a person would, and waits for the layer to draw it. */
const selectText = (browser: Browser, start: number, end: number) => browser.evaluate(([from, to]) => new Promise<void>((resolve) => {
  const box = document.querySelector('.notes-pane:not([hidden]) textarea') as HTMLTextAreaElement;
  box.focus();
  box.setSelectionRange(from, to);
  requestAnimationFrame(() => requestAnimationFrame(() => resolve()));
}), [start, end] as const);

test('a selection is drawn behind the text in rounded shapes that glide with it', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio'));
  await openNotes(screen, null);
  const team = screen.getByRole('textbox', 'Team notes');
  await team.fill('line one\nline two is longer\nthree');
  await expect(screen.getByRole('status', 'Save state')).toHaveText('Saved');

  // The browser's own highlight is not painted; the layer is decoration only.
  expect(await browser.evaluate(() => getComputedStyle(document.querySelector('.notes-pane:not([hidden]) textarea') as Element, '::selection').backgroundColor)).toBe('rgba(0, 0, 0, 0)');
  expect(await browser.evaluate(() => (document.querySelector('.notes-pane:not([hidden]) .text-layer') as Element).getAttribute('aria-hidden'))).toBe('true');

  // A caret alone draws nothing.
  await selectText(browser, 3, 3);
  expect(await ownShapes(browser)).toEqual([]);

  // "one\nline two": one rounded shape per line, the second starting at the line's start.
  await selectText(browser, 5, 18);
  await expect.poll(async () => (await ownShapes(browser)).length).toBe(2);
  const [first, second] = await ownShapes(browser);
  expect(first.radius).toBe('4px');
  expect(second.top).toBeGreaterThan(first.top);
  expect(first.left).toBeGreaterThan(second.left);
  expect(second.left).toBe(16); // the text box's padding
  // It glides: the shapes move and resize with a transition, not at once.
  expect(await browser.evaluate(() => getComputedStyle(document.querySelector('.text-layer .tl-own .tl-shape') as Element).transitionProperty)).toMatch(/transform/);

  // A smaller selection reuses the first shape and lets the second go.
  await selectText(browser, 0, 4);
  await expect.poll(async () => (await ownShapes(browser)).length).toBe(1);
  await expect.poll(async () => (await ownShapes(browser))[0]).toMatchObject({ left: 16, top: first.top });

  // Scrolling keeps the shape on its text.
  await team.fill(Array.from({ length: 80 }, (_, i) => `line ${i}`).join('\n'));
  const scrollTo = (top: number) => browser.evaluate((y) => { (document.querySelector('.notes-pane:not([hidden]) textarea') as HTMLTextAreaElement).scrollTop = y; }, top);
  await selectText(browser, 0, 6);
  await scrollTo(0);
  await expect.poll(async () => (await ownShapes(browser))[0]?.top).toBe(first.top);
  await scrollTo(40);
  await expect.poll(async () => (await ownShapes(browser))[0].top).toBe(first.top - 40);
  expect(await axeViolations(browser)).toEqual([]);
});
