-- Yulius migration 0056 (SPEC-484, ADR-0325): how many steps a task's plan had when its first
-- step was finished. A plan holds all the work its agent knows of; one that grows afterwards
-- (work found later) is shown beside this number, so a person can tell that it grew. Written
-- once by the step write that finishes the first step, never changed after. NULL while no step
-- is finished, and for a task whose steps were finished before this version until its next
-- step write (schema version 56).
ALTER TABLE tasks ADD COLUMN plan_first_steps INTEGER;
