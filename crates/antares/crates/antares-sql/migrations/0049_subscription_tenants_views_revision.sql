-- Start-up and the background ticks stop walking every Tenant (T-7688,
-- review S5-26, S5-27, S5-28).
--
-- `subscription_tenants` counts each Tenant's subscription, context-source
-- subscription and registration rows (the domain `subscription_tenants`
-- promises). The triggers below keep it in the writing transaction, so the
-- mirror seed and the interval sweep list the Tenants that hold one instead
-- of every row of `tenants`. A row
-- at zero stays (a superset is what `subscription_tenants` promises).
--
-- `views_revision` moves whenever a `views` row of its Tenant is written or
-- deleted. The view reload reads that one row each tick and lists the
-- documents' digests only when it moved.
--
-- Both are keyed by Tenant like the inventory (0043): row-level security is
-- ENABLEd, a transaction scoped to a Tenant sees and writes its own row (the
-- triggers run as the writer, under its scope), and a member of
-- `antares_maintenance` lists them all. Neither holds a document or a
-- credential.
--
-- The backfill reads the three tables as the owner. Their row-level
-- security is FORCEd (0001), which binds the owner too, so it is lifted for
-- the backfill and FORCEd again in this same transaction; the ALTERs hold
-- their tables' lock until the commit, so no write slips between the
-- triggers and the count.
SET LOCAL lock_timeout = '30s';

CREATE TABLE subscription_tenants (
  tenant_id text PRIMARY KEY,
  docs      bigint NOT NULL DEFAULT 0
);
ALTER TABLE subscription_tenants ENABLE ROW LEVEL SECURITY;
CREATE POLICY tenant_inventory ON subscription_tenants
  USING (tenant_id = current_setting('antares.tenant', true) OR (SELECT antares_inventory_role()))
  WITH CHECK (tenant_id = current_setting('antares.tenant', true) OR (SELECT antares_inventory_role()));

CREATE FUNCTION subscription_tenants_count() RETURNS trigger
  LANGUAGE plpgsql AS $$
BEGIN
  IF TG_OP = 'DELETE' THEN
    UPDATE subscription_tenants SET docs = docs - 1 WHERE tenant_id = OLD.tenant_id;
  ELSE
    INSERT INTO subscription_tenants (tenant_id, docs) VALUES (NEW.tenant_id, 1)
      ON CONFLICT (tenant_id) DO UPDATE SET docs = subscription_tenants.docs + 1;
  END IF;
  RETURN NULL;
END $$;

-- A Tenant rename moves `tenant_id` of every table carrying it (0037), these
-- two counts included, so the triggers leave a moved row alone.
CREATE TRIGGER subscription_tenants_count
  AFTER INSERT OR DELETE ON subscriptions
  FOR EACH ROW EXECUTE FUNCTION subscription_tenants_count();
CREATE TRIGGER subscription_tenants_count
  AFTER INSERT OR DELETE ON csource_subscriptions
  FOR EACH ROW EXECUTE FUNCTION subscription_tenants_count();
CREATE TRIGGER subscription_tenants_count
  AFTER INSERT OR DELETE ON csource_registrations
  FOR EACH ROW EXECUTE FUNCTION subscription_tenants_count();

ALTER TABLE subscriptions NO FORCE ROW LEVEL SECURITY;
ALTER TABLE csource_subscriptions NO FORCE ROW LEVEL SECURITY;
ALTER TABLE csource_registrations NO FORCE ROW LEVEL SECURITY;
INSERT INTO subscription_tenants (tenant_id, docs)
  SELECT tenant_id, count(*) FROM (
    SELECT tenant_id FROM subscriptions
    UNION ALL
    SELECT tenant_id FROM csource_subscriptions
    UNION ALL
    SELECT tenant_id FROM csource_registrations
  ) s GROUP BY tenant_id;
ALTER TABLE subscriptions FORCE ROW LEVEL SECURITY;
ALTER TABLE csource_subscriptions FORCE ROW LEVEL SECURITY;
ALTER TABLE csource_registrations FORCE ROW LEVEL SECURITY;

