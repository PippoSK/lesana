-- Original Official Journal publication is not entry into force or applicability.
ALTER TABLE acts ADD COLUMN regulation jsonb;
ALTER TABLE acts ADD CONSTRAINT regulation_object CHECK (
    regulation IS NULL OR (
        jsonb_typeof(regulation) = 'object' AND judgment IS NULL AND treaty IS NULL
    )
);
ALTER TABLE source_import_jobs DROP CONSTRAINT source_import_jobs_kind_check;
ALTER TABLE source_import_jobs ADD CONSTRAINT source_import_jobs_kind_check
    CHECK (kind IN ('legislation', 'judgment', 'eu_regulation'));
