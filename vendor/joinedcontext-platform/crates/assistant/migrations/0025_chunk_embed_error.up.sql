-- T-7290: a passage the embedder refuses for itself (413, longer than its byte cap) keeps why in
-- `embed_error` and leaves the claim: before, the minute loop sent the same batch every minute
-- and the project's later passages never got a vector. The chunk is written anew when its page
-- or document changes, which clears the mark.
ALTER TABLE chunks ADD COLUMN embed_error text;
DROP INDEX chunks_embedding_pending;
CREATE INDEX chunks_embedding_pending ON chunks (project, id) WHERE embedding IS NULL AND embed_error IS NULL;
