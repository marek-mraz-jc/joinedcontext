-- W/recount.tsv: the copies left (tenant, id, expires_at).
BEGIN READ ONLY;
SELECT tenant_id, id, coalesce(to_char(expires_at AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS"Z"'), '')
  FROM entities WHERE split_part(id, ':', 4) = 'portal.smartcity.joinedcontext.com'
 ORDER BY 1, 2;
ROLLBACK;
