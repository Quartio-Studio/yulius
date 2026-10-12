// Models and effort levels the new-task composer offers (SPEC-001 UI-021). Grouped by provider so
// another harness's models can join the list; each provider names the harness that runs them.
// Yulius runs Claude Code only today (TSK-003). A model id goes to Claude Code's `--model` and a
// level to `--effort`; choosing "Default" sends neither and the task runs Yulius's default,
// Opus 5.5 at high effort (task #21).

/** This browser's defaults for new tasks (the composer and Settings share them). */
export const MODEL_PREF_KEY = 'yulius.composer.model';
export const EFFORT_PREF_KEY = 'yulius.composer.effort';

export const PROVIDERS = [
  {
    id: 'anthropic',
    name: 'Anthropic',
    harness: 'claude',
    models: [
      { id: 'claude-fable-5-1', name: 'Fable 5.1', description: 'Most capable, for the hardest long-running work (needs Fable access)' },
      { id: 'claude-opus-5-5', name: 'Opus 5.5', description: 'Complex coding and agentic work' },
      { id: 'claude-sonnet-5-5', name: 'Sonnet 5.5', description: 'Fast and capable for everyday coding' },
      // Haiku 4.5 takes no effort level.
      { id: 'claude-haiku-4-5', name: 'Haiku 4.5', description: 'Fastest, for small and quick tasks', effort: false },
    ],
  },
];

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
    { label: 'Default', description: 'Opus 5.5', checked: selected === null, onSelect: () => onPick(null) },
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
