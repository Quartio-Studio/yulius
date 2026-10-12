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
  };
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
  await notes.click();
  await browser.keyboard.type('Staging DB password rotates on Friday');
  await expect(screen.getByRole('status', 'Save state')).toHaveText('Saved');
  expect(await stored(browser, 'sandbox:sb_studio')).toBe('Staging DB password rotates on Friday');
  expect(await axeViolations(browser)).toEqual([]);

  await browser.reload();
  await screen.getByRole('navigation', 'Tools').getByRole('button', 'Notes').click();
  await expect(screen.getByRole('textbox', 'Sandbox notes')).toHaveValue('Staging DB password rotates on Friday');
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
