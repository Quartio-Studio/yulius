import { test } from '@e2e-dev/web';
import { expect } from 'e2e';
import { failOnPageErrors, mock } from './support.ts';

failOnPageErrors();

type Mocked = {
  __yuliusMock: {
    accounts: { id: string; provider: string; label: string; status: string }[];
    lastTaskCreate: { harness?: string; model?: string; effort?: string };
    tasks: { harness: string; harness_choice: string; model: string; effort: string; model_default: boolean }[];
  };
};

// SPEC-620 CDM-003: a browser with no saved model starts on Opus 5.5 (Claude Code).
test('an untouched new task runs Claude Code on Opus 5.5 at the inherited effort', async ({ app, browser, screen }) => {
  await app.open(mock('/settings'));
  await expect(screen.getByRole('button', /^Model for new tasks:/)).toContainText('Opus 5.5');
  await app.open(mock('/s/studio'));
  await expect(screen.getByRole('button', /^Model:/)).toContainText('Opus 5.5');
  await screen.getByRole('textbox', 'Task prompt').fill('Add a health check');
  await screen.getByRole('button', 'Start task').click();
  await expect(browser).toHaveURL(/#\/s\/studio\/t\/\d+$/);
  const { lastTaskCreate, task } = await browser.evaluate(() => {
    const { lastTaskCreate, tasks } = (window as unknown as Mocked).__yuliusMock;
    return { lastTaskCreate, task: tasks.at(-1) };
  });
  expect(lastTaskCreate.model).toBe('claude-opus-5-5');
  expect(Object.hasOwn(lastTaskCreate, 'harness')).toBe(false);
  expect(Object.hasOwn(lastTaskCreate, 'effort')).toBe(false);
  // The browser names Opus 5.5 itself, so the task records it as chosen, not inherited.
  expect(task).toMatchObject({ harness: 'claude', harness_choice: 'chosen', model: 'claude-opus-5-5', model_default: false, effort: 'high' });
});

test('unrecognized and unreadable saved models recover to Opus 5.5, a saved Codex default stays', async ({ app, browser, screen }) => {
  await app.open(mock('/settings'));
  for (const [raw, shown] of [['"removed-model"', 'Opus 5.5'], ['{', 'Opus 5.5'], ['"codex"', 'Codex default']]) {
    await browser.evaluate((value) => localStorage.setItem('yulius.composer.model', value), raw);
    await app.open(mock('/s/studio'));
    await expect(screen.getByRole('button', /^Model:/)).toContainText(shown);
    await app.open(mock('/settings'));
    await expect(screen.getByRole('button', /^Model for new tasks:/)).toContainText(shown);
  }
});

test('the untouched composer offers the Claude connection when no subscription is usable', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio', '&claude=0'));
  await expect(screen.getByRole('link', 'Connect Claude')).toHaveAttribute('href', /\/admin\/accounts\?add=1$/);
  await screen.getByRole('textbox', 'Task prompt').fill('Add a health check');
  await screen.getByRole('button', 'Start task').click();
  const toast = browser.locator('.toast-error');
  await expect(toast).toContainText('Connect a Claude account first');
  await toast.getByRole('button', { name: 'Connect Claude' }).click();
  await expect(screen.getByRole('dialog', 'Add a Claude subscription')).toBeVisible();
});

