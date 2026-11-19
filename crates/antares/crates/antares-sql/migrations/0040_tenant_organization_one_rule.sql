-- One Organization-of-Tenant rule (S5-48, T-7306).
--
-- 0036 filled `tenants.organization_id` by the 2026-11-15 form only; 0037
-- added `tenant_organization()`, which also reads the project-key form
-- `{OrgId}__{ProjectId}__{space}`, and the broker bound the value of the
-- Rust `TenantId::organization_of` on every claim. Three copies of one rule:
-- a disagreement would make a Tenant read as reassigned, an unknown Tenant
-- under ANTARES_TENANT_ORGANIZATION=enforce.
--
-- From here the stored value has one source: the claim writes
-- `tenant_organization($1)` (store/pg `claim_tenant`, `ensure_tenant`), and
-- the test `tenant_organization_agrees_with_rust` holds the SQL function to
-- the Rust rule the request door compares against. This migration fills the
-- rows 0036 left NULL because their name is of the project-key form. A
-- stored value that disagrees with its name is left as it is: that is the
-- reassignment the door refuses, and rewriting it here would lift the
-- refusal.
UPDATE tenants
   SET organization_id = tenant_organization(tenant_id)
 WHERE organization_id IS NULL
   AND tenant_organization(tenant_id) IS NOT NULL;
