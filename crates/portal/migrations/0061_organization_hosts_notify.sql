-- jc swaps its router's host table from the served rows of the host registry (T-7129 §1,
-- T-7136): each write to `organization_hosts` wakes it with `NOTIFY organization_hosts`, and jc
-- rebuilds every 60 seconds as well, so a lost notification delays a change and loses none.
CREATE OR REPLACE FUNCTION organization_hosts_notify() RETURNS trigger AS $$
BEGIN
    PERFORM pg_notify('organization_hosts', '');
    RETURN NULL;
END
$$ LANGUAGE plpgsql;

DROP TRIGGER IF EXISTS organization_hosts_notify ON organization_hosts;
CREATE TRIGGER organization_hosts_notify AFTER INSERT OR UPDATE ON organization_hosts
    FOR EACH STATEMENT EXECUTE FUNCTION organization_hosts_notify();
