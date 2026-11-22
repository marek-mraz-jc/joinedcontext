-- Undoes migration 0004 by an operator's decision; nothing runs it by itself. As kaisatsu_migrate:
--   psql -v ON_ERROR_STOP=1 -c "SET kaisatsu.restore = 'all'" -f 0004_revert.sql
-- or with ids of grant_cleanup_0004 ('12,14') in place of 'all'. A piece whose role, client or
-- group was created again since, or whose holder is gone, stays removed: restoring it would grant
-- what nobody granted. 'all' also drops the table and the migration's row, for a rollback.
DO $revert$
DECLARE
    choice text := nullif(current_setting('kaisatsu.restore', true), '');
    all_realms text[];
    r text;
    p record;
    ok boolean;
    n bigint;
    restored bigint := 0;
    skipped bigint := 0;
BEGIN
    IF choice IS NULL OR choice !~ '^(all|[0-9]+(,[0-9]+)*)$' THEN
        RAISE EXCEPTION 'set kaisatsu.restore to all or to the ids of grant_cleanup_0004';
    END IF;
    PERFORM set_config('kaisatsu.realm', '*', true), set_config('kaisatsu.orgs', '*', true);
    SELECT coalesce(array_agg(realm_id::text), '{}') INTO all_realms FROM kaisatsu.realms;

    FOREACH r IN ARRAY all_realms LOOP
        PERFORM set_config('kaisatsu.realm', r, true);
        FOR p IN
            SELECT * FROM kaisatsu.grant_cleanup_0004
            WHERE realm_id = r AND (choice = 'all' OR id::text = ANY (string_to_array(choice, ',')))
            ORDER BY id
        LOOP
            ok := false;
            IF p.kind = 'row' AND p.tbl = 'groups' THEN
                INSERT INTO kaisatsu.groups (realm_id, id, org_id, path, doc)
                SELECT r, p.row_id, p.value->>'org_id', p.value->>'path', p.value->'doc'
                ON CONFLICT DO NOTHING;
                GET DIAGNOSTICS n = ROW_COUNT;
                ok := n > 0;
            ELSIF p.kind = 'row' AND p.tbl = 'group_members' THEN
                INSERT INTO kaisatsu.group_members (realm_id, group_id, user_id, org_id, doc)
                SELECT r, g.id, u.id, p.value->>'org_id', p.value->'doc'
                FROM kaisatsu.groups g, kaisatsu.people u
                WHERE g.realm_id = r AND g.id = p.value->>'group_id'
                  AND u.realm_id = r AND u.id = p.value->>'user_id'
                ON CONFLICT DO NOTHING;
                GET DIAGNOSTICS n = ROW_COUNT;
                ok := n > 0;
            ELSIF p.kind = 'row' AND p.tbl = 'role_mappings' THEN
                INSERT INTO kaisatsu.role_mappings
                    (realm_id, id, principal_kind, principal_id, client_uuid, role_name, doc)
                SELECT r, p.row_id, p.value->>'principal_kind', p.value->>'principal_id',
                       p.value->>'client_uuid', p.value->>'role_name', p.value->'doc'
                WHERE (EXISTS (SELECT 1 FROM kaisatsu.people u
                               WHERE u.realm_id = r AND u.id = p.value->>'principal_id'
                                 AND p.value->>'principal_kind' = 'user')
                    OR EXISTS (SELECT 1 FROM kaisatsu.groups g
                               WHERE g.realm_id = r AND g.id = p.value->>'principal_id'
                                 AND p.value->>'principal_kind' = 'group'))
                  AND (p.value->>'client_uuid' IS NULL OR EXISTS (
                      SELECT 1 FROM kaisatsu.applications a
                      WHERE a.realm_id = r AND a.id = p.value->>'client_uuid'))
                ON CONFLICT DO NOTHING;
                GET DIAGNOSTICS n = ROW_COUNT;
                ok := n > 0;
            ELSIF p.kind = 'row' AND p.tbl = 'roles' THEN
                INSERT INTO kaisatsu.roles (realm_id, id, container_id, client_role, name, doc)
                SELECT r, p.row_id, p.value->>'container_id', true, p.value->>'name', p.value->'doc'
                ON CONFLICT DO NOTHING;
                GET DIAGNOSTICS n = ROW_COUNT;
                ok := n > 0;
            ELSIF p.kind = 'parent' THEN
                UPDATE kaisatsu.groups SET doc = jsonb_set(doc, p.path, p.value)
                WHERE realm_id = r AND id = p.row_id;
                GET DIAGNOSTICS n = ROW_COUNT;
                ok := n > 0;
            ELSIF p.kind = 'key' THEN
                IF NOT EXISTS (SELECT 1 FROM kaisatsu.applications a
                               WHERE a.realm_id = r AND a.client_id = p.path[array_length(p.path, 1)]) THEN
                    EXECUTE format('UPDATE kaisatsu.%I SET doc = jsonb_set(doc, $1, $2) WHERE realm_id = $3 AND id = $4', p.tbl)
                    USING p.path, p.value, r, p.row_id;
                    GET DIAGNOSTICS n = ROW_COUNT;
                    ok := n > 0;
                END IF;
            ELSIF p.kind = 'item' THEN
                IF NOT EXISTS (
                    SELECT 1 FROM kaisatsu.roles x
                    LEFT JOIN kaisatsu.applications a ON a.realm_id = r AND a.id = x.container_id
                    WHERE x.realm_id = r AND x.name = p.value #>> '{}'
                      AND ((p.path = '{realmRoles}' AND NOT x.client_role)
                           OR a.client_id = p.path[2])
                ) THEN
                    EXECUTE format('UPDATE kaisatsu.%I SET doc = jsonb_set(doc, $1, coalesce(doc #> $1, ''[]'') || $2) WHERE realm_id = $3 AND id = $4', p.tbl)
                    USING p.path, p.value, r, p.row_id;
                    GET DIAGNOSTICS n = ROW_COUNT;
                    ok := n > 0;
                END IF;
            ELSIF p.kind = 'member' THEN
                IF EXISTS (SELECT 1 FROM kaisatsu.groups g
                           WHERE g.realm_id = r AND g.id = p.value->>'groupId'
                             AND (g.path = p.value->>'name' OR g.doc->>'name' = p.value->>'name')) THEN
                    UPDATE kaisatsu.people
                    SET doc = jsonb_set(doc, '{groups}',
                                        coalesce(doc->'groups', '[]') || (p.value->'name'))
                    WHERE realm_id = r AND id = p.row_id;
                    GET DIAGNOSTICS n = ROW_COUNT;
                    ok := n > 0;
                END IF;
            END IF;
            IF ok THEN
                restored := restored + 1;
                DELETE FROM kaisatsu.grant_cleanup_0004 WHERE realm_id = r AND id = p.id;
            ELSE
                skipped := skipped + 1;
            END IF;
        END LOOP;
    END LOOP;

    IF choice = 'all' THEN
        DROP TABLE kaisatsu.grant_cleanup_0004;
        DELETE FROM kaisatsu_migrations._sqlx_migrations WHERE version = 4;
    END IF;
    PERFORM set_config('kaisatsu.realm', '', true), set_config('kaisatsu.orgs', '', true);
    RAISE NOTICE 'kaisatsu 0004 revert: % restored, % left removed', restored, skipped;
END
$revert$;
