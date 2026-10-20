# Ready-made automations

Yulius lists these automations on every project's Automations page as its Defaults, where a person
switches one on (SPEC-042, SPEC-197). Each `<id>.md` file here is one automation: a frontmatter (`name`,
`description`, `cron`, `utc_offset_minutes`, optional `workflow` such as `wf-docs`, `order`) and
then the prompt of each task it creates. Yulius embeds every `*.md` file of this directory except
this README at build time; a unit test parses each one, so a malformed file fails CI.

Nothing here runs until a person enables it. Enabling creates an ordinary automation from the
file's content (then editable, pausable and deletable); a project's automation with the same name
counts as this one, so a release that changes a prompt does not change automations already enabled.

Each task an automation creates is a goal task (SPEC-240), and a goal longer than Claude Code's cap
is cut. So a prompt's first paragraph, which every goal keeps (a unit test checks it), says what
the run does and how it ends: for instance that it opens no pull request, or does not wait for the
tasks it starts.
