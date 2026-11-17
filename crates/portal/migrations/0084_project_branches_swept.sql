-- The expiry job of branch projects (ARCH/CHG-15; T-7215): when it deleted an expired branch's
-- rows. The branch's project_ids and project_branches rows stay as its tombstone.
ALTER TABLE project_branches ADD COLUMN IF NOT EXISTS swept_at timestamptz;
