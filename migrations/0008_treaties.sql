-- For archived treaty consolidations, valid_from is the publication date.
-- Availability in the corpus is not a determination of legal applicability.
ALTER TABLE acts ADD COLUMN treaty jsonb;
ALTER TABLE acts ADD CONSTRAINT treaty_object CHECK (
    treaty IS NULL OR (jsonb_typeof(treaty) = 'object' AND judgment IS NULL)
);
