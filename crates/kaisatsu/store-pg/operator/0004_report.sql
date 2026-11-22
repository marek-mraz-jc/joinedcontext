-- Read-only, before the rollout of migration 0004. As kaisatsu_migrate:
--   psql -v ON_ERROR_STOP=1 -f 0004_report.sql
-- Lists on the operator's terminal, by id, what needs a decision:
--   membership  only in the person's document: add the membership, or let 0004 end it
--   holder      a reserved client id held by an Organization or a Portal mark: it carries no trust
--   rootless    a nested path with no parent group: adopt or delete it
--   absent      grants of `{realm}-realm` where no such realm exists
DO $report$
DECLARE
    all_realms text[];
    names text[];
    r text;
    p record;
BEGIN
    PERFORM set_config('kaisatsu.realm', '*', true), set_config('kaisatsu.orgs', '*', true);
    SELECT coalesce(array_agg(realm_id::text), '{}'), coalesce(array_agg(name::text), '{}')
    INTO all_realms, names FROM kaisatsu.realms;

    FOREACH r IN ARRAY all_realms LOOP
        PERFORM set_config('kaisatsu.realm', r, true);
        FOR p IN
            SELECT 'membership' AS what, u.id::text AS holder, g.id::text AS object
            FROM kaisatsu.people u,
                 jsonb_array_elements_text(CASE WHEN jsonb_typeof(u.doc->'groups') = 'array' THEN u.doc->'groups' ELSE '[]' END) n(name),
                 kaisatsu.groups g
            WHERE u.realm_id = r AND g.realm_id = r AND (g.path = n.name OR g.doc->>'name' = n.name)
              AND NOT EXISTS (SELECT 1 FROM kaisatsu.group_members m
                              WHERE m.realm_id = r AND m.group_id = g.id AND m.user_id = u.id)
            UNION ALL
            SELECT 'holder', a.id::text, a.client_id::text FROM kaisatsu.applications a
            WHERE a.realm_id = r
              AND (lower(a.client_id) IN ('realm-management', 'broker', 'account', 'account-console',
                                          'admin-cli', 'admin-permissions', 'security-admin-console')
                   OR lower(a.client_id) LIKE '%-realm')
              AND (a.org_id IS NOT NULL OR a.doc #>> '{attributes,managed-by}' = 'joinedcontext')
            UNION ALL
            SELECT 'rootless', g.id::text, g.path::text FROM kaisatsu.groups g
            WHERE g.realm_id = r AND g.doc->>'parentId' IS NULL AND g.path ~ '^/[^/]+/.'
              AND NOT EXISTS (SELECT 1 FROM kaisatsu.groups x
                              WHERE x.realm_id = r AND x.path = regexp_replace(g.path, '/[^/]+$', ''))
            UNION ALL
            SELECT 'absent', u.id::text, k.key
            FROM kaisatsu.people u,
                 jsonb_object_keys(CASE WHEN jsonb_typeof(u.doc->'clientRoles') = 'object' THEN u.doc->'clientRoles' ELSE '{}' END) k(key)
            WHERE u.realm_id = r AND k.key LIKE '%-realm'
              AND NOT regexp_replace(k.key, '-realm$', '') = ANY (names)
        LOOP
            RAISE NOTICE 'realm % % % %', r, p.what, p.holder, p.object;
        END LOOP;
    END LOOP;
    PERFORM set_config('kaisatsu.realm', '', true), set_config('kaisatsu.orgs', '', true);
END
$report$;
