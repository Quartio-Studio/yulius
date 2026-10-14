import { test } from '@e2e-dev/web';
import { expect } from 'e2e';
import { axeViolations, failOnPageErrors, mock } from './support.ts';

failOnPageErrors();

// Ready-made automations (SPEC-042 AUT-009, AC-AUT-8): the Automations page offers the
// automations Yulius ships, each enabled with one click, after which it is an ordinary automation.

test('a ready-made automation is enabled in one click and joins the list', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/automations'));
  await expect(screen.getByRole('heading', { level: 1 })).toHaveText('Automations');
  await expect(screen.getByRole('heading', /^Ready-made/)).toBeVisible();
  await expect(screen.getByText('Keep the docs up to date')).toBeVisible();
  await expect(screen.getByText('Work through open issues')).toBeVisible();
  expect(await axeViolations(browser)).toEqual([]);

  await screen.getByRole('button', 'Enable Keep the docs up to date').click();
  await expect(screen.getByText('Keep the docs up to date enabled.')).toBeVisible();
  // Now an ordinary automation: its schedule switch is on and it is no longer suggested.
  await expect(screen.getByRole('switch', 'Keep the docs up to date runs on schedule')).toBeChecked();
  await expect(screen.getByRole('button', 'Enable Keep the docs up to date')).toBeHidden();
  await expect(screen.getByRole('button', 'Enable Work through open issues')).toBeVisible();

  await screen.getByRole('button', 'Enable Work through open issues').click();
  await expect(screen.getByRole('switch', 'Work through open issues runs on schedule')).toBeChecked();
  // Nothing left to suggest: the section goes away.
  await expect(screen.getByRole('heading', /^Ready-made/)).toBeHidden();
});

// How an automation's tasks start (SPEC-078, SPEC-093 AUT-020..024): one chip in the action bar,
// beside Delete, Run now and Save, that opens New task's start menu; saved with the automation,
// named in its sentence and list row, and used by Run now as shown.

/** The stored start choice of a mock automation. */
const startOf = (browser, id: string) => browser.evaluate((key) => {
  const a = (window as unknown as { __yuliusMock: { automations: { id: string; supertask?: boolean; after_task_id?: string | null; start_rule?: string | null }[] } }).__yuliusMock.automations.find((x) => x.id === key);
  return a ? { supertask: Boolean(a.supertask), after: a.after_task_id ?? null, rule: a.start_rule ?? null } : null;
}, id);

/** The left edge of an element, to check the action bar's order. */
const leftOf = (browser, selector: string) => browser.evaluate((sel) => document.querySelector(sel)!.getBoundingClientRect().left, selector);

