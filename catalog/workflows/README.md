# Default workflows

Yulius ships these workflows to every instance. Each `<id>.md` file here is one workflow:
Yulius embeds every `*.md` file of this directory except this README at build time, offers it
read-only in every sandbox with the id `wf-<id>`, and replaces its content with each release.
People customise a default by duplicating it.

The format is specified in SPEC-002 §7.10, "Workflow catalog format"
([SPEC-002](../../.ai/specs/SPEC-002-yulius-architecture.md)): a frontmatter (`name`, `description`, `when_to_use`, `spec`, `tests`, `order`), then exactly
`## Steps` (a list) and `## Instructions` (Markdown for the agent). A unit test parses every
file, so a malformed one fails CI.

Never rename a file that has shipped: tasks and automations refer to its id.
