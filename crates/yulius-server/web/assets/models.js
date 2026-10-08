// Models and effort levels the new-task composer offers (SPEC-001 UI-021). Grouped by provider so
// another harness's models can join the list; each provider names the harness that runs them.
// Yulius runs Claude Code only today (TSK-003). A model id goes to Claude Code's `--model` and a
// level to `--effort`; choosing "Default" sends neither and leaves it to Claude Code.

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
