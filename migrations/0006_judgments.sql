-- NULL preserves the existing statute model and historical source snapshots.
-- For judgments, act_versions.valid_from is the decision date, not effectiveness.
ALTER TABLE acts ADD COLUMN judgment jsonb;
ALTER TABLE acts ADD CONSTRAINT judgment_object CHECK (
    judgment IS NULL OR jsonb_typeof(judgment) = 'object'
);
