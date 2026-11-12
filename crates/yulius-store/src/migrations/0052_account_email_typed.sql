-- Yulius migration 0052 (SPEC-463, ADR-0300): whether a subscription's `email` was typed by a
-- person on the Subscriptions page (1) or read from Claude's profile (0, as every email stored
-- before). A typed email stays across sign-ins and Claude's answers until its person changes or
-- removes it; one read from Claude is still cleared by a new sign-in (schema version 38).
ALTER TABLE accounts ADD COLUMN email_typed INTEGER NOT NULL DEFAULT 0;
