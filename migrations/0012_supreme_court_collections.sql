ALTER TABLE source_import_jobs DROP CONSTRAINT source_import_jobs_kind_check;
ALTER TABLE source_import_jobs ADD CONSTRAINT source_import_jobs_kind_check
    CHECK (kind IN ('legislation', 'judgment', 'eu_regulation', 'supreme_court_judgment', 'supreme_court_collection'));
ALTER TABLE source_import_jobs ADD COLUMN imported_count bigint CHECK (imported_count > 0);
