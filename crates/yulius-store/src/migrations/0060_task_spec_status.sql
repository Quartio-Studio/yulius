-- Yulius migration 0060 (SPEC-491, ADR-0332): what a task's spec says of its work, the
-- `status` of its frontmatter (`draft`, `approved`, `in-progress`, `done`, `abandoned`), kept
-- whenever Yulius reads the spec. A task whose spec does not say `done` or `abandoned` has work
-- left, however complete its plan reads, so it is not Done and the tasks waiting for it do not
-- start. NULL for a task without a spec, and for one whose spec was not read since this version.
ALTER TABLE tasks ADD COLUMN spec_status TEXT;
