import { test } from '@e2e-dev/web';
import { expect } from 'e2e';
import { axeViolations, failOnPageErrors, mock } from './support.ts';

failOnPageErrors();

// Default automations (SPEC-042 AUT-009, SPEC-197 AUT-032..036): the automations Yulius ships come
// first, as the default workflows do, each switched on or off in place; the project's others are
// Custom. A shipped automation an agent created by name is a default, not "Created by an agent".

/** The names of the automation cards in one section, in order. */
const namesIn = (browser, section: string) => browser.evaluate((id) => [...document.querySelectorAll(`section[aria-labelledby="${id}"] .item-card .item-name`)].map((n) => n.textContent), section);

/** The badges and button labels of one automation card. */
const cardOf = (browser, name: string) => browser.evaluate((title) => {
  const card = [...document.querySelectorAll('.item-card')].find((c) => c.querySelector('.item-name')?.textContent === title);
  return card ? { badges: [...card.querySelectorAll('.item-title .badge')].map((b) => b.textContent.trim()), buttons: [...card.querySelectorAll('.item-controls button')].map((b) => b.getAttribute('aria-label') || b.textContent.trim()), opens: Boolean(card.querySelector('.item-open')) } : null;
}, name);

const CUSTOM = ['Daily customer feedback', 'Weekly dependency audit'];
const DEFAULTS = ['Keep the docs up to date', 'Work through open issues', 'Deploy dev every night'];

test('the shipped automations are the defaults, switched on in place, and Delete resets one (AC-AUT-31..33)', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/automations'));
  await expect(screen.getByRole('heading', { level: 1 })).toHaveText('Automations');
  await expect(screen.getByRole('heading', /^Defaults/)).toBeVisible();
  await expect(screen.getByRole('heading', /^Custom/)).toBeVisible();
  await expect(screen.getByRole('heading', /^Ready-made/)).toBeHidden();
  expect(await namesIn(browser, 'automations-defaults')).toEqual(DEFAULTS);
  expect(await namesIn(browser, 'automations-custom')).toEqual(CUSTOM);
  // An agent created this one by name: it is a default all the same.
  const issues = await cardOf(browser, 'Work through open issues');
  expect(issues!.badges).toEqual(['Default']);
  expect(issues!.buttons).toContain('Run now');
  expect(issues!.opens).toBe(true);
  // Custom automations keep their own badges.
  expect((await cardOf(browser, 'Daily customer feedback'))!.badges).toEqual(['Created by an agent']);
  // Not added yet: off, with nothing else to press.
  const docs = screen.getByRole('switch', 'Keep the docs up to date runs on schedule');
  await expect(docs).not.toBeChecked();
  expect((await cardOf(browser, 'Keep the docs up to date'))!).toEqual({ badges: ['Default'], buttons: [], opens: false });
  expect(await axeViolations(browser)).toEqual([]);

  // The switch turns it on (from the keyboard): it is created, enabled, and is an ordinary
  // automation from then on, still a default, with the keyboard still on its switch.
  await docs.focus();
  await browser.keyboard.press('Space');
  // A second press while it is being enabled keeps it on: one automation per switch (AUT-034).
  await browser.keyboard.press('Space');
  expect(await browser.evaluate(() => (document.activeElement as HTMLInputElement).checked)).toBe(true);
  await expect(screen.getByText('Keep the docs up to date enabled.')).toBeVisible();
  await expect(screen.getByRole('button', 'Run history of Keep the docs up to date')).toBeVisible();
  await expect(screen.getByRole('switch', 'Keep the docs up to date runs on schedule')).toBeChecked();
  await expect(screen.getByRole('switch', 'Keep the docs up to date runs on schedule')).toBeFocused();
  const docsOn = await cardOf(browser, 'Keep the docs up to date');
  expect(docsOn!.badges).toEqual(['Default']);
  expect(docsOn!.buttons).toContain('Run now');
  expect(docsOn!.opens).toBe(true);
  expect(await namesIn(browser, 'automations-defaults')).toEqual(DEFAULTS);
  expect(await namesIn(browser, 'automations-custom')).toEqual(CUSTOM);
  await screen.getByRole('switch', 'Keep the docs up to date runs on schedule').click();
  await expect(screen.getByText('Keep the docs up to date paused.')).toBeVisible();

  // Delete resets a default: it stays listed, off, to be switched on again from the shipped version.
  await screen.getByRole('button', 'Delete Work through open issues').click();
  const dialog = screen.getByRole('dialog', 'Delete automation “Work through open issues”?');
  await expect(dialog.getByText(/stays among the defaults, off/)).toBeVisible();
  await expect(dialog.getByText(/changes made here are lost/)).toBeVisible();
  await dialog.getByRole('button', 'Delete').click();
  await expect(screen.getByRole('button', 'Delete Work through open issues')).toBeHidden();
  await expect(screen.getByRole('switch', 'Work through open issues runs on schedule')).not.toBeChecked();
  expect((await cardOf(browser, 'Work through open issues'))!).toEqual({ badges: ['Default'], buttons: [], opens: false });
  expect(await namesIn(browser, 'automations-defaults')).toEqual(DEFAULTS);
  expect(await namesIn(browser, 'automations-custom')).toEqual(CUSTOM);
});

