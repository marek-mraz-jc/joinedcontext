-- 4.22 for Attribute instances (T-4837): the earliest `expiresAt` of any
-- instance in `entity`, written by every entity write beside `expires_at`,
-- so the sweep finds the rows whose current state holds an expired instance
-- by this partial index instead of reading every doc. NULL on rows whose
-- instances carry no expiry, which is nearly all of them.
ALTER TABLE entities ADD COLUMN attrs_expire_at timestamptz;
CREATE INDEX i_entities_attrs_expire ON entities (attrs_expire_at)
  WHERE attrs_expire_at IS NOT NULL;

-- Backfill the rows written before this column. UPDATE holds to the
-- caller's Tenant on both sides (0005), so it runs once per Tenant; the
-- Tenant list is read under the service escape, which SELECT grants.
DO $$
DECLARE t text;
BEGIN
  PERFORM set_config('antares.service', 'on', true);
  FOR t IN SELECT DISTINCT tenant_id FROM entities LOOP
    PERFORM set_config('antares.tenant', t, true);
    UPDATE entities e SET attrs_expire_at = m.at
      FROM (SELECT x.id, min(try_timestamptz(replace(i->>'expiresAt', ',', '.'))) AS at
              FROM entities x, jsonb_each(x.entity) a(k, v),
                   jsonb_array_elements(CASE WHEN jsonb_typeof(a.v) = 'array'
                                             THEN a.v ELSE '[]'::jsonb END) i
             WHERE x.tenant_id = t AND x.entity::text LIKE '%expiresAt%'
               AND a.k NOT IN ('id', 'type', 'scope', 'createdAt', 'modifiedAt',
                               'deletedAt', 'expiresAt', '@context')
               AND jsonb_typeof(i) = 'object' AND i ? 'expiresAt'
             GROUP BY x.id) m
     WHERE e.tenant_id = t AND e.id = m.id AND m.at IS NOT NULL;
  END LOOP;
  PERFORM set_config('antares.tenant', '', true);
  PERFORM set_config('antares.service', '', true);
END $$;
