-- The tenancy key on a person's own rows (ARCH/STO-02, STO-03; review S1-27, T-7282), phase one:
-- `connectors`, `connector_calls` and `user_preferences` were read on the bare pool by subject or
-- connector id, so any statement read or overwrote a person's connectors and preferences of every
-- Organization. Each row now takes the reserved project key (TEN-11) of the Organization the
-- person used the Portal in: the request's Organization host, else the installation's.
--
-- The columns arrive nullable here. Between this migration and 0088 the Portal keys the rows
-- written before under JC_PORTAL_INSTALLATION_ORGANIZATION's reserved project
-- (`project_ids::key_person_rows`, run by `db::connect_with`), and 0088 forces the key.

ALTER TABLE connectors
    ADD COLUMN IF NOT EXISTS org_id uuid
        DEFAULT nullif(current_setting('jc.org', true), '')::uuid,
    ADD COLUMN IF NOT EXISTS project_id uuid
        DEFAULT nullif(current_setting('jc.project', true), '')::uuid;
ALTER TABLE connector_calls
    ADD COLUMN IF NOT EXISTS org_id uuid
        DEFAULT nullif(current_setting('jc.org', true), '')::uuid,
    ADD COLUMN IF NOT EXISTS project_id uuid
        DEFAULT nullif(current_setting('jc.project', true), '')::uuid;
ALTER TABLE user_preferences
    ADD COLUMN IF NOT EXISTS org_id uuid
        DEFAULT nullif(current_setting('jc.org', true), '')::uuid,
    ADD COLUMN IF NOT EXISTS project_id uuid
        DEFAULT nullif(current_setting('jc.project', true), '')::uuid;