test('the builder’s Delete of a default says it is reset, of a custom one not (AUT-035)', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/automations/aut_3'));
  await expect(screen.getByRole('textbox', 'Automation name')).toHaveValue('Work through open issues');
  await browser.locator('.wf-head .btn-quiet-danger').click();
  const dialog = screen.getByRole('dialog', 'Delete automation “Work through open issues”?');
  await expect(dialog.getByText(/stays among the defaults, off/)).toBeVisible();
  await dialog.getByRole('button', 'Cancel').click();

  await browser.evaluate(() => { location.hash = '#/s/studio/automations/aut_2'; });
  await expect(screen.getByRole('textbox', 'Automation name')).toHaveValue('Weekly dependency audit');
  await browser.locator('.wf-head .btn-quiet-danger').click();
  const custom = screen.getByRole('dialog', 'Delete automation “Weekly dependency audit”?');
  await expect(custom.getByText('Future runs are cancelled. Tasks it already created stay.')).toBeVisible();
  await expect(custom.getByText(/stays among the defaults/)).toBeHidden();
});

test('a reload keeps focus on the same labelled control, wherever it moved (unit, in the page)', async ({ app, browser }) => {
  await app.open(mock('/s/studio/automations'));
  const focused = await browser.evaluate(async () => {
    const url = '/assets/dom.js';
    const { h, keepFocus } = await import(url);
    const container = h('div');
    document.body.append(container);
    const card = (labels: string[]) => labels.map((label) => h('button', { type: 'button', 'aria-label': label }, label));
    container.append(...card(['a', 'switch']));
    (container.lastChild as HTMLElement).focus();
    // The rebuilt card gains a control before the switch (an added default's open button).
    keepFocus(container, () => { container.replaceChildren(...card(['open', 'a', 'switch'])); });
    const byLabel = document.activeElement?.getAttribute('aria-label') ?? null;
    // Labels that are not unique fall back to the position.
    container.replaceChildren(...card(['x', 'x']));
    (container.firstChild as HTMLElement).focus();
    keepFocus(container, () => { container.replaceChildren(...card(['y', 'x', 'x'])); });
    const byPlace = document.activeElement?.getAttribute('aria-label') ?? null;
    container.remove();
    return [byLabel, byPlace];
  });
  expect(focused).toEqual(['switch', 'y']);
});

test('without the shipped list every automation shows under Custom (AC-AUT-35)', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/automations', '&fail=/api/sandboxes/sb_studio/automation-templates'));
  await expect(screen.getByRole('heading', /^Custom/)).toBeVisible();
  await expect(screen.getByRole('heading', /^Defaults/)).toBeHidden();
  expect(await namesIn(browser, 'automations-custom')).toEqual(['Daily customer feedback', 'Weekly dependency audit', 'Work through open issues']);
  expect((await cardOf(browser, 'Work through open issues'))!.badges).toEqual(['Created by an agent']);
});

