// Models and effort levels the new-task composer offers (SPEC-001 UI-021). Grouped by provider so
// another harness's models can join the list; each provider names the harness that runs them.
// Yulius runs Claude Code, Codex (SPEC-503) and Antigravity (SPEC-505). A Claude model id goes to
// Claude Code's `--model` and a level to `--effort`; choosing "Random" sends no model and no agent,
// so the server draws uniformly across Claude Code (on Yulius's default, Opus 5.5 at high effort;
// SPEC-023, SPEC-462, SPEC-493), Codex and Antigravity among those with non-exhausted
// subscriptions (SPEC-520 RHM-001/006). The browser starts on Codex default (SPEC-587).
// Choosing Codex default sends `harness: codex` and no model: the server runs GPT-6.2 Astra, at
// high effort unless one is chosen (SPEC-607). An explicit Codex model sends its id. Choosing a
// Gemini model sends `harness: antigravity` and the model; Antigravity's default is Gemini 4
// Neon at high effort (SPEC-519; Gemini 4 Carbon before, SPEC-505).

import { readJson, writeJson } from './store.js';

/** This browser's defaults for new tasks (the composer and Settings share them). */
export const MODEL_PREF_KEY = 'yulius.composer.model';
export const EFFORT_PREF_KEY = 'yulius.composer.effort';
export const DEFAULT_EFFORT = 'high';

const STANDARD = ['low', 'medium', 'high'];
const EXTRA_HIGH = [...STANDARD, 'xhigh'];
const MAX = [...EXTRA_HIGH, 'max'];
const ULTRA = [...MAX, 'ultra'];

export const PROVIDERS = [
  {
    id: 'anthropic',
    name: 'Anthropic',
    harness: 'claude',
    efforts: MAX,
    models: [
      { id: 'claude-opus-5-5', name: 'Opus 5.5', description: 'Complex coding and agentic work' },
      { id: 'claude-fable-5-2', name: 'Fable 5.2', description: 'Most capable, for the hardest long-running work (needs Fable access)' },
      { id: 'claude-fable-5-1', name: 'Fable 5.1', description: 'The previous Fable (needs Fable access)' },
      { id: 'claude-sonnet-5-5', name: 'Sonnet 5.5', description: 'Fast and capable for everyday coding' },
      // Haiku 4.5 takes no effort level.
      { id: 'claude-haiku-4-5', name: 'Haiku 4.5', description: 'Fastest, for small and quick tasks', efforts: [] },
    ],
  },
  {
    id: 'openai',
    name: 'OpenAI',
    harness: 'codex',
    efforts: ULTRA,
    models: [
      // No model id: the server runs its Codex default, GPT-6.2 Astra (`harnessOnly`, SPEC-511).
      { id: 'codex', name: 'Codex default', description: 'GPT-6.2 Astra at high effort by default', harnessOnly: true },
      { id: 'gpt-6.2-astra', name: 'GPT-6.2 Astra' },
      { id: 'gpt-6.2-sol', name: 'GPT-6.2 Sol' },
      { id: 'gpt-6.1-sol', name: 'GPT-6.1 Sol' },
      { id: 'gpt-6.1-terra', name: 'GPT-6.1 Terra' },
      { id: 'gpt-6-astra', name: 'GPT-6 Astra' },
      { id: 'gpt-6-sol', name: 'GPT-6 Sol' },
      { id: 'gpt-6-luna', name: 'GPT-6 Luna', efforts: MAX },
      { id: 'gpt-5.6-sol', name: 'GPT-5.6 Sol' },
      { id: 'gpt-5.6-terra', name: 'GPT-5.6 Terra' },
      { id: 'gpt-5.6-luna', name: 'GPT-5.6 Luna', efforts: MAX },
      { id: 'gpt-5.5', name: 'GPT-5.5', efforts: EXTRA_HIGH },
    ],
  },
  {
    id: 'google',
    name: 'Google',
    harness: 'antigravity',
    efforts: STANDARD,
    // Without an effort chosen, the server runs an Antigravity task at high effort.
    models: [
      { id: 'gemini-4-neon', name: 'Gemini 4 Neon', description: 'Antigravity, high effort by default', efforts: EXTRA_HIGH },
      { id: 'gemini-4-carbon', name: 'Gemini 4 Carbon' },
      { id: 'gemini-4-hydrogen', name: 'Gemini 4 Hydrogen', efforts: EXTRA_HIGH },
      { id: 'gemini-4-argon', name: 'Gemini 4 Argon' },
      { id: 'gemini-3.8-flash', name: 'Gemini 3.8 Flash' },
      { id: 'gemini-3.7-flash', name: 'Gemini 3.7 Flash', description: 'Enterprise' },
      { id: 'gemini-3.6-flash', name: 'Gemini 3.6 Flash', description: 'Enterprise' },
      { id: 'gemini-3.1-pro', name: 'Gemini 3.1 Pro', description: 'Enterprise', efforts: ['low', 'high'] },
    ],
  },
];

/**
 * The model a task of a Fable model runs while no subscription can run Fable (SPEC-482): the
 * server's `FALLBACK_CLAUDE_MODEL`, which a test keeps equal to this.
 */
