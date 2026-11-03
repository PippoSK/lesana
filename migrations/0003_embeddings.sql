CREATE EXTENSION IF NOT EXISTS vector;

-- Keep the original spelling and ancestry for the embedding model. This view
-- also detects text/context changes without changing the official source nodes.
CREATE VIEW legal_embedding_inputs AS
WITH RECURSIVE ancestry AS (
    SELECT id, version_id, parent_anchor,
        CASE WHEN kind='group' THEN label ELSE heading END AS titles
    FROM legal_nodes
    UNION ALL
    SELECT child.id, child.version_id, parent.parent_anchor,
        child.titles || ' ' || CASE WHEN parent.kind='group' THEN parent.label ELSE parent.heading END
    FROM ancestry child
    JOIN legal_nodes parent ON parent.version_id=child.version_id AND parent.anchor=child.parent_anchor
), documents AS (
    SELECT n.id AS node_id,
        a.title || ' (' || a.number || ')' || E'\n' || n.citation || E'\n' || ancestry.titles
        || E'\n' || n.own_text
        || CASE WHEN n.context_text <> n.own_text THEN E'\nKontext:\n' || n.context_text ELSE '' END AS content
    FROM legal_nodes n
    JOIN ancestry ON ancestry.id=n.id AND ancestry.parent_anchor IS NULL
    JOIN act_versions v ON v.id=n.version_id
    JOIN acts a ON a.id=v.act_id
    WHERE n.searchable
)
SELECT node_id, content, md5(content) AS content_hash FROM documents;

CREATE TABLE embedding_models (
    model_key text PRIMARY KEY,
    model_name text NOT NULL,
    dimensions integer NOT NULL CHECK(dimensions BETWEEN 1 AND 16000),
    UNIQUE(model_key, dimensions)
);

CREATE TABLE legal_embedding_documents (
    model_key text NOT NULL REFERENCES embedding_models(model_key),
    node_id bigint NOT NULL REFERENCES legal_nodes(id) ON DELETE CASCADE,
    content_hash text NOT NULL,
    indexed_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY(model_key, node_id)
);

CREATE TABLE legal_node_embeddings (
    model_key text NOT NULL,
    node_id bigint NOT NULL,
    chunk_index integer NOT NULL CHECK(chunk_index >= 0),
    dimensions integer NOT NULL,
    embedding vector NOT NULL CHECK(vector_dims(embedding)=dimensions AND vector_norm(embedding)>0),
    PRIMARY KEY(model_key, node_id, chunk_index),
    FOREIGN KEY(model_key, node_id) REFERENCES legal_embedding_documents(model_key, node_id) ON DELETE CASCADE,
    FOREIGN KEY(model_key, dimensions) REFERENCES embedding_models(model_key, dimensions)
);

-- Exact cosine search is deliberate for this small corpus: date/act filters
-- are applied before ranking, and different model dimensions remain isolated.
-- Approximate vector indexes can be added per model when the corpus grows.
