-- Yulius migration 0095 (SPEC-651, ADR-0478): when a subscription charges its card. A person
-- gives one charge date (`anchor`, YYYY-MM-DD, the last charge or the next one) and how often it
-- repeats (`period`); Yulius counts the next charge from them. No provider tells Yulius the
-- billing date. At most one cycle per subscription, removed with it (schema version 95).
CREATE TABLE account_billing (
    account_id TEXT PRIMARY KEY REFERENCES accounts(id) ON DELETE CASCADE,
    anchor TEXT NOT NULL,
    period TEXT NOT NULL CHECK (period IN ('month', 'year')),
    updated_at INTEGER NOT NULL
);