CREATE TABLE views_revision (
  tenant_id text PRIMARY KEY,
  revision  bigint NOT NULL
);
ALTER TABLE views_revision ENABLE ROW LEVEL SECURITY;
CREATE POLICY tenant_inventory ON views_revision
  USING (tenant_id = current_setting('antares.tenant', true) OR (SELECT antares_inventory_role()))
  WITH CHECK (tenant_id = current_setting('antares.tenant', true) OR (SELECT antares_inventory_role()));

CREATE FUNCTION views_revision_bump() RETURNS trigger
  LANGUAGE plpgsql AS $$
DECLARE
  t text := CASE WHEN TG_OP = 'DELETE' THEN OLD.tenant_id ELSE NEW.tenant_id END;
BEGIN
  INSERT INTO views_revision (tenant_id, revision) VALUES (t, 1)
    ON CONFLICT (tenant_id) DO UPDATE SET revision = views_revision.revision + 1;
  RETURN NULL;
END $$;

CREATE TRIGGER views_revision_bump
  AFTER INSERT OR DELETE ON views
  FOR EACH ROW EXECUTE FUNCTION views_revision_bump();
CREATE TRIGGER views_revision_bump_update
  AFTER UPDATE ON views
  FOR EACH ROW WHEN (OLD.doc IS DISTINCT FROM NEW.doc)
  EXECUTE FUNCTION views_revision_bump();

-- The plaintext seal walk at boot runs once, not at every boot (S5-27).
-- The walk marks `plaintext_seal_walk` with a token before it starts and
-- turns it into `plaintext_sealed` at its end only while that token is still
-- there, so a walk that dies leaves no `plaintext_sealed` and the next boot
-- walks again. A write that leaves a credential in plaintext deletes both
-- marks in its transaction, whatever release or statement wrote it. It
-- first takes a shared advisory lock the walk's start takes exclusively: a
-- writer in flight when a walk starts commits before it, so the walk sees
-- its row, and a writer after the start deletes the token. An UPDATE of a
-- row already plain (the bookkeeping of each notification sent without a
-- key) deletes nothing: the write that made it plain did. The paths are
-- those of store/pg/seal.rs `plaintext_path`.
CREATE FUNCTION plaintext_seal_forget() RETURNS trigger
  LANGUAGE plpgsql AS $$
DECLARE
  plain boolean;
BEGIN
  EXECUTE format('SELECT jsonb_path_exists(($1).%I, $2::jsonpath)', TG_ARGV[0])
    INTO plain USING NEW, TG_ARGV[1];
  IF plain AND TG_OP = 'UPDATE' THEN
    EXECUTE format('SELECT NOT jsonb_path_exists(($1).%I, $2::jsonpath)', TG_ARGV[0])
      INTO plain USING OLD, TG_ARGV[1];
  END IF;
  IF plain THEN
    PERFORM pg_advisory_xact_lock_shared(hashtext('antares.seal'), 0);
    DELETE FROM maintenance_jobs WHERE name IN ('plaintext_seal_walk', 'plaintext_sealed');
  END IF;
  RETURN NULL;
END $$;

CREATE TRIGGER plaintext_seal_forget AFTER INSERT OR UPDATE ON subscriptions
  FOR EACH ROW EXECUTE FUNCTION plaintext_seal_forget('subscription', '$.notification.endpoint.receiverInfo[*].value ? (@.type() == "string" && !(@ starts with "enc:v1:"))');
CREATE TRIGGER plaintext_seal_forget AFTER INSERT OR UPDATE ON csource_subscriptions
  FOR EACH ROW EXECUTE FUNCTION plaintext_seal_forget('subscription', '$.notification.endpoint.receiverInfo[*].value ? (@.type() == "string" && !(@ starts with "enc:v1:"))');
CREATE TRIGGER plaintext_seal_forget AFTER INSERT OR UPDATE ON csource_registrations
  FOR EACH ROW EXECUTE FUNCTION plaintext_seal_forget('registration', '$.contextSourceInfo[*].value ? (@.type() == "string" && !(@ starts with "enc:v1:"))');
CREATE TRIGGER plaintext_seal_forget AFTER INSERT OR UPDATE ON dead_letters
  FOR EACH ROW EXECUTE FUNCTION plaintext_seal_forget('doc', '$ ? (exists(@.receiverInfo[*][1] ? (@.type() == "string" && !(@ starts with "enc:v1:"))) || exists(@.headers[*][1] ? (@.type() == "string" && !(@ starts with "enc:v1:"))))');