test('the start chip beside Run now sets each start choice, kept by Save and a reload (AC-AUT-20..22)', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/automations/aut_1'));
  const chip = screen.getByRole('button', /^Start: /);
  await expect(chip).toHaveText('Start: Start now');
  await expect(chip).toHaveAttribute('title', 'Each task starts as soon as a session slot is free.');
  // In the action bar between Delete and Run now; "The task" has no Start row any more.
  const [del, at, run] = [await leftOf(browser, '.wf-head .btn-quiet-danger'), await leftOf(browser, '.au-start'), await leftOf(browser, '.wf-head .au-run')];
  expect(del < at && at < run).toBe(true);
  await expect(screen.getByRole('radiogroup', 'Start')).toHaveCount(0);
  const sentence = browser.locator('.au-when');
  await expect(sentence).not.toHaveText(/supertask|after #|when #/);

  // New task's menu: Start now, Supertask, the tasks to run after and a rule.
  await chip.click();
  await expect(screen.getByRole('menuitemradio', /^Start now/)).toHaveAttribute('aria-checked', 'true');
  await expect(screen.getByRole('menuitemradio', /^#12 Checkout API/)).toBeVisible();
  await expect(screen.getByRole('menuitemradio', /^When a rule is true/)).toBeVisible();
  expect(await axeViolations(browser)).toEqual([]);

  // Supertask.
  await screen.getByRole('menuitemradio', /^Supertask/).click();
  await expect(chip).toHaveText('Start: Supertask');
  await expect(chip).toHaveAttribute('title', /runs alone/);
  await expect(sentence).toHaveText(/ · supertask/);
  await screen.getByRole('button', 'Save').click();
  await expect(screen.getByText('Automation Daily customer feedback saved.')).toBeVisible();
  expect(await startOf(browser, 'aut_1')).toEqual({ supertask: true, after: null, rule: null });

  // After a task: the project's tasks that have not ended.
  await chip.click();
  await screen.getByRole('menuitemradio', /^#12 Checkout API/).click();
  await expect(chip).toHaveText('Start: After #12');
  await expect(sentence).toHaveText(/ · after #12/);
  await screen.getByRole('button', 'Save').click();
  await expect(screen.getByRole('button', 'Save')).toBeDisabled();
  expect(await startOf(browser, 'aut_1')).toEqual({ supertask: false, after: 'tsk_studio_12', rule: null });

  // A rule, picked in New task's rule editor.
  await chip.click();
  await screen.getByRole('menuitemradio', /^When a rule is true/).click();
  const editor = screen.getByRole('dialog', 'Start when a rule is true');
  await editor.getByRole('combobox', 'Task of condition 1').selectOption({ label: '#12 Checkout API' });
  await editor.getByRole('combobox', 'State of condition 1').selectOption({ label: 'is merged' });
  await editor.getByRole('button', 'Use this rule').click();
  await expect(chip).toHaveText('Start: When #12 merged');
  await expect(sentence).toHaveText(/ · when #12 merged/);
  await screen.getByRole('button', 'Save').click();
  await expect(screen.getByRole('button', 'Save')).toBeDisabled();
  expect(await startOf(browser, 'aut_1')).toEqual({ supertask: false, after: null, rule: '#12 merged' });

  // The list names it, and its switch keeps it.
  await browser.evaluate(() => { location.hash = '#/s/studio/automations'; });
  await expect(screen.getByText(/when #12 merged/)).toBeVisible();
  await screen.getByRole('switch', 'Daily customer feedback runs on schedule').click();
  await expect(screen.getByText('Daily customer feedback paused.')).toBeVisible();
  expect(await startOf(browser, 'aut_1')).toEqual({ supertask: false, after: null, rule: '#12 merged' });

  // Opened again (read from the server: the mock's data does not outlive a page reload), the chip
  // reads the saved rule, which opens in the editor as its condition.
  await browser.evaluate(() => { location.hash = '#/s/studio/automations/aut_1'; });
  await expect(chip).toHaveText('Start: When #12 merged');
  await chip.click();
  await expect(screen.getByRole('menuitemradio', /^When a rule is true/)).toHaveAttribute('aria-checked', 'true');
  await screen.getByRole('menuitemradio', /^When a rule is true/).click();
  await expect(screen.getByRole('dialog', 'Start when a rule is true').getByRole('combobox', 'State of condition 1')).toHaveValue('merged');
  await screen.getByRole('dialog', 'Start when a rule is true').getByRole('button', 'Cancel').click();

  // Start now clears it.
  await chip.click();
  await screen.getByRole('menuitemradio', /^Start now/).click();
  await expect(chip).toHaveText('Start: Start now');
  await screen.getByRole('button', 'Save').click();
  await expect(screen.getByRole('button', 'Save')).toBeDisabled();
  expect(await startOf(browser, 'aut_1')).toEqual({ supertask: false, after: null, rule: null });
});

test('Run now starts its task the way the chip shows, saved or not (AC-AUT-23)', async ({ app, browser, screen }) => {
  // aut_2 has no earlier task still active, so its run creates one.
  await app.open(mock('/s/studio/automations/aut_2'));
  const chip = screen.getByRole('button', /^Start: /);
  await expect(chip).toHaveText('Start: Start now');
  await chip.click();
  await screen.getByRole('menuitemradio', /^Supertask/).click();
  await expect(chip).toHaveText('Start: Supertask');
  // Not saved: Run now still makes a supertask, and the automation keeps its stored choice.
  await screen.getByRole('button', 'Run now').click();
  await expect(screen.getByText(/^task created \(#\d+\)/)).toBeVisible();
  const made = await browser.evaluate(() => {
    const mockDb = (window as unknown as { __yuliusMock: { automations: { id: string; last_run: { task_id: string } | null }[]; tasks: { id: string; supertask?: boolean; status: string }[] } }).__yuliusMock;
    const run = mockDb.automations.find((a) => a.id === 'aut_2')!.last_run!;
    const task = mockDb.tasks.find((t) => t.id === run.task_id)!;
    return { supertask: Boolean(task.supertask), status: task.status };
  });
  expect(made).toEqual({ supertask: true, status: 'queued' });
  expect(await startOf(browser, 'aut_2')).toEqual({ supertask: false, after: null, rule: null });
  await expect(screen.getByRole('button', 'Save')).toBeEnabled();
});

test('Run now and the start chip wait until the automation has loaded (SPEC-093 review)', async ({ app, browser, screen }) => {
  // The automation is read late: until then the chip names no choice and Run now is off, so it
  // never runs with a choice the page has not shown.
  await app.open(mock('/s/studio/automations/aut_2', '&hold=/api/sandboxes/sb_studio/automations'));
  await expect(screen.getByRole('button', 'Start')).toBeDisabled();
  await expect(screen.getByRole('button', 'Run now')).toBeDisabled();
  await browser.evaluate(() => (window as unknown as { __yuliusMock: { release: () => void } }).__yuliusMock.release());
  await expect(screen.getByRole('button', 'Run now')).toBeEnabled();
  await expect(screen.getByRole('button', 'Start: Start now')).toBeEnabled();
});

test('a reader sees the start chip, disabled, in the action bar (AC-AUT-24)', async ({ app, screen }) => {
  await app.open(mock('/s/sandbox-3/automations/new'));
  await expect(screen.getByText(/^Read-only: changing automations/)).toBeVisible();
  await expect(screen.getByRole('button', 'Start: Start now')).toBeDisabled();
  await expect(screen.getByRole('button', 'Create automation')).toBeDisabled();
});

test('a stored rule text opens in the rule editor as conditions, or not at all (unit, in the page)', async ({ app, browser }) => {
  await app.open(mock('/s/studio/automations'));
  const read = await browser.evaluate(async () => {
    const url = '/assets/ruleeditor.js';
    const { ruleFromText, ruleText } = await import(url);
    const texts = ['#12 done', '#12 done and #13 merged', '#12 done or #4 ended', '#1 done and #2 done or #3 done', '(#1 done or #2 done) and #3 done', '', 'not #1 done'];
    return texts.map((text) => {
      const rule = ruleFromText(text);
      return rule ? ruleText(rule) : null;
    });
  });
  expect(read).toEqual(['#12 done', '#12 done and #13 merged', '#12 done or #4 ended', null, null, null, null]);
});
