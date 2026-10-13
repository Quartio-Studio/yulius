---
name: dead-code-cleanup
description: Find and remove dead code, unused imports, unused dependencies and stale files with each language's established tool (vulture and skylos for Python, cargo-shear for Rust, knip for JavaScript and TypeScript, deadcode for Go, and others), verifying every candidate before deleting it and keeping the gate green. Use when asked to clean up, tidy, prune or slim a codebase, remove dead or unused code, or add a dead-code check to a project's gate.
---

# Dead-code cleanup

Dead code costs every later reader and every agent's context window. Remove it with tools, not
by eye, and prove each removal safe: a tool reports candidates, you decide. The project's own
instructions win where they say more.

## 1. Use what the project already has

Read AGENTS.md, the gate and the lint configuration first. When the project already runs a
dead-code or unused-dependency check, use that exact command and its allowlist. Otherwise pick
the established tool for each language the project has from the table below, run it through the
ecosystem's one-shot runner (`uvx`, `pipx run`, `npx`, `go run`), and do not add it to the
project's dependencies unless you are wiring it into the gate.

## 2. Tools per language

| Language | Dead code | Unused dependencies |
| --- | --- | --- |
| Python | `vulture . --min-confidence 80` (fast, static; `--make-whitelist` for an allowlist file); `skylos . --no-upload` (framework-aware, confidence scores); `ruff check --select F401,F841` (imports, variables) | `deptry .` |
| Rust | the compiler's `dead_code`/`unused_*` lints as errors (`cargo clippy --all-targets -- -D warnings`); they skip items exported from a library crate, so grep the workspace for other callers | `cargo shear` (also workspace dependencies), `cargo machete`, `cargo +nightly udeps` (exact, slow) |
| JavaScript, TypeScript | `knip` (unused files, exports, types); `tsc --noEmit --noUnusedLocals --noUnusedParameters`; ESLint `no-unused-vars` | `knip` |
| Go | `go run golang.org/x/tools/cmd/deadcode@latest ./...` (unreachable functions); `staticcheck` (`U1000`) | `go mod tidy` and a diff |
| Java, Kotlin | IntelliJ "Unused declaration" inspection; PMD (`UnusedPrivateMethod`, `UnusedPrivateField`); detekt (`UnusedPrivateMember`) for Kotlin | Gradle `dependency-analysis` plugin; Maven `dependency:analyze` |
| C#, .NET | analyzers `IDE0051`, `IDE0052`, `IDE0060`, `CA1823` as errors in `.editorconfig`, with `<EnforceCodeStyleInBuild>true</EnforceCodeStyleInBuild>` so `dotnet build` enforces them; `jb inspectcode` | search each `PackageReference`'s namespaces |
| PHP | PHPStan with `shipmonk/dead-code-detector` | `composer-unused` |
| Ruby | `debride`; RuboCop `Lint/UselessAssignment`, `Lint/UselessMethodDefinition` | search each Gemfile gem's `require` and constant |
| Swift | `periphery scan` | `periphery scan` (unused imports) |
| Dart, Flutter | `dart analyze` (`unused_element`, `unused_import`); DCM `check-unused-code`, `check-unused-files` (commercial licence) | `dependency_validator` |
| C, C++ | `-Wunused` as errors; `cppcheck --enable=unusedFunction`; build with `-ffunction-sections -fdata-sections -Wl,--gc-sections,--print-gc-sections` to list unreferenced functions | `include-what-you-use` for headers |
| Elixir | `mix compile --warnings-as-errors` (unused private functions, variables, aliases); `mix xref graph` | `mix deps.unlock --check-unused` (stale lock entries); search for each `mix.exs` dependency's modules |

Python's vulture and skylos come from the ML in PL agentic-coding workshop
(https://ktfish.github.io/). The table is a starting point, not the full list: when the stack is
missing here, a tool looks unmaintained (no release for a year, archived repository) or a newer
one has replaced it (as knip replaced ts-prune), search the web for the current established tool
before you choose, and say which one you picked and why.

Keep code private: never upload a scan to a tool's cloud service (skylos needs `--no-upload`),
and never send code to a model API the tool calls unless the project already allows it.

## 3. Verify every candidate

Static tools miss dynamic use. Before deleting, search the whole repository (code, templates,
configuration, scripts, docs, tests) for the name and rule out:

- dynamic use: reflection, `getattr`, string dispatch, dependency injection, serializers, ORM
  models, plugin registries and entry points (`pyproject.toml` scripts, `package.json` `bin` and
  `exports`, `main`);
- framework hooks called by name: routes, signal handlers, test fixtures, CLI commands, migrations;
- public API of a library: exported items other projects use (check the changelog and semver
  before removing them);
- feature flags, platform-specific code (`cfg`, build tags), generated code and vendored code
  (leave those to their generator or upstream).

A false positive goes into the tool's allowlist (vulture whitelist file, `[package.metadata.cargo-shear]
ignored`, knip `ignoreDependencies` or a JSDoc `@public` tag on an export, a lint `allow` on the item) with a comment saying why it is used.

## 4. Remove in small steps

- One kind of removal per commit (unused dependencies; unused imports; one unused module or
  feature), each leaving the project building with its tests green, so a revert is cheap.
- Delete code, do not comment it out; Git keeps the history.
- Remove tests only for code that is gone, never to make the gate pass. When a removal changes
  behavior (an endpoint, a CLI flag, a public function), it is no longer a chore: write a spec.
- Rerun the tool after each step: removing one item often reveals the next.

## 5. Keep it from coming back

When the check is cheap and has few false positives (unused imports and dependencies nearly
always are), add it to the project's single gate (`infra-bootstrap`), pinned to a version, with
its allowlist committed. Noisy checks (whole-program dead functions) stay on demand: run them
during cleanups and before releases.

## Report

List what was removed, what was kept as a false positive and why, the tools and versions used,
and the gate result.
