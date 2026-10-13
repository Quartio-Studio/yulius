-- Yulius schema version 20 (SPEC-083): numbers given out for numbered files (decision records,
-- specs, migrations), so parallel tasks never take the same one. A reservation of a task that is
-- archived or deleted no longer counts; a number in use is seen in the files themselves.
CREATE TABLE record_numbers (
    sandbox_id TEXT NOT NULL REFERENCES sandboxes(id),
    dir TEXT NOT NULL,              -- repository-relative directory, e.g. docs/decisions
    prefix TEXT NOT NULL,           -- the text before the number: '' (0066-x.md), 'SPEC-'
    number INTEGER NOT NULL,
    task_id TEXT NOT NULL REFERENCES tasks(id),
    reserved_at INTEGER NOT NULL,
    PRIMARY KEY (sandbox_id, dir, prefix, number)
);
CREATE INDEX record_numbers_task ON record_numbers (task_id);
