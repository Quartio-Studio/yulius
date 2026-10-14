// End-to-end tests of the web UI (#86). The runner serves crates/yulius-server/web with Python's
// static file server and the tests open it in mock mode (`?mock=1`), so no `yulius serve`,
// account or model is needed. Agent steps (`agent.act`, `agent.assert`) need a model: they use
// Anthropic when ANTHROPIC_API_KEY is set and are skipped otherwise. CI uses the same budgets and
// no retries, so a timeout is a failure: they held under agents' builds (SPEC-053 Evidence).
import type { E2EConfig } from 'e2e';
import { anthropic } from '@ai-sdk/anthropic';
import { web } from '@e2e-dev/web';
import { fileURLToPath } from 'node:url';

const ui = fileURLToPath(new URL('../crates/yulius-server/web', import.meta.url));
// Where Yulius runs the suite (agent sessions, CI jobs) it gives the process a CPU budget
// (SPEC-082). Like the runner's own default of half the cores, half the budget: a worker is a
// browser and the runner. Elsewhere the runner's default (half the cores; one in CI).
const budget = Number.parseInt(process.env.YULIUS_CPUS ?? '', 10);
const workers = Number.isInteger(budget) && budget > 0 ? Math.max(1, Math.floor(budget / 2)) : undefined;

export default {
  tests: 'tests/**/*.e2e.ts',
  targets: [
    {
      name: 'chromium',
      engine: web({ viewport: { width: 1440, height: 900 } }),
      app: {
        url: 'http://127.0.0.1:0',
        command: {
          executable: 'python3',
          args: ['-I', '-m', 'http.server', '{port}', '--bind', '127.0.0.1', '--directory', ui],
          log: '.e2e/logs/server.log',
        },
      },
    },
  ],
  ...(process.env.ANTHROPIC_API_KEY
    ? { agents: { default: { model: anthropic(process.env.E2E_MODEL ?? 'claude-sonnet-5-5') } } }
    : {}),
  // Verified agent actions are recorded here and replayed without model calls; the directory is
  // committed. CI only reads it.
  cache: { dir: 'replays' },
  // The runner retries once in CI by default; a retry would hide a slow or flaky test.
  retries: 0,
  ...(workers ? { workers } : {}),
} satisfies E2EConfig;
