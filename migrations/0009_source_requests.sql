-- Jobs survive process restarts; subscriptions keep each session's requests private.
CREATE TABLE source_import_jobs (
    id uuid PRIMARY KEY,
    kind text NOT NULL CHECK (kind IN ('legislation', 'judgment')),
    reference text NOT NULL,
    status text NOT NULL DEFAULT 'queued' CHECK (status IN ('queued', 'running', 'completed', 'failed')),
    message text,
    act_id text REFERENCES acts(id),
    title text,
    node_count bigint,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    lease_until timestamptz,
    attempt_token uuid,
    UNIQUE (kind, reference)
);
CREATE INDEX source_import_jobs_pending ON source_import_jobs(created_at)
    WHERE status IN ('queued', 'running');
CREATE TABLE source_requests (
    session_id uuid NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
    job_id uuid NOT NULL REFERENCES source_import_jobs(id),
    created_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (session_id, job_id)
);
