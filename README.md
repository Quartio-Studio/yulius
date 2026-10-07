# Yulius

A coding sandbox for the Quartio team, inspired by
[Cezar](https://github.com/open-mercato/cezar), to be built in Rust.

The product is specified in [SPEC-001](.ai/specs/SPEC-001-yulius-product.md) (product),
[SPEC-002](.ai/specs/SPEC-002-yulius-architecture.md) (architecture and runtime contracts) and
[SPEC-003](.ai/specs/SPEC-003-traceability.md) (source traceability). Implementation progress is
tracked per checkpoint in GitHub issues; each spec's evidence section records what is verified.

## Start

Prerequisites: Python 3.11+, [rustup](https://rustup.rs), Git and a platform C linker.

```sh
python3 scripts/bootstrap.py
scripts/dev doctor
scripts/dev ci
```

Pinned tools install into ignored `.tools/`; the default Rust toolchain is unchanged.
See [development instructions](docs/engineering/development.md) for commands and CI.

- [AGENTS.md](AGENTS.md): coding-agent entry point.
- [Git delivery](docs/engineering/git.md): feature → dev → main, self-merges and cleanup.
- [Engineering workflow](docs/engineering/workflow.md): specify, implement, verify and review.
- [Rust rules](docs/engineering/rust.md) and [dependency boundaries](docs/engineering/architecture.md).
- [Repository skills](.agents/skills): specification, Rust implementation, runners, verification and review.
- [Future specification guidance](docs/engineering/specifications.md).

Checks need no production credentials, database or model subscription.
[MIT license](LICENSE).
