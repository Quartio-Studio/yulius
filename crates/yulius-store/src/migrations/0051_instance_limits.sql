-- Yulius schema version 50 (SPEC-459, ADR-0294): the instance's resource limits as one JSON
-- document: the settings a superadmin saved (a number or host-sized, who, when, the host then)
-- and every unit value the server has seen with the host it first saw that number on. NULL until
-- the first start of a version that records them.
ALTER TABLE instance ADD COLUMN limits TEXT;