test('a failed switch-on stays off and says why; read access cannot switch (AC-AUT-32, AC-AUT-34)', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio/automations', '&failpost=/api/sandboxes/sb_studio/automation-templates/nightly-dev-deploy/enable'));
  await screen.getByRole('switch', 'Deploy dev every night runs on schedule').click();
  await expect(screen.getByText(/Couldn’t enable automation/)).toBeVisible();
  await expect(screen.getByRole('switch', 'Deploy dev every night runs on schedule')).not.toBeChecked();
  await expect(screen.getByRole('switch', 'Deploy dev every night runs on schedule')).toBeEnabled();

  await browser.evaluate(() => { location.hash = '#/s/sandbox-3/automations'; });
  await expect(screen.getByRole('heading', /^Defaults/)).toBeVisible();
  for (const name of ['Keep the docs up to date', 'Work through open issues', 'Deploy dev every night']) {
    await expect(screen.getByRole('switch', `${name} runs on schedule`)).toBeDisabled();
  }
  await expect(screen.getByText('No custom automations yet')).toBeVisible();
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
  // New task's wording (SUPERTASK_HELP), said of each task (#407).
  await expect(chip).toHaveAttribute('title', 'Each task waits for this project’s running tasks, then runs alone while the project’s other tasks wait.');
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
  // Its label loads as a shimmering bar, as every loading field does (ADR-0073, #407): no "…".
  expect(await browser.evaluate(() => {
    const chip = document.querySelector('.au-start')!;
    const bar = chip.querySelector('.skeleton-bar');
    return { text: chip.textContent, bar: Boolean(bar), hidden: bar?.closest('[aria-hidden="true"]') !== null, drawn: (bar?.getBoundingClientRect().width ?? 0) >= 40 };
  })).toEqual({ text: 'Start', bar: true, hidden: true, drawn: true });
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

// "When I decide" (SPEC-155, AC-AUT-30): each task of the automation waits, prepared, until a
// person presses Start now. The menu offers it without "At a time…": the schedule is the time.

/** The stored `prepared` flag of a mock automation, and the status of its last run's task. */
const preparedOf = (browser, id: string) => browser.evaluate((key) => {
  const mockDb = (window as unknown as { __yuliusMock: { automations: { id: string; prepared?: boolean; last_run: { task_id: string | null } | null }[]; tasks: { id: string; status: string }[] } }).__yuliusMock;
  const a = mockDb.automations.find((x) => x.id === key)!;
  const task = a.last_run && a.last_run.task_id ? mockDb.tasks.find((t) => t.id === a.last_run!.task_id) : null;
  return { prepared: Boolean(a.prepared), lastTask: task ? task.status : null };
}, id);

test('When I decide prepares each task: chip, sentence, Run now, Save and the list (AC-AUT-30)', async ({ app, browser, screen }) => {
  // aut_2 has no earlier task still active, so its run creates one.
  await app.open(mock('/s/studio/automations/aut_2'));
  const chip = screen.getByRole('button', /^Start: /);
  await expect(chip).toHaveText('Start: Start now');
  await chip.click();
  await expect(screen.getByRole('menuitemradio', /^When I decide/)).toHaveAttribute('aria-checked', 'false');
  await expect(screen.getByRole('menuitemradio', /^At a time/)).toHaveCount(0);
  expect(await axeViolations(browser)).toEqual([]);
  await screen.getByRole('menuitemradio', /^When I decide/).click();
  await expect(chip).toHaveText('Start: When I decide');
  await expect(chip).toHaveAttribute('title', 'Each task waits, prepared, until you press Start now.');
  const sentence = browser.locator('.au-when');
  await expect(sentence).toHaveText(/ · when I decide/);

  // Not saved: Run now still prepares its task, and the automation keeps its stored choice.
  await screen.getByRole('button', 'Run now').click();
  await expect(screen.getByText(/^task created \(#\d+\)/)).toBeVisible();
  expect(await preparedOf(browser, 'aut_2')).toEqual({ prepared: false, lastTask: 'prepared' });

  await screen.getByRole('button', 'Save').click();
  await expect(screen.getByRole('button', 'Save')).toBeDisabled();
  expect(await preparedOf(browser, 'aut_2')).toEqual({ prepared: true, lastTask: 'prepared' });
  // The prepared task still waits: the next run is skipped.
  await screen.getByRole('button', 'Run now').click();
  await expect(browser.locator('.toast-msg:has-text("skipped: previous task still active")')).toBeVisible();
  expect(await preparedOf(browser, 'aut_2')).toEqual({ prepared: true, lastTask: null });

  // Picked again, the menu marks it; another choice clears it.
  await chip.click();
  await expect(screen.getByRole('menuitemradio', /^When I decide/)).toHaveAttribute('aria-checked', 'true');
  await screen.getByRole('menuitemradio', /^Supertask/).click();
  await expect(chip).toHaveText('Start: Supertask');
  await chip.click();
  await screen.getByRole('menuitemradio', /^When I decide/).click();
  await expect(screen.getByRole('button', 'Save')).toBeDisabled();

  // The list names it, and its switch keeps it.
  await browser.evaluate(() => { location.hash = '#/s/studio/automations'; });
  await expect(screen.getByText(/when I decide/)).toBeVisible();
  await screen.getByRole('switch', 'Weekly dependency audit runs on schedule').click();
  await expect(screen.getByText('Weekly dependency audit enabled.')).toBeVisible();
  expect((await preparedOf(browser, 'aut_2')).prepared).toBe(true);

  // Opened again, the chip reads it.
  await browser.evaluate(() => { location.hash = '#/s/studio/automations/aut_2'; });
  await expect(chip).toHaveText('Start: When I decide');
});
