# Ready-made automations

Yulius offers these automations on every project's Automations page, where a person enables one
with one click (SPEC-042). Each `<id>.md` file here is one automation: a frontmatter (`name`,
`description`, `cron`, `utc_offset_minutes`, optional `workflow` such as `wf-docs`, `order`) and
then the prompt of each task it creates. Yulius embeds every `*.md` file of this directory except
this README at build time; a unit test parses each one, so a malformed file fails CI.

Nothing here runs until a person enables it. Enabling creates an ordinary automation from the
file's content (then editable, pausable and deletable); a project's automation with the same name
counts as this one, so a release that changes a prompt does not change automations already enabled.
