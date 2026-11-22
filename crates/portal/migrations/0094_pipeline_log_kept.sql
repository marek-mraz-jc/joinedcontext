-- The trim of a pipeline's log (T-7494). Its cut walks the newest 5000 ids of the pipeline, and
-- forced row-level security (0080) reads org_id and project_id of each, which pipeline_log_by_pipeline
-- does not hold: 5000 heap rows per trim, on a table whose trims timed out on jc-prod. With the key
-- in the index the cut reads about 60 index pages on 1.4 million rows and no heap page that vacuum
-- marked visible, so the table is vacuumed at 2 % changed or inserted rather than 20 %.
--
-- The build holds writes to pipeline_log while it runs (5 s on 1.4 million warm rows). An operator
-- may build it before the release, outside a transaction:
--   CREATE INDEX CONCURRENTLY pipeline_log_kept ON pipeline_log (project, pipeline, id DESC)
--     INCLUDE (org_id, project_id);
-- It is not built that way here: two Portals migrating at once would deadlock on the migrator's lock.
DO $$
BEGIN
    -- A concurrent build that failed leaves an invalid index of the name, which IF NOT EXISTS keeps.
    IF EXISTS (SELECT 1 FROM pg_index WHERE indexrelid = to_regclass('pipeline_log_kept') AND NOT indisvalid) THEN
        DROP INDEX pipeline_log_kept;
    END IF;
END $$;
CREATE INDEX IF NOT EXISTS pipeline_log_kept
    ON pipeline_log (project, pipeline, id DESC) INCLUDE (org_id, project_id);
DROP INDEX IF EXISTS pipeline_log_by_pipeline;
ALTER TABLE pipeline_log SET (
    autovacuum_vacuum_scale_factor = 0.02,
    autovacuum_vacuum_insert_scale_factor = 0.02
);
