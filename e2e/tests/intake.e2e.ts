import { test } from '@e2e-dev/web';
import { expect } from 'e2e';
import { axeViolations, failOnPageErrors, mock } from './support.ts';

failOnPageErrors();

// Task intake and chaining (SPEC-001 TSK-011..013): the composer's split card, chained tasks
// and the card of a prompt held by the scope check. The mock answers like the server.

const MIXED = [
  'Finish the release before Friday, all of this has to land together please.',
  '1. Make the attention bar opaque over the terminal and keep its colours readable.',
  '2. Stop the tools rail pills from overflowing the rail on narrow windows.',
  '3. Route new tasks past exhausted accounts in the backend subscription pool.',
  '4. Make the server mark tasks Done reliably once the pull request is tested.',
].join('\n');

test('a request mixing UI and backend work offers a split; a chain waits for its predecessor', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio'));
  // The mock holds its proposal until released, so the waiting card is there to look at however
  // slowly the page is driven.
  await browser.evaluate(() => {
    const mocked = window as unknown as { __yuliusMock: { holdProposal?: Promise<void>; releaseProposal?: () => void } };
    mocked.__yuliusMock.holdProposal = new Promise<void>((resolve) => { mocked.__yuliusMock.releaseProposal = resolve; });
  });
  const prompt = screen.getByRole('textbox', 'Task prompt');
  await prompt.fill(MIXED);
  await prompt.press('ControlOrMeta+Enter');

  // While Claude prepares the proposal the card shimmers below the box; one task already works.
  const card = screen.getByRole('region', 'Split proposal');
  await expect(card).toHaveText(/Splitting your request…/);
  await expect(card.getByRole('status')).toHaveText('Preparing a split proposal…');
  await expect(card.getByRole('button', 'Keep as one task')).toBeVisible();
  await browser.evaluate(() => (window as unknown as { __yuliusMock: { releaseProposal: () => void } }).__yuliusMock.releaseProposal());

  await expect(card).toHaveText(/Split into 2 tasks\?/);
  await expect(card.getByRole('textbox', 'Title of task 1')).toHaveValue('UX: interface fixes');
  // Compact rows: a one-line preview, the prompt collapsed; one primary action.
  await expect(browser.locator('.split-card .split-prompt-text').first()).toBeHidden();
  await browser.locator('.split-card .split-more-closed').first().click();
  await expect(browser.locator('.split-card .split-prompt-text').first()).toHaveText(/attention bar opaque/);
  expect(await browser.evaluate(() => (document.querySelector('.split-run input[value="parallel"]') as HTMLInputElement).checked)).toBe(true);
  expect(await axeViolations(browser)).toEqual([]);

  // The segmented choice's radio is visually hidden: people click its label.
  await screen.getByText('As a chain').click();
  expect(await browser.evaluate(() => (document.querySelector('.split-run input[value="chain"]') as HTMLInputElement).checked)).toBe(true);
  await card.getByRole('button', 'Create 2 tasks').click();
  await expect(browser).toHaveURL(/#\/s\/studio\/t\/\d+$/);
  await expect(screen.getByRole('heading', { level: 1 })).toHaveText(/^#\d+ UX: interface fixes$/);

  // The chain shows on both tasks: "Then #N" here, "Queued after #M" there.
  await screen.getByRole('link', /^Then #\d+$/).click();
  await expect(screen.getByRole('heading', { level: 1 })).toHaveText(/^#\d+ Backend: server work$/);
  await expect(screen.getByRole('link', /^After #\d+$/)).toBeVisible();
  await expect(browser.locator('.attention-bar')).toHaveText(/Queued after #\d+\. It starts by itself when #\d+ is done\./);
  await screen.getByRole('button', 'Start now').click();
  await expect(browser.locator('.strip-status')).not.toHaveText(/Queued after/);
});

test('"Keep as one task" creates the task as written', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio'));
  const prompt = screen.getByRole('textbox', 'Task prompt');
  await prompt.fill(MIXED);
  await prompt.press('ControlOrMeta+Enter');
  const card = screen.getByRole('region', 'Split proposal');
  await card.getByRole('button', 'Keep as one task').click();
  await expect(browser).toHaveURL(/#\/s\/studio\/t\/\d+$/);
  await expect(screen.getByRole('heading', { level: 1 })).toHaveText(/^#\d+ Finish the release before Friday/);
});

test('a short request starts at once and "Run after" chains it', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio'));
  // The choices are the sandbox's tasks: wait until the list has loaded.
  await expect(screen.getByRole('link', /Checkout API/).first()).toBeVisible();
  await screen.getByRole('button', /Run after: Start now/).click();
  await screen.getByRole('menuitemradio', /^After #12 Checkout API/).click();
  const prompt = screen.getByRole('textbox', 'Task prompt');
  await prompt.fill('Write the checkout UI tests');
  await screen.getByRole('button', 'Start task').click();
  await expect(browser).toHaveURL(/#\/s\/studio\/t\/\d+$/);
  await expect(browser.locator('.strip-status')).toHaveText(/Queued after #12/);
});

test('picked conditions become the canonical start rule text (unit, in the page)', async ({ app, browser }) => {
  await app.open(mock('/s/studio'));
  const result = await browser.evaluate(async () => {
    const url = '/assets/ruleeditor.js';
    const { ruleText, ruleChoices, conditionChoices, RULE_MAX_CONDITIONS } = await import(url);
    const tasks = [
      { id: 'a', number: 3, title: 'Three', archived: false },
      { id: 'b', number: 9, title: 'Nine', archived: false },
      { id: 'c', number: 5, title: 'Five', archived: true },
    ];
    const live = tasks.filter((t) => !t.archived); // as ctx.store.tasksOf lists them
    const label = (o: { number: number; label: string }) => `${o.number}:${o.label}`;
    return {
      none: ruleText({ join: 'and', conditions: [] }),
      one: ruleText({ join: 'or', conditions: [{ number: 12, state: 'done' }] }),
      all: ruleText({ join: 'and', conditions: [{ number: 12, state: 'done' }, { number: 13, state: 'merged' }] }),
      any: ruleText({ join: 'or', conditions: [{ number: 12, state: 'done' }, { number: 4, state: 'ended' }, { number: 3, state: 'done' }] }),
      choices: ruleChoices([
        { id: 'a', number: 3, title: 'Three', archived: false },
        { id: 'b', number: 9, title: 'Nine', archived: false },
        { id: 'c', number: 5, title: 'Five', archived: true },
      ]).map((t: { number: number }) => t.number),
      empty: ruleChoices([{ id: 'c', number: 5, title: 'Five', archived: true }]).length,
      // The server takes at most 16 tasks in a rule (MAX_ATOMS in yulius-domain).
      max: RULE_MAX_CONDITIONS,
      // A picked task that is no longer offered stays in its row. The store lists only tasks that
      // are not archived, so a live one beyond the offered choices keeps its title, and one the
      // store no longer has is archived or removed.
      listed: conditionChoices(ruleChoices(tasks), 9, live).map(label),
      older: conditionChoices(ruleChoices(tasks).slice(0, 1), 3, live).map(label),
      unlisted: conditionChoices(ruleChoices(tasks), 5, live).map(label),
    };
  });
  expect(result).toEqual({
    none: '',
    one: '#12 done',
    all: '#12 done and #13 merged',
    any: '#12 done or #4 ended or #3 done',
    choices: [9, 3],
    empty: 0,
    max: 16,
    listed: ['9:#9 Nine', '3:#3 Three'],
    older: ['9:#9 Nine', '3:#3 Three'],
    unlisted: ['9:#9 Nine', '3:#3 Three', '5:#5 (archived or removed)'],
  });
});

test('"Run after" builds a start rule from picked conditions; the task waits for it', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio'));
  await expect(screen.getByRole('link', /Checkout API/).first()).toBeVisible();
  await screen.getByRole('button', /Run after: Start now/).click();
  await screen.getByRole('menuitemradio', /^When a rule is true/).click();

  // An editor of conditions, not a sentence to type (SPEC-022 RED-001).
  const editor = screen.getByRole('dialog', 'Start when a rule is true');
  await expect(editor).toBeVisible();
  await expect(editor.getByRole('textbox')).toHaveCount(0);
  await expect(editor.getByRole('combobox', 'Task of condition 1')).toBeVisible();
  // One condition needs no all/any choice.
  await expect(editor.getByRole('radiogroup', 'Start when')).toHaveCount(0);
  await editor.getByRole('combobox', 'Task of condition 1').selectOption({ label: '#12 Checkout API' });
  await expect(editor.getByRole('combobox', 'State of condition 1')).toHaveValue('done');
  await expect(editor.getByText(/^Starts by itself when/)).toHaveText('Starts by itself when #12 done');

  // A second condition, and whether all or any of them must hold (RED-002).
  await editor.getByRole('button', 'Add condition').click();
  // A new condition names a task not named yet: the newest one, #14.
  await expect(editor.getByRole('combobox', 'Task of condition 2')).toHaveValue('14');
  await editor.getByRole('combobox', 'Task of condition 2').selectOption({ label: '#4 Payment webhook' });
  await editor.getByRole('combobox', 'State of condition 2').selectOption({ label: 'has ended' });
  await expect(editor.getByText(/^Starts by itself when/)).toHaveText('Starts by itself when #12 done and #4 ended');
  // The segment's label takes the click, as for a person.
  await editor.getByText('Any is true').click();
  await expect(editor.getByRole('radio', 'Any is true')).toBeChecked();
  await expect(editor.getByText(/^Starts by itself when/)).toHaveText('Starts by itself when #12 done or #4 ended');
  expect(await axeViolations(browser)).toEqual([]);

  await editor.getByRole('button', 'Use this rule').click();
  await expect(screen.getByRole('button', /Run after: When #12 done or #4 ended/)).toBeVisible();
  await screen.getByRole('textbox', 'Task prompt').fill('Write the checkout UI tests');
  await screen.getByRole('button', 'Start task').click();
  await expect(browser).toHaveURL(/#\/s\/studio\/t\/\d+$/);
  await expect(browser.locator('.attention-bar')).toHaveText(/Queued by a rule\. It starts by itself when #12 done or #4 ended\./);
});

test('a picked rule opens again with its conditions, can be changed, and Start now clears it', async ({ app, screen }) => {
  await app.open(mock('/s/studio'));
  await expect(screen.getByRole('link', /Checkout API/).first()).toBeVisible();
  await screen.getByRole('button', /Run after: Start now/).click();
  await screen.getByRole('menuitemradio', /^When a rule is true/).click();
  let editor = screen.getByRole('dialog', 'Start when a rule is true');
  await editor.getByRole('combobox', 'Task of condition 1').selectOption({ label: '#12 Checkout API' });
  await editor.getByRole('button', 'Add condition').click();
  await editor.getByRole('combobox', 'Task of condition 2').selectOption({ label: '#4 Payment webhook' });
  await editor.getByRole('combobox', 'State of condition 2').selectOption({ label: 'has ended' });
  await editor.getByText('Any is true').click();
  await editor.getByRole('button', 'Use this rule').click();
  await expect(screen.getByRole('button', /Run after: When #12 done or #4 ended/)).toBeVisible();

  // Opening it again shows what was picked (RED-004).
  await screen.getByRole('button', /Run after: When/).click();
  await screen.getByRole('menuitemradio', /^When a rule is true/).click();
  editor = screen.getByRole('dialog', 'Start when a rule is true');
  await expect(editor.getByRole('combobox', 'Task of condition 1')).toHaveValue('12');
  await expect(editor.getByRole('combobox', 'Task of condition 2')).toHaveValue('4');
  await expect(editor.getByRole('combobox', 'State of condition 2')).toHaveValue('ended');
  await expect(editor.getByRole('radio', 'Any is true')).toBeChecked();

  // Removing a condition leaves one: no all/any choice, no remove button.
  await editor.getByRole('button', 'Remove condition 1').click();
  await expect(editor.getByRole('combobox', 'Task of condition 1')).toHaveValue('4');
  await expect(editor.getByRole('combobox', 'Task of condition 2')).toHaveCount(0);
  await expect(editor.getByRole('radiogroup', 'Start when')).toHaveCount(0);
  await expect(editor.getByRole('button', /^Remove condition/)).toHaveCount(0);
  await expect(editor.getByText(/^Starts by itself when/)).toHaveText('Starts by itself when #4 ended');
  await editor.getByRole('button', 'Use this rule').click();
  await expect(screen.getByRole('button', /Run after: When #4 ended/)).toBeVisible();

  // Cancel keeps the rule; Start now clears it.
  await screen.getByRole('button', /Run after: When/).click();
  await screen.getByRole('menuitemradio', /^When a rule is true/).click();
  editor = screen.getByRole('dialog', 'Start when a rule is true');
  await editor.getByRole('button', 'Add condition').click();
  await editor.getByRole('button', 'Cancel').click();
  await expect(screen.getByRole('button', /Run after: When #4 ended$/)).toBeVisible();
  await screen.getByRole('button', /Run after: When/).click();
  await screen.getByRole('menuitemradio', /^Start now/).click();
  await expect(screen.getByRole('button', /Run after: Start now/)).toBeVisible();
});

test('a rule takes at most 16 conditions, as the server does', async ({ app, screen }) => {
  await app.open(mock('/s/studio'));
  await expect(screen.getByRole('link', /Checkout API/).first()).toBeVisible();
  await screen.getByRole('button', /Run after: Start now/).click();
  await screen.getByRole('menuitemradio', /^When a rule is true/).click();
  const editor = screen.getByRole('dialog', 'Start when a rule is true');
  const add = editor.getByRole('button', 'Add condition');
  for (let n = 2; n <= 16; n += 1) {
    await add.click();
    await expect(editor.getByRole('combobox', `Task of condition ${n}`)).toBeVisible();
  }
  await expect(add).toBeDisabled();
  await expect(editor.getByText('A rule names at most 16 conditions.')).toBeVisible();
  await editor.getByRole('button', 'Remove condition 16').click();
  await expect(add).toBeEnabled();
});

test('a sandbox without tasks offers no start rule', async ({ app, screen }) => {
  await app.open(mock('/s/studio', '&tasks=0'));
  await expect(screen.getByRole('textbox', 'Task prompt')).toBeVisible();
  await screen.getByRole('button', /Run after: Start now/).click();
  await expect(screen.getByRole('menuitemradio', /^Start now/)).toBeVisible();
  await expect(screen.getByRole('menuitemradio', /^When a rule is true/)).toHaveCount(0);
});

test('a held prompt shows its choices; "Run after this task" chains a new task', async ({ app, browser, screen }) => {
  await app.open(mock('/s/product/t/4'));
  const card = screen.getByRole('region', 'Held prompt');
  await expect(card).toHaveText(/New work outside this task/);
  await expect(card).toHaveText(/Also add a dark mode to the invoice settings page\./);
  await expect(card.getByRole('button', 'Send anyway')).toBeVisible();
  expect(await axeViolations(browser)).toEqual([]);

  await card.getByRole('button', 'Run after this task').click();
  await expect(card).toBeHidden();
  await expect(screen.getByRole('link', /^Then #8$/)).toBeVisible();
});

test('a chained task can be started now', async ({ app, browser, screen }) => {
  await app.open(mock('/s/product/t/7'));
  await expect(screen.getByRole('heading', { level: 1 })).toHaveText('#7 Invoice emails');
  await expect(browser.locator('.strip-status')).toHaveText(/Queued after #4/);
  await screen.getByRole('button', 'Start now').click();
  await expect(browser.locator('.strip-status')).not.toHaveText(/Queued after/);
});
