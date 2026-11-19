// Models and effort levels the new-task composer offers (SPEC-001 UI-021). Grouped by provider so
// another harness's models can join the list; each provider names the harness that runs them.
// Yulius runs Claude Code, Codex (SPEC-503) and Antigravity (SPEC-505). A Claude model id goes to
// Claude Code's `--model` and a level to `--effort`; choosing "Random" sends no model and no agent,
// so the server draws Claude Code (on Yulius's default, Opus 5.5 at high effort; SPEC-023,
// SPEC-462, SPEC-493) or Antigravity among the usable agents (SPEC-505 RND-001). The first model
// listed is the default, as on the server. Choosing Codex sends `harness: codex` and no model:
// the server runs GPT-6.2 Astra, at high effort unless one is chosen (SPEC-511). Choosing a
// Gemini model sends `harness: antigravity` and the model; Antigravity's default is Gemini 4
// Carbon at high effort.

/** This browser's defaults for new tasks (the composer and Settings share them). */
export const MODEL_PREF_KEY = 'yulius.composer.model';
export const EFFORT_PREF_KEY = 'yulius.composer.effort';

export const PROVIDERS = [
  {
    id: 'anthropic',
    name: 'Anthropic',
    harness: 'claude',
    models: [
      { id: 'claude-opus-5-5', name: 'Opus 5.5', description: 'Complex coding and agentic work' },
      { id: 'claude-fable-5-2', name: 'Fable 5.2', description: 'Most capable, for the hardest long-running work (needs Fable access)' },
      { id: 'claude-fable-5-1', name: 'Fable 5.1', description: 'The previous Fable (needs Fable access)' },
      { id: 'claude-sonnet-5-5', name: 'Sonnet 5.5', description: 'Fast and capable for everyday coding' },
      // Haiku 4.5 takes no effort level.
      { id: 'claude-haiku-4-5', name: 'Haiku 4.5', description: 'Fastest, for small and quick tasks', effort: false },
    ],
  },
  {
    id: 'openai',
    name: 'OpenAI',
    harness: 'codex',
    models: [
      // No model id: the server runs its Codex default, GPT-6.2 Astra (`harnessOnly`, SPEC-511).
      { id: 'codex', name: 'Codex', description: 'OpenAI Codex CLI on a Codex account, GPT-6.2 Astra at high effort by default', harnessOnly: true },
    ],
  },
  {
    id: 'google',
    name: 'Google',
    harness: 'antigravity',
    // Without an effort chosen, the server runs an Antigravity task at high effort.
    models: [
      { id: 'gemini-4-carbon', name: 'Gemini 4 Carbon', description: 'Antigravity CLI on an Antigravity account, high effort by default' },
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
  { id: 'xhigh', name: 'Extra high', description: 'Deep reasoning, best for most coding' },
  { id: 'max', name: 'Max', description: 'Deepest reasoning, slowest' },
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

/** Menu items to pick a model (grouped by provider); `onPick(id|null)`. */
export function modelMenuItems(selected, onPick) {
  return [
    // No model chosen: the agent is drawn for each task, Claude Code (on Opus 5.5) or Antigravity
    // (on Gemini 4 Carbon), among those with a usable subscription (SPEC-505 RND-001).
    { label: 'Random', description: 'Claude Code (Opus 5.5) or Antigravity (Gemini 4 Carbon), drawn for each task', checked: selected === null, onSelect: () => onPick(null) },
    ...PROVIDERS.flatMap((provider) => [
      { header: provider.name },
      ...provider.models.map((model) => ({ label: model.name, description: model.description, checked: model.id === selected, onSelect: () => onPick(model.id) })),
    ]),
  ];
}

/** Menu items to pick an effort level; `onPick(id|null)`. */
export function effortMenuItems(selected, onPick) {
  return [
    { label: 'Default', description: 'High (none for Haiku)', checked: selected === null, onSelect: () => onPick(null) },
    ...EFFORTS.map((effort) => ({ label: effort.name, description: effort.description, checked: effort.id === selected, onSelect: () => onPick(effort.id) })),
  ];
}
