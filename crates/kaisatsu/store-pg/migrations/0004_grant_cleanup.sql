-- S3-15, S3-16: grants left by deletes that did not cascade, and memberships kept as paths in
-- person documents. Every removed row and document piece is kept in kaisatsu.grant_cleanup_0004;
-- operator/0004_revert.sql restores them, operator/0004_report.sql lists before the rollout what
-- needs a decision. The log gets counts only. FORCE RLS binds the owner, so each realm is cleaned
-- inside its own scope.

CREATE TABLE kaisatsu.grant_cleanup_0004 (
    realm_id text NOT NULL COLLATE "C",
    id bigint GENERATED ALWAYS AS IDENTITY,
    kind text NOT NULL,
    tbl text NOT NULL,
    row_id text COLLATE "C",
    path text[],
    value jsonb NOT NULL,
    PRIMARY KEY (realm_id, id)
);
ALTER TABLE kaisatsu.grant_cleanup_0004 ENABLE ROW LEVEL SECURITY;
ALTER TABLE kaisatsu.grant_cleanup_0004 FORCE ROW LEVEL SECURITY;
CREATE POLICY grant_cleanup_0004_rls ON kaisatsu.grant_cleanup_0004
    USING (realm_id = kaisatsu.current_realm())
    WITH CHECK (realm_id = kaisatsu.current_realm());
-- The request role never reads what was taken away.
REVOKE ALL ON kaisatsu.grant_cleanup_0004 FROM kaisatsu_app;

DO $cleanup$
DECLARE
    all_realms text[];
    r text;
    p record;
    removed bigint;
    doc_only bigint;
    total bigint := 0;
    total_doc_only bigint := 0;
    -- Names that act without a row: built in, read by the Authorizer, or (mfa-required) asking
    -- more of their holder.
    builtin_roles text[] := ARRAY[
        'offline_access', 'uma_authorization', 'admin', 'realm-admin', 'create-realm',
        'realm-apply', 'mfa-required', 'scim', 'manage-smtp', 'impersonation', 'create-client',
        'manage-authorization', 'manage-clients', 'manage-events', 'manage-identity-providers',
        'manage-realm', 'manage-users', 'query-clients', 'query-groups', 'query-realms',
        'query-users', 'view-authorization', 'view-clients', 'view-events',
        'view-identity-providers', 'view-realm', 'view-users'];
    builtin_clients text[] := ARRAY[
        'realm-management', 'broker', 'account', 'account-console', 'admin-cli',
        'admin-permissions', 'security-admin-console'];
