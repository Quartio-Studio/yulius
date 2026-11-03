# 0258. Test fixtures hold no provider-shaped credential literal

## Status

Accepted (2026-11-03, task #388, [SPEC-406](../../.ai/specs/SPEC-406-vidoc-triage-credentials-in-test-code-82.md)).
Applies AGENTS.md ("Use synthetic/redacted fixtures") and SEC-002 of the runtime contracts.

## Context

The product finds credentials by their provider prefix: transcript and output redaction
(`yulius-domain` `redact.rs`: `sk-ant-`, `sk_live_`, `sk_test_`, `ghp_`, …) and the sign-in's token
extraction (`CLAUDE_TOKEN_PREFIX`, `sk-ant-oat01-`). So tests need values of those shapes at run
time.

Before this change, 26 literals in 17 files under `crates/yulius/tests` held a real provider
prefix and a body, such as an Anthropic OAuth prefix with `test-credential` or a Stripe test key
prefix with a made-up body. All of them were synthetic. Vidoc reported two of them (#822, #797),
and the person who asked for the triage said to treat a real token prefix as real. The values are
not live, so the risk is their shape:

- Secret scanners and GitHub push protection match the shape. Committed lookalikes are noise
  that people learn to dismiss.
- A real token pasted into a fixture looks just like the synthetic ones around it.

## Decision

- **No provider-shaped literal in `crates/yulius/tests`.** A fixture that needs the shape is
  `concat!` of the prefix and a synthetic body, for example `concat!("sk_test_", "51NzQ9…")`. The
  value, and each test's behavior, stay the same. Fixtures stay `const`.
- **A test keeps it that way.** `fixture_hygiene::test_fixtures_hold_no_provider_shaped_credential`
  reads every file under `crates/yulius/tests`. It fails on a word that starts with a prefix of
  the redaction rules (`redact.rs` `TOKEN_RULES`) and goes on with 8 or more token characters. For
  prefixes that are also parts of common words (`npm_`, `hf_`, `secret_`, `yul_`, legacy `sk-`, …)
  it takes the rule's own minimum, and AWS key ids need 16 upper-case letters or digits. It names
  the file, the line, the prefix and the body's length, never the body. A prefix alone, a
  `format!` template (`"ghp_{}"`), a mask (`…[REDACTED]`) and a prefix inside a word
  (`network_test_…`) pass.

## Alternatives

- **A run-time helper (`synthetic_credential(prefix, body)`).** It is the same value built later,
  and it would turn every `const` fixture and its call sites into function calls. Rejected for
  `concat!`.
- **A random value per run, as Vidoc suggested.** Tests compare against fixed values and seal them
  into stores, and a fixed value keeps failures reproducible. Rejected.
- **Values without a provider prefix.** The redaction and sign-in tests need the prefix to
  exercise the product's detection. Rejected where the shape matters, and not worth the churn
  elsewhere.
- **Ignoring the findings.** That would leave the shape in place, and a pasted real token would
  pass unnoticed. Rejected.

## Consequences

- A real token pasted into a test under `crates/yulius/tests` fails the gate, and the failure
  message does not repeat it.
- Splitting a literal still passes the test. That is deliberate: the test catches whole
  credentials, the kind scanners match and people paste, not intent.
- Provider-shaped fixtures elsewhere (`yulius-harness/tests`, `yulius-egress/tests` and
  `#[cfg(test)]` modules in `src`) are outside this check.
- A repeat of #797 or #822 in the same wording matches its `[[ignore]]` entry in
  `.config/vidoc.toml`.
