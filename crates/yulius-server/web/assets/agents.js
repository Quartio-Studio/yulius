// The agents Yulius runs (SPEC-503, SPEC-505): each harness, the provider of its subscriptions and
// how the pages name it. A task keeps its harness for life and runs only on its provider's
// subscriptions, so whatever a task or a subscription says names its own agent.

/**
 * Each agent: its harness, its subscriptions' provider, its CLI's name, its short name and the
 * vendor behind its subscriptions (SPEC-513: a subscription card's badge names both).
 */
export const AGENTS = [
  { harness: 'claude', provider: 'anthropic', cli: 'Claude Code', name: 'Claude', article: 'a', vendor: 'Anthropic' },
  { harness: 'codex', provider: 'openai', cli: 'Codex', name: 'Codex', article: 'a', vendor: 'OpenAI' },
  { harness: 'antigravity', provider: 'google', cli: 'Antigravity', name: 'Antigravity', article: 'an', vendor: 'Google Gemini' },
];

/** The agent of a harness (`claude` when not known). */
export function agentOfHarness(harness) {
  return AGENTS.find((agent) => agent.harness === harness) || AGENTS[0];
}

/** The agent of a subscription's provider (Claude's when not known). */
export function agentOfProvider(provider) {
  return AGENTS.find((agent) => agent.provider === provider) || AGENTS[0];
}

/** "a Claude", "a Codex", "an Antigravity": the agent's short name with its article. */
export function aName(agent) {
  return `${agent.article} ${agent.name}`;
}
