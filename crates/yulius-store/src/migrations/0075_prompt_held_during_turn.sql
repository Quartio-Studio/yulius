-- A queued prompt and the turn running at its Enter have independent hooks (#1206).
ALTER TABLE capacity_pauses ADD COLUMN during_turn INTEGER NOT NULL DEFAULT 0;
-- A live hold written by an older server does not identify which turn entered the prompt.
-- Its transcript, the capacity loop or an abandoned hook can still settle it.
UPDATE capacity_pauses SET during_turn = 1 WHERE kind = 'prompt' AND ended_at IS NULL;
