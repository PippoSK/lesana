ALTER TABLE source_import_jobs DROP CONSTRAINT source_import_jobs_kind_check;
ALTER TABLE source_import_jobs ADD CONSTRAINT source_import_jobs_kind_check
    CHECK (kind IN ('legislation', 'judgment', 'eu_regulation', 'supreme_court_judgment', 'supreme_court_collection', 'czech_supreme_court_judgment'));
