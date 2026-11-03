-- Historical replies used the pre-Rig engine; do not relabel them as Rig runs.
ALTER TABLE messages ADD COLUMN architecture text
    CHECK (architecture IN ('pipeline', 'agentic'));
ALTER TABLE messages ADD COLUMN tool_calls jsonb NOT NULL DEFAULT '[]'::jsonb;
