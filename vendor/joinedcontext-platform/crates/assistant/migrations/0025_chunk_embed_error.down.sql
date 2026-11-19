DROP INDEX chunks_embedding_pending;
CREATE INDEX chunks_embedding_pending ON chunks (project, id) WHERE embedding IS NULL;
ALTER TABLE chunks DROP COLUMN embed_error;
