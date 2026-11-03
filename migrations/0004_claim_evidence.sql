ALTER TABLE messages ADD COLUMN claims jsonb NOT NULL DEFAULT '[]'::jsonb;
ALTER TABLE messages ADD CONSTRAINT messages_claims_array CHECK (jsonb_typeof(claims) = 'array');
