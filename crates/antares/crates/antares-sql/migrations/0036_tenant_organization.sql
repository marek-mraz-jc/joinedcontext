-- The Organization check's stored facts (EP-188, T-7042).
--
-- tenants.organization_id: the Organization id half of the tenant name
-- `{organization id}__{segment}`, written when the row is created
-- (store/pg `claim_tenant`, `ensure_tenant`) and compared by the check under
-- ANTARES_TENANT_ORGANIZATION=enforce, so no later code path can move an
-- existing tenant to another Organization: a row whose stored id disagrees
-- with its name answers as an unknown tenant. NULL for a name without that
-- shape. Rows that exist already get it here, by the same rule as
-- `TenantId::organization_of`: split at the first `__`, the left half holds
-- a `_`, the segment none, neither empty.
--
-- tenant_rename: one row per tenant name the rename (T-7039) moved away.
-- Under enforce such a name answers as an unknown tenant at every door and
-- is never created again (design 10.1). Read by `tenant_standing` before a
-- request sets `antares.tenant`, like `tenants`, so neither table has
-- row-level security; the rename writes it as a BYPASSRLS role.

ALTER TABLE tenants ADD COLUMN organization_id text;

UPDATE tenants t
   SET organization_id = s.organization
  FROM (SELECT tenant_id,
               left(tenant_id, position('__' IN tenant_id) - 1) AS organization,
               substr(tenant_id, position('__' IN tenant_id) + 2) AS segment
          FROM tenants
         WHERE position('__' IN tenant_id) > 0) s
 WHERE t.tenant_id = s.tenant_id
   AND s.organization <> ''
   AND strpos(s.organization, '_') > 0
   AND s.segment <> ''
   AND strpos(s.segment, '_') = 0;

CREATE TABLE tenant_rename (
  old        text PRIMARY KEY,
  new        text NOT NULL,
  renamed_at timestamptz NOT NULL DEFAULT now()
);
