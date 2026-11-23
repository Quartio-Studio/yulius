-- Yulius migration 0094 (SPEC-626 BRK-015): a Telegram notice of a brokered run's approval
-- request keeps that run's id, so a reply answers exactly the run its notice showed and never
-- another one asked at the same time (review round 1 of slice 3). NULL for every other notice.
ALTER TABLE telegram_sent ADD COLUMN command_run TEXT;