export const FALLBACK_MODEL = 'claude-opus-5-5';

/**
 * The Fable model the pages watch for that fallback (SPEC-493): the server's
 * `REVIEW_CLAUDE_MODEL`, which a test keeps equal to this. The default model has no fallback.
 */
export const REVIEW_MODEL = 'claude-fable-5-2';

/** The model a task of `model` falls back to, or null: a Fable model's is [`FALLBACK_MODEL`]. */
export function fallbackOf(model) {
  return String(model || '').toLowerCase().includes('fable') ? FALLBACK_MODEL : null;
}

/** What the pool says while tasks of `model` run its fallback (SPEC-482), in one line. */
export function fallbackLine(model, fallback) {
  return `${modelLabel(model)} is used up on every subscription: tasks set to it run on ${modelLabel(fallback)} until it can run again`;
}

/** The request fields of a model choice: `{model?, harness?}` (none for "Random"). */
export function modelFields(id) {
  const found = findModel(id);
  if (!found) return {};
  const fields = found.model.harnessOnly ? {} : { model: found.model.id };
  if (found.provider.harness !== 'claude') fields.harness = found.provider.harness;
  return fields;
}

export const EFFORTS = [
  { id: 'low', name: 'Low', description: 'Quick, fewest tokens' },
  { id: 'medium', name: 'Medium', description: 'Balanced speed and depth' },
  { id: 'high', name: 'High', description: 'Thorough reasoning' },
  { id: 'xhigh', name: 'Extra high', description: 'Deeper reasoning for complex work' },
  { id: 'max', name: 'Max', description: 'More time for complex work' },
  { id: 'ultra', name: 'Ultra', description: 'Deep reasoning with automatic delegation' },
];

/** A model id as people say it: "Opus 5.5" for a listed model, "Opus 4" for claude-opus-4. */
export function modelLabel(id) {
  const known = findModel(id);
  if (known) return known.model.name;
  const m = /^claude-([a-z]+)-(\d+)(?:-(\d{1,2}))?(?:-|$)/.exec(String(id || ''));
  return m ? `${m[1][0].toUpperCase()}${m[1].slice(1)} ${m[2]}${m[3] ? `.${m[3]}` : ''}` : String(id || 'Unknown model');
}

/** {provider, model} for a model id, or null. */
export function findModel(id) {
  for (const provider of PROVIDERS) {
    const model = provider.models.find((m) => m.id === id);
    if (model) return { provider, model };
  }
  return null;
}

/** The effort level with this id, or null. */
export function findEffort(id) {
  return EFFORTS.find((e) => e.id === id) || null;
}

function effortLevels(modelId) {
  const found = findModel(modelId);
  // Random (`modelId === null`) may draw Gemini 4 Neon (`EXTRA_HIGH`), Opus 5.5 (`MAX`) or
  // GPT-6.2 Astra (`ULTRA`), so offer the common `EXTRA_HIGH` tiers accepted by all three defaults.
  return found?.model.efforts ?? found?.provider.efforts ?? EXTRA_HIGH;
}

/** A browser's new-task picks, shared by Settings and the composer. */
export function modelPreferences() {
  // Choosing Random saves JSON null. An absent or unreadable key returns undefined.
  const savedModel = readJson(MODEL_PREF_KEY, undefined);
  const model = savedModel === null ? null : findModel(savedModel)?.model.id ?? 'codex';
  const saved = readJson(EFFORT_PREF_KEY, null);
  return {
    model,
    effort: effortLevels(model).includes(saved) ? saved : null,
    setModel(id) {
      this.model = id;
      writeJson(MODEL_PREF_KEY, id);
      this.setEffort(this.effort);
    },
    setEffort(id) {
      this.effort = effortLevels(this.model).includes(id) ? id : null;
      writeJson(EFFORT_PREF_KEY, this.effort);
    },
  };
}

/** Menu items to pick a model (grouped by provider); `onPick(id|null)`. */
export function modelMenuItems(selected, onPick) {
  return [
    // No model chosen: the agent is drawn uniformly for each task across Claude Code (Opus 5.5),
    // Codex (GPT-6.2 Astra) and Antigravity (Gemini 4 Neon), among those with non-exhausted
    // subscriptions (SPEC-520 RHM-001/006, SPEC-519 NEO-004).
    { label: 'Random', description: 'Claude Code (Opus 5.5), Codex (GPT-6.2 Astra) or Antigravity (Gemini 4 Neon), drawn for each task', checked: selected === null, onSelect: () => onPick(null) },
    ...PROVIDERS.map((provider) => ({
      header: provider.name,
      items: provider.models.map((model) => ({ label: model.name, description: model.description, checked: model.id === selected, onSelect: () => onPick(model.id) })),
    })),
  ];
}

/** Menu items to pick an effort level; `onPick(id|null)`. */
export function effortMenuItems(model, selected, onPick) {
  return [
    { label: 'Default', description: findEffort(DEFAULT_EFFORT).name, checked: selected === null, onSelect: () => onPick(null) },
    ...effortLevels(model).map(findEffort).map((effort) => ({ label: effort.name, description: effort.description, checked: effort.id === selected, onSelect: () => onPick(effort.id) })),
  ];
}
