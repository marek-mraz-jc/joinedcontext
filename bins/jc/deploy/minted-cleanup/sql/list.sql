-- W/list.tsv: tenant, id, types, expires_at, the minted ids the document names.
-- psql -q -At -F "$(printf '\t')" -v ON_ERROR_STOP=1 -f sql/list.sql > W/list.tsv
BEGIN READ ONLY;
SELECT tenant_id, id, array_to_string(types, ' '),
  coalesce(to_char(expires_at AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS"Z"'), ''),
  coalesce((SELECT string_agg(DISTINCT m[1], ' ')
            FROM regexp_matches(entity::text, 'urn:ngsi-ld:[A-Za-z]+:portal\.smartcity\.joinedcontext\.com:[^"\\[:space:]]+', 'g') AS m
            WHERE m[1] <> id), '')
  FROM entities WHERE split_part(id, ':', 4) = 'portal.smartcity.joinedcontext.com'
 ORDER BY tenant_id, id;
ROLLBACK;
