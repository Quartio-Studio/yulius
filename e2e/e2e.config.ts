// End-to-end tests of the web UI (#86). The runner serves crates/yulius-server/web with Python's
// static file server and the tests open it in mock mode (`?mock=1`), so no `yulius serve`,
// account or model is needed. Agent steps (`agent.act`, `agent.assert`) need a model: they use
// Anthropic when ANTHROPIC_API_KEY is set and are skipped otherwise. CI uses the same budgets and
// no retries: its jobs get the CPU before the agents' builds (SPEC-053), so a timeout is a failure.
import type { E2EConfig } from 'e2e';
import { anthropic } from '@ai-sdk/anthropic';
import { web } from '@e2e-dev/web';
import { fileURLToPath } from 'node:url';

const ui = fileURLToPath(new URL('../crates/yulius-server/web', import.meta.url));

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
} satisfies E2EConfig;
