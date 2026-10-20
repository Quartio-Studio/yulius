-- Yulius schema version 38 (SPEC-244, ADR-0185): the email of the Claude account behind each
-- subscription, read from Claude's profile when it signs in (NULL while unknown), and when Claude
-- answered that request definitely for the current sign-in (an email or a refusal; NULL while it
-- never did, so the background round asks once). A new sign-in clears both.
ALTER TABLE accounts ADD COLUMN email TEXT;
ALTER TABLE accounts ADD COLUMN email_checked_at INTEGER;
