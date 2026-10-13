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

// How an automation's tasks start (SPEC-078 AUT-018, AC-AUT-18): the choices of New task's start
// chip, picked on the builder, saved with the automation and named in its sentence and list row.

/** The stored start choice of a mock automation. */
const startOf = (browser, id: string) => browser.evaluate((key) => {
  const a = (window as unknown as { __yuliusMock: { automations: { id: string; supertask?: boolean; after_task_id?: string | null; start_rule?: string | null }[] } }).__yuliusMock.automations.find((x) => x.id === key);
  return a ? { supertask: Boolean(a.supertask), after: a.after_task_id ?? null, rule: a.start_rule ?? null } : null;
}, id);

test('an automation starts its tasks now, as a supertask, after a task or on a rule (AC-AUT-18)', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/automations/aut_1'));
  const start = screen.getByRole('radiogroup', 'Start');
  await expect(start.getByRole('radio', 'Start now')).toHaveAttribute('aria-checked', 'true');
  const sentence = browser.locator('.au-when');
  await expect(sentence).not.toHaveText(/supertask|after #|when #/);

  // Supertask.
  await start.getByRole('radio', 'Supertask').click();
  await expect(start.getByRole('radio', 'Supertask')).toHaveAttribute('aria-checked', 'true');
  await expect(sentence).toHaveText(/ · supertask/);
  await expect(screen.getByText(/runs alone/)).toBeVisible();
  expect(await axeViolations(browser)).toEqual([]);
  await screen.getByRole('button', 'Save').click();
  await expect(screen.getByText('Automation Daily customer feedback saved.')).toBeVisible();
  expect(await startOf(browser, 'aut_1')).toEqual({ supertask: true, after: null, rule: null });

  // After a task: the project's tasks that have not ended.
  await start.getByRole('radio', /^After a task/).click();
  await screen.getByRole('menuitemradio', /^#12 Checkout API/).click();
  await expect(start.getByRole('radio', 'After #12')).toHaveAttribute('aria-checked', 'true');
  await expect(sentence).toHaveText(/ · after #12/);
  await screen.getByRole('button', 'Save').click();
  await expect(screen.getByRole('button', 'Save')).toBeDisabled();
  expect(await startOf(browser, 'aut_1')).toEqual({ supertask: false, after: 'tsk_studio_12', rule: null });

  // A rule, picked in New task's rule editor.
  await start.getByRole('radio', /^When a rule is true/).click();
  const editor = screen.getByRole('dialog', 'Start when a rule is true');
  await editor.getByRole('combobox', 'Task of condition 1').selectOption({ label: '#12 Checkout API' });
  await editor.getByRole('combobox', 'State of condition 1').selectOption({ label: 'is merged' });
  await editor.getByRole('button', 'Use this rule').click();
  await expect(start.getByRole('radio', 'When #12 merged')).toHaveAttribute('aria-checked', 'true');
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

  // Opened again, the rule shows in the editor with its condition; Start now clears it.
  await browser.evaluate(() => { location.hash = '#/s/studio/automations/aut_1'; });
  await expect(start.getByRole('radio', 'When #12 merged')).toHaveAttribute('aria-checked', 'true');
  await start.getByRole('radio', 'When #12 merged').click();
  await expect(screen.getByRole('dialog', 'Start when a rule is true').getByRole('combobox', 'State of condition 1')).toHaveValue('merged');
  await screen.getByRole('dialog', 'Start when a rule is true').getByRole('button', 'Cancel').click();
  await start.getByRole('radio', 'Start now').click();
  await screen.getByRole('button', 'Save').click();
  await expect(screen.getByRole('button', 'Save')).toBeDisabled();
  expect(await startOf(browser, 'aut_1')).toEqual({ supertask: false, after: null, rule: null });
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
