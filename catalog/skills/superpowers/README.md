# Superpowers skills (adapted)

Two skills adapted from [obra/superpowers](https://github.com/obra/superpowers) and shipped with
Yulius because they serve its delivery pipeline directly: a test that fails without the change,
and evidence before every claim. Thank you to Jesse Vincent and the superpowers contributors for
publishing them under the MIT license.

- Source: <https://github.com/obra/superpowers>, directory `skills/<name>/`
- Upstream commit: `bb92a77741419a4ab5f06e711a283343f1ada0c3`
- License: MIT, see [`LICENSE`](LICENSE) (the upstream license, kept unchanged)

| Skill | Upstream | Purpose |
| --- | --- | --- |
| `test-driven-development` | `test-driven-development` (with `writing-good-tests.md`) and `systematic-debugging/condition-based-waiting.md` | Red, green, refactor; tests that name the break they catch; reliable, non-flaky tests |
| `verification-before-completion` | `verification-before-completion` | Run the command that proves a claim before making it |

## What changed

The skills are **adapted, not byte-for-byte copies**:

- rephrased for autonomous agents: no step defers to a person; exceptions are decided by the
  agent and recorded in the spec or pull request;
- the flaky-test rules of the Yulius pipeline (no retries until green, injected clocks, seeded
  randomness, own test data, quarantine with an issue) were added to the condition-based waiting
  reference, which now lives with `test-driven-development`;
- references to other superpowers skills that Yulius does not ship were removed, and each
  `SKILL.md` carries a visible attribution note.

Yulius embeds these files in its binary and extracts them to `<data_dir>/skills/superpowers/` at
startup; they are enabled per sandbox on the Skills page (default: enabled). They work with the
adapted Open Mercato skills (`om-fix`, `om-code-review`, `om-check-and-commit`, ...) and Yulius's
own `delivery-pipeline`, `infra-bootstrap`, `git-hygiene` and `dead-code-cleanup`.