BEGIN
    PERFORM set_config('kaisatsu.realm', '*', true), set_config('kaisatsu.orgs', '*', true);
    -- Read whole first: the scope below would hide the next realm from an open cursor.
    SELECT coalesce(array_agg(realm_id::text), '{}') INTO all_realms FROM kaisatsu.realms;

    FOREACH r IN ARRAY all_realms LOOP
        PERFORM set_config('kaisatsu.realm', r, true);

        -- A child whose parent is gone: under the group now at the parent's path, else deleted
        -- with what is below it.
        FOR p IN
            SELECT g.id, g.doc->'parentId' AS old, parent.id AS parent
            FROM kaisatsu.groups g
            JOIN kaisatsu.groups parent ON parent.realm_id = g.realm_id
                AND parent.path = regexp_replace(g.path, '/[^/]+$', '') AND parent.id <> g.id
            WHERE g.realm_id = r AND g.doc->>'parentId' IS NOT NULL
              AND NOT EXISTS (SELECT 1 FROM kaisatsu.groups x
                              WHERE x.realm_id = r AND x.id = g.doc->>'parentId')
        LOOP
            INSERT INTO kaisatsu.grant_cleanup_0004 (realm_id, kind, tbl, row_id, path, value)
            VALUES (r, 'parent', 'groups', p.id, '{parentId}', p.old);
            UPDATE kaisatsu.groups SET doc = jsonb_set(doc, '{parentId}', to_jsonb(p.parent::text))
            WHERE realm_id = r AND id = p.id;
        END LOOP;
        WITH RECURSIVE tree AS (
            SELECT g.id, g.path FROM kaisatsu.groups g
            WHERE g.realm_id = r AND g.doc->>'parentId' IS NOT NULL
              AND NOT EXISTS (SELECT 1 FROM kaisatsu.groups x
                              WHERE x.realm_id = r AND x.id = g.doc->>'parentId')
            UNION
            SELECT g.id, g.path FROM kaisatsu.groups g
            JOIN tree t ON g.doc->>'parentId' = t.id
                OR (g.doc->>'parentId' IS NULL AND starts_with(g.path, t.path || '/'))
            WHERE g.realm_id = r
        ), gone AS (
            DELETE FROM kaisatsu.groups g
            WHERE g.realm_id = r AND g.id IN (SELECT id FROM tree)
            RETURNING g.*
        )
        INSERT INTO kaisatsu.grant_cleanup_0004 (realm_id, kind, tbl, row_id, value)
        SELECT r, 'row', 'groups', id, to_jsonb(gone) - 'seq' FROM gone;

        -- Memberships and mappings of who or what is gone.
        WITH gone AS (
            DELETE FROM kaisatsu.group_members m
            WHERE m.realm_id = r AND (
                NOT EXISTS (SELECT 1 FROM kaisatsu.groups g
                            WHERE g.realm_id = r AND g.id = m.group_id)
                OR NOT EXISTS (SELECT 1 FROM kaisatsu.people u
                               WHERE u.realm_id = r AND u.id = m.user_id))
            RETURNING m.*
        )
        INSERT INTO kaisatsu.grant_cleanup_0004 (realm_id, kind, tbl, row_id, value)
        SELECT r, 'row', 'group_members', user_id, to_jsonb(gone) - 'seq' FROM gone;

        WITH gone AS (
            DELETE FROM kaisatsu.role_mappings m
            WHERE m.realm_id = r AND (
                (m.principal_kind = 'user' AND NOT EXISTS (
                    SELECT 1 FROM kaisatsu.people u
                    WHERE u.realm_id = r AND u.id = m.principal_id))
                OR (m.principal_kind = 'group' AND NOT EXISTS (
                    SELECT 1 FROM kaisatsu.groups g
                    WHERE g.realm_id = r AND g.id = m.principal_id))
                OR (m.client_uuid IS NOT NULL AND NOT EXISTS (
                    SELECT 1 FROM kaisatsu.applications a
                    WHERE a.realm_id = r AND a.id = m.client_uuid)))
            RETURNING m.*
        )
        INSERT INTO kaisatsu.grant_cleanup_0004 (realm_id, kind, tbl, row_id, value)
        SELECT r, 'row', 'role_mappings', id, to_jsonb(gone) - 'seq' FROM gone;

        WITH gone AS (
            DELETE FROM kaisatsu.roles x
            WHERE x.realm_id = r AND x.client_role AND x.container_id IS NOT NULL
              AND NOT EXISTS (SELECT 1 FROM kaisatsu.applications a
                              WHERE a.realm_id = r AND a.id = x.container_id)
            RETURNING x.*
        )
        INSERT INTO kaisatsu.grant_cleanup_0004 (realm_id, kind, tbl, row_id, value)
        SELECT r, 'row', 'roles', id, to_jsonb(gone) - 'seq' FROM gone;

        -- Client grants that name no client.
        FOR p IN
            SELECT d.tbl, d.id, d.base || k.key AS path, k.value
            FROM (
                SELECT 'people' AS tbl, id::text AS id, ARRAY['clientRoles'] AS base,
                       doc->'clientRoles' AS grants
                FROM kaisatsu.people WHERE realm_id = r
                UNION ALL
                SELECT 'groups', id::text, ARRAY['clientRoles'], doc->'clientRoles'
                FROM kaisatsu.groups WHERE realm_id = r
                UNION ALL
                SELECT 'roles', id::text, ARRAY['composites', 'client'], doc #> '{composites,client}'
                FROM kaisatsu.roles WHERE realm_id = r
            ) d, jsonb_each(CASE WHEN jsonb_typeof(d.grants) = 'object' THEN d.grants ELSE '{}' END) k
            WHERE NOT k.key = ANY (builtin_clients) AND k.key NOT LIKE '%-realm'
              AND NOT EXISTS (SELECT 1 FROM kaisatsu.applications a
                              WHERE a.realm_id = r AND a.client_id = k.key)
        LOOP
            INSERT INTO kaisatsu.grant_cleanup_0004 (realm_id, kind, tbl, row_id, path, value)
            VALUES (r, 'key', p.tbl, p.id, p.path, p.value);
            EXECUTE format('UPDATE kaisatsu.%I SET doc = doc #- $1 WHERE realm_id = $2 AND id = $3', p.tbl)
            USING p.path, r, p.id;
        END LOOP;

        -- Names with neither a role row nor a mapping row, what a delete without cascade left:
        -- realm roles (in a realm that has role rows) and roles of the Portal's App clients.
        FOR p IN
            SELECT d.tbl, d.id, ARRAY['realmRoles'] AS path, n.name
            FROM (
                SELECT 'people' AS tbl, 'user' AS kind, id::text AS id, doc->'realmRoles' AS names
                FROM kaisatsu.people WHERE realm_id = r
                UNION ALL
                SELECT 'groups', 'group', id::text, doc->'realmRoles'
                FROM kaisatsu.groups WHERE realm_id = r
            ) d, jsonb_array_elements_text(CASE WHEN jsonb_typeof(d.names) = 'array' THEN d.names ELSE '[]' END) n(name)
            WHERE EXISTS (SELECT 1 FROM kaisatsu.roles x WHERE x.realm_id = r AND NOT x.client_role)
              AND NOT n.name = ANY (builtin_roles) AND n.name NOT LIKE 'default-roles-%'
              AND NOT EXISTS (SELECT 1 FROM kaisatsu.roles x
                              WHERE x.realm_id = r AND NOT x.client_role AND x.name = n.name)
              AND NOT EXISTS (SELECT 1 FROM kaisatsu.role_mappings m
                              WHERE m.realm_id = r AND m.principal_kind = d.kind
                                AND m.principal_id = d.id AND m.client_uuid IS NULL
                                AND m.role_name = n.name)
            UNION ALL
            SELECT d.tbl, d.id, ARRAY['clientRoles', a.client_id::text], n.name
            FROM kaisatsu.applications a
            JOIN (
                SELECT 'people' AS tbl, 'user' AS kind, id::text AS id, doc
                FROM kaisatsu.people WHERE realm_id = r
                UNION ALL
                SELECT 'groups', 'group', id::text, doc FROM kaisatsu.groups WHERE realm_id = r
            ) d ON jsonb_typeof(d.doc #> ARRAY['clientRoles', a.client_id::text]) = 'array',
            jsonb_array_elements_text(d.doc #> ARRAY['clientRoles', a.client_id::text]) n(name)
            WHERE a.realm_id = r AND a.client_id LIKE 'app-%'
              AND a.doc #>> '{attributes,managed-by}' = 'joinedcontext'
              AND EXISTS (SELECT 1 FROM kaisatsu.roles x
                          WHERE x.realm_id = r AND x.client_role AND x.container_id = a.id)
              AND NOT EXISTS (SELECT 1 FROM kaisatsu.roles x
                              WHERE x.realm_id = r AND x.client_role AND x.container_id = a.id
                                AND x.name = n.name)
              AND NOT EXISTS (SELECT 1 FROM kaisatsu.role_mappings m
                              WHERE m.realm_id = r AND m.principal_kind = d.kind
                                AND m.principal_id = d.id AND m.client_uuid = a.id
                                AND m.role_name = n.name)
        LOOP
            INSERT INTO kaisatsu.grant_cleanup_0004 (realm_id, kind, tbl, row_id, path, value)
            VALUES (r, 'item', p.tbl, p.id, p.path, to_jsonb(p.name));
            EXECUTE format('UPDATE kaisatsu.%I SET doc = jsonb_set(doc, $1, (doc #> $1) - $2) WHERE realm_id = $3 AND id = $4', p.tbl)
            USING p.path, p.name, r, p.id;
        END LOOP;

        -- The document's group paths. A path never becomes a row here: after a rename or a
        -- removal nothing tells it from a membership.
        SELECT count(*) INTO doc_only
        FROM kaisatsu.people u,
             jsonb_array_elements_text(CASE WHEN jsonb_typeof(u.doc->'groups') = 'array' THEN u.doc->'groups' ELSE '[]' END) n(name),
             kaisatsu.groups g
        WHERE u.realm_id = r AND g.realm_id = r AND (g.path = n.name OR g.doc->>'name' = n.name)
          AND NOT EXISTS (SELECT 1 FROM kaisatsu.group_members m
                          WHERE m.realm_id = r AND m.group_id = g.id AND m.user_id = u.id);
        INSERT INTO kaisatsu.grant_cleanup_0004 (realm_id, kind, tbl, row_id, path, value)
        SELECT r, 'member', 'people', u.id, '{groups}',
               jsonb_build_object('name', n.name, 'groupId', (
                   SELECT g.id FROM kaisatsu.groups g
                   WHERE g.realm_id = r AND (g.path = n.name OR g.doc->>'name' = n.name)
                   ORDER BY (g.path = n.name) DESC, g.seq LIMIT 1))
        FROM kaisatsu.people u,
             jsonb_array_elements_text(CASE WHEN jsonb_typeof(u.doc->'groups') = 'array' THEN u.doc->'groups' ELSE '[]' END) n(name)
        WHERE u.realm_id = r;
        UPDATE kaisatsu.people SET doc = jsonb_set(doc, '{groups}', '[]')
        WHERE realm_id = r AND jsonb_typeof(doc->'groups') = 'array' AND doc->'groups' <> '[]';

        SELECT count(*) INTO removed FROM kaisatsu.grant_cleanup_0004 WHERE realm_id = r;
        total := total + removed;
        total_doc_only := total_doc_only + doc_only;
    END LOOP;

    -- The scope ends with the migration's transaction.
    PERFORM set_config('kaisatsu.realm', '', true), set_config('kaisatsu.orgs', '', true);
    RAISE NOTICE 'kaisatsu 0004: % orphan grant entries removed and kept in grant_cleanup_0004', total;
    IF total_doc_only > 0 THEN
        RAISE WARNING 'kaisatsu 0004: % group memberships existed only in person documents and were not kept', total_doc_only;
    END IF;
END
$cleanup$;
