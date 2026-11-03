CREATE EXTENSION IF NOT EXISTS unaccent;
CREATE EXTENSION IF NOT EXISTS pg_trgm;

CREATE TABLE acts (
    id text PRIMARY KEY,
    title text NOT NULL,
    number text NOT NULL
);
CREATE TABLE act_versions (
    id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    act_id text NOT NULL REFERENCES acts(id),
    valid_from date NOT NULL,
    valid_to date,
    verified_on date NOT NULL,
    source_url text NOT NULL,
    pdf_url text NOT NULL,
    sha256 text NOT NULL,
    imported_at timestamptz NOT NULL DEFAULT now(),
    UNIQUE(act_id, valid_from),
    CHECK(valid_to IS NULL OR valid_to >= valid_from),
    CHECK(verified_on >= valid_from)
);
CREATE TABLE legal_nodes (
    id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    version_id bigint NOT NULL REFERENCES act_versions(id),
    anchor text NOT NULL,
    parent_anchor text,
    kind text NOT NULL,
    label text NOT NULL,
    heading text NOT NULL,
    citation text NOT NULL,
    section text,
    position integer NOT NULL,
    own_text text NOT NULL,
    full_text text NOT NULL,
    context_text text NOT NULL,
    search_text text NOT NULL,
    searchable boolean NOT NULL,
    search_vector tsvector GENERATED ALWAYS AS (to_tsvector('simple', search_text)) STORED,
    UNIQUE(version_id, anchor),
    FOREIGN KEY(version_id, parent_anchor) REFERENCES legal_nodes(version_id, anchor)
        DEFERRABLE INITIALLY DEFERRED
);
CREATE INDEX legal_nodes_fts ON legal_nodes USING gin(search_vector);
CREATE INDEX legal_nodes_trgm ON legal_nodes USING gin(search_text gin_trgm_ops);
CREATE INDEX legal_nodes_section ON legal_nodes(version_id, section);
CREATE INDEX legal_nodes_parent ON legal_nodes(version_id, parent_anchor);

CREATE TABLE sessions (
    id uuid PRIMARY KEY,
    expires_at timestamptz NOT NULL DEFAULT now() + interval '30 days'
);
CREATE TABLE conversations (
    id uuid PRIMARY KEY,
    session_id uuid NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
    title text NOT NULL DEFAULT 'Nový rozhovor',
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX conversations_owner ON conversations(session_id, updated_at DESC);
CREATE TABLE messages (
    id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    conversation_id uuid NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
    role text NOT NULL CHECK(role IN ('user', 'assistant')),
    content text NOT NULL,
    sources jsonb NOT NULL DEFAULT '[]',
    as_of date NOT NULL,
    mode text NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX messages_conversation ON messages(conversation_id, id);
