CREATE TABLE conversation_attachments (
    id uuid PRIMARY KEY,
    conversation_id uuid NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
    filename text NOT NULL CHECK (octet_length(filename) BETWEEN 1 AND 240),
    content_type text NOT NULL,
    size_bytes bigint NOT NULL CHECK (size_bytes BETWEEN 1 AND 10485760),
    text_chars bigint NOT NULL CHECK (text_chars BETWEEN 1 AND 100000),
    original_bytes bytea NOT NULL,
    extracted_text text NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now(),
    CHECK (octet_length(original_bytes) = size_bytes),
    CHECK (char_length(extracted_text) = text_chars)
);
CREATE INDEX conversation_attachments_conversation ON conversation_attachments(conversation_id, created_at, id);