for (const choice of [
  { label: 'GPT-6.2 Astra', query: '6.2 Astra', id: 'gpt-6.2-astra', provider: 'openai', harness: 'codex', tier: 'Max', effort: 'max' },
  { label: 'GPT-6.2 Sol', query: '6.2 Sol', id: 'gpt-6.2-sol', provider: 'openai', harness: 'codex', tier: 'Max', effort: 'max' },
  { label: 'GPT-6.2 Sol', query: '6.2 Sol', id: 'gpt-6.2-sol', provider: 'openai', harness: 'codex', tier: 'Ultra', effort: 'ultra' },
  { label: 'Gemini 4 Hydrogen', query: 'Hydrogen', id: 'gemini-4-hydrogen', provider: 'google', harness: 'antigravity', tier: 'Extra high', effort: 'xhigh' },
]) {
  test(`starting a task on ${choice.label} at ${choice.tier} uses its model and effort`, async ({ app, browser, screen }) => {
    await app.open(mock('/s/studio'));
    await browser.evaluate((provider) => {
      (window as unknown as Mocked).__yuliusMock.accounts.push({ id: 'acc_test', provider, label: 'Test', status: 'ok' });
    }, choice.provider);
    await screen.getByRole('button', /^Model:/).click();
    await screen.getByRole('searchbox', 'Filter').fill(choice.query);
    await screen.getByRole('menuitemradio', new RegExp('^' + choice.label)).click();
    await screen.getByRole('button', /^Effort:/).click();
    await expect(screen.getByRole('menuitemradio', /^Default/)).toContainText('High');
    await screen.getByRole('menuitemradio', new RegExp('^' + choice.tier)).click();
    const prompt = screen.getByRole('textbox', 'Task prompt');
    await prompt.fill('Add a health check');
    await prompt.press('ControlOrMeta+Enter');
    await expect(browser).toHaveURL(/#\/s\/studio\/t\/\d+$/);
    const task = await browser.evaluate(() => (window as unknown as Mocked).__yuliusMock.tasks.at(-1));
    const submitted = await browser.evaluate(() => (window as unknown as Mocked).__yuliusMock.lastTaskCreate);
    expect(submitted).toMatchObject({ harness: choice.harness, model: choice.id, effort: choice.effort });
    expect(task).toMatchObject({ harness: choice.harness, model: choice.id, effort: choice.effort, model_default: false });
  });
}

test('Settings preserves compatible effort and clears incompatible choices in the composer', async ({ app, browser, screen }) => {
  await app.open(mock('/settings'));
  const model = screen.getByRole('button', /^Model for new tasks:/);
  const effort = screen.getByRole('button', /^Effort for new tasks:/);
  await model.click();
  await screen.getByRole('menuitemradio', /^GPT-6.2 Astra/).click();
  await effort.click();
  await expect(screen.getByRole('menuitemradio', /^Default/)).toContainText('High');
  await screen.getByRole('menuitemradio', /^Ultra/).click();
  await model.click();
  await screen.getByRole('menuitemradio', /^GPT-6.1 Terra/).click();
  await expect(effort).toContainText('Ultra');
  await model.click();
  await screen.getByRole('menuitemradio', /^GPT-6 Luna/).click();
  await expect(effort).toContainText('Default effort');
  await effort.click();
  await expect(screen.getByRole('menuitemradio', /^Ultra/)).toHaveCount(0);
  await screen.getByRole('menuitemradio', /^Max/).click();
  await app.open(mock('/s/studio'));
  await expect(screen.getByRole('button', 'Effort: Max')).toBeVisible();
  await screen.getByRole('button', /^Model:/).click();
  await screen.getByRole('menuitemradio', /^Random/).click();
  await expect(screen.getByRole('button', 'Effort: Default effort')).toBeVisible();
  await screen.getByRole('button', /^Effort:/).click();
  await expect(screen.getByRole('menuitemradio', /^Default/)).toContainText('High');
  const randomEfforts = await browser.evaluate(() => Array.from(document.querySelectorAll('.menu .menu-label'), (el) => el.textContent));
  expect(randomEfforts).toEqual(['Default', 'Low', 'Medium', 'High', 'Extra high']);
  await screen.getByRole('menuitemradio', /^Default/).press('Escape');
  await screen.getByRole('button', /^Model:/).click();
  await screen.getByRole('menuitemradio', /^GPT-6 Luna/).click();
  await screen.getByRole('button', /^Effort:/).click();
  await screen.getByRole('menuitemradio', /^Max/).click();
  await screen.getByRole('button', /^Model:/).click();
  await screen.getByRole('menuitemradio', /^Gemini 4 Carbon/).click();
  await expect(screen.getByRole('button', 'Effort: Default effort')).toBeVisible();
  await screen.getByRole('button', /^Effort:/).click();
  await expect(screen.getByRole('menuitemradio')).toHaveCount(4);
  await screen.getByRole('menuitemradio', /^Low/).click();
  await app.open(mock('/settings'));
  await expect(effort).toContainText('Low');
  await model.click();
  await screen.getByRole('menuitemradio', /^Haiku/).click();
  await expect(effort).toBeHidden();
  // A browser preference written by an older client cannot submit a tier the model lacks.
  await browser.evaluate(() => {
    localStorage.setItem('yulius.composer.model', JSON.stringify('gemini-3.1-pro'));
    localStorage.setItem('yulius.composer.effort', JSON.stringify('medium'));
  });
  await app.open(mock('/s/studio'));
  await expect(screen.getByRole('button', 'Effort: Default effort')).toBeVisible();
  await screen.getByRole('button', /^Effort:/).click();
  await expect(screen.getByRole('menuitemradio')).toHaveCount(3);
  await expect(screen.getByRole('menuitemradio', /^Medium/)).toHaveCount(0);
  await screen.getByRole('menuitemradio', /^Default/).press('Escape');
  await browser.evaluate(() => {
    (window as unknown as Mocked).__yuliusMock.accounts.push({ id: 'acc_test', provider: 'google', label: 'Test', status: 'ok' });
  });
  const prompt = screen.getByRole('textbox', 'Task prompt');
  await prompt.fill('Use the restored model');
  await prompt.press('ControlOrMeta+Enter');
  await expect(browser).toHaveURL(/#\/s\/studio\/t\/\d+$/);
  const task = await browser.evaluate(() => (window as unknown as Mocked).__yuliusMock.tasks.at(-1));
  const submitted = await browser.evaluate(() => (window as unknown as Mocked).__yuliusMock.lastTaskCreate);
  expect(submitted).toMatchObject({ harness: 'antigravity', model: 'gemini-3.1-pro' });
  expect(Object.hasOwn(submitted, 'effort')).toBe(false);
  expect(task).toMatchObject({ model: 'gemini-3.1-pro', effort: 'high' });
});

test('each provider group lists its reasoning models with the tiers the CLI accepts', async ({ app, browser, screen }) => {
  await app.open(mock('/s/studio'));
  const openModels = screen.getByRole('button', /^Model:/);
  const openEffort = screen.getByRole('button', /^Effort:/);
  const standard = ['Low', 'Medium', 'High'];
  const extra = [...standard, 'Extra high'];
  const max = [...extra, 'Max'];
  const ultra = [...max, 'Ultra'];
  const providers = [
    { name: 'OpenAI', models: [
      { names: ['Codex', 'GPT-6.2 Astra', 'GPT-6.2 Sol', 'GPT-6.1 Terra', 'GPT-6.1 Sol', 'GPT-6 Astra', 'GPT-6 Sol', 'GPT-5.6 Sol', 'GPT-5.6 Terra'], tiers: ultra },
      { names: ['GPT-6 Luna', 'GPT-5.6 Luna'], tiers: max },
      { names: ['GPT-5.5'], tiers: extra },
    ] },
    { name: 'Google', models: [
      { names: ['Gemini 4 Neon', 'Gemini 4 Hydrogen'], tiers: extra },
      { names: ['Gemini 4 Carbon', 'Gemini 4 Argon', 'Gemini 3.8 Flash', 'Gemini 3.7 Flash', 'Gemini 3.6 Flash'], tiers: standard },
      { names: ['Gemini 3.1 Pro'], tiers: ['Low', 'High'] },
    ] },
  ];
  for (const provider of providers) {
    await openModels.click();
    const group = screen.getByRole('group', provider.name);
    const names = provider.models.flatMap((family) => family.names);
    await expect(group.getByRole('menuitemradio')).toHaveCount(names.length);
    for (const family of provider.models) {
      for (const name of family.names) {
        await group.getByRole('menuitemradio', new RegExp('^' + name)).click();
        await openEffort.click();
        const labels = await browser.evaluate(() => Array.from(document.querySelectorAll('.menu .menu-label'), (el) => el.textContent));
        expect(labels).toEqual(['Default', ...family.tiers]);
        await screen.getByRole('menuitemradio', /^Default/).click();
        await openModels.click();
      }
    }
    await screen.getByRole('menuitemradio', /^Random/).click();
  }
});
