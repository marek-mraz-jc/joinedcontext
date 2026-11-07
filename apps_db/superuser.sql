-- Run once by the superuser when apps-db is created (deployment components/apps-db,
-- CloudNativePG `postInitApplicationSQL`, which carries these statements; T-3362, AP-144).
--
-- A shard's login may switch into the role of every App of its shard, as Postgres checks a
-- switch against the session's login. So no App role may run a function that switches again or
-- runs SQL text it was handed: `set_config` is executable by `jc_set_config` alone, whose members
-- are the shards' logins and the Portal's, and the functions that run a query given as text are
-- executable by nobody but the superuser. Once the host has set an App's role, Postgres itself
-- refuses that App becoming another.
CREATE ROLE jc_set_config NOLOGIN;
REVOKE EXECUTE ON FUNCTION pg_catalog.set_config(text, text, boolean) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION pg_catalog.set_config(text, text, boolean) TO jc_set_config;
REVOKE EXECUTE ON FUNCTION pg_catalog.query_to_xml(text, boolean, boolean, text) FROM PUBLIC;
REVOKE EXECUTE ON FUNCTION pg_catalog.query_to_xmlschema(text, boolean, boolean, text) FROM PUBLIC;
REVOKE EXECUTE ON FUNCTION pg_catalog.query_to_xml_and_xmlschema(text, boolean, boolean, text) FROM PUBLIC;
REVOKE EXECUTE ON FUNCTION pg_catalog.cursor_to_xml(refcursor, integer, boolean, boolean, text) FROM PUBLIC;
REVOKE EXECUTE ON FUNCTION pg_catalog.cursor_to_xmlschema(refcursor, boolean, boolean, text) FROM PUBLIC;
REVOKE EXECUTE ON FUNCTION pg_catalog.ts_stat(text) FROM PUBLIC;
REVOKE EXECUTE ON FUNCTION pg_catalog.ts_stat(text, text) FROM PUBLIC;
REVOKE EXECUTE ON FUNCTION pg_catalog.ts_rewrite(tsquery, text) FROM PUBLIC;
