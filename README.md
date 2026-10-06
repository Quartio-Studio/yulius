# Yulius

A coding sandbox for the Quartio team, inspired by
[Cezar](https://github.com/open-mercato/cezar), to be built in Rust.

This checkout currently contains the engineering foundation: repository rules,
agent skills, templates and reproducible development checks. Product implementation
will follow the detailed brief. The only Rust executable is the development task runner.

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
- [Engineering workflow](docs/engineering/workflow.md): specify, implement, verify and review.
- [Rust rules](docs/engineering/rust.md) and [dependency boundaries](docs/engineering/architecture.md).
- [Repository skills](.agents/skills): specification, Rust implementation, runners, verification and review.
- [Future specification guidance](docs/engineering/specifications.md).

Checks need no production credentials, database or model subscription.
[MIT license](LICENSE).
