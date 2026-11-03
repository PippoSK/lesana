-- Derived navigation metadata; official text and embedding inputs stay unchanged.
ALTER TABLE legal_nodes ADD COLUMN reference_links jsonb NOT NULL DEFAULT '[]';
