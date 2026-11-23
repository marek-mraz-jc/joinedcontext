-- T-7564: shako.store_log stops growing. The typed tables hold every entry up to
-- shako.projection.seq, so such an entry is only kept for a rollback to a release that rebuilds
-- from the log alone. Once a tenant has been projected for 30 days, its entries older than 30
-- days go (`PgStore::trim_log`). create_repo entries stay: projection's first catch-up reads
-- them to tell a repository row an earlier release wrote. A ledger of its own
-- (shako_trim_migrations), so the previous release still starts.
ALTER TABLE shako.projection ADD COLUMN IF NOT EXISTS since TIMESTAMPTZ NOT NULL DEFAULT now();
-- The request role writes the projected sequence only, never `since`, the trim's own gate.
REVOKE INSERT, UPDATE ON shako.projection FROM shako_app;
GRANT INSERT (instance_id, org_id, seq), UPDATE (seq) ON shako.projection TO shako_app;

-- The request role may delete exactly what the tables hold and the rollback window passed; the
-- permissive tenant_isolation policy still names the tenant.
GRANT DELETE ON shako.store_log TO shako_app;
DROP POLICY IF EXISTS trim_projected ON shako.store_log;
CREATE POLICY trim_projected ON shako.store_log AS RESTRICTIVE FOR DELETE TO shako_app
    USING (
        created_at < now() - interval '30 days'
        AND op ->> 'op' <> 'create_repo'
        AND EXISTS (
            SELECT 1 FROM shako.projection p
            WHERE p.instance_id = store_log.instance_id
              AND p.org_id = store_log.org_id
              AND store_log.seq <= p.seq
              AND p.since < now() - interval '30 days'
        )
    );
