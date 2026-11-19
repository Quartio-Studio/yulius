-- Yulius migration 0067, schema version 67 (SPEC-510 TKC-003): whether a person wants teammates
-- to ask before taking control of their tasks. Off for everyone: a teammate takes control at once
-- (ADR-0365) until its owner chooses to be asked.
ALTER TABLE users ADD COLUMN control_ask_first INTEGER NOT NULL DEFAULT 0;
