-- T-7524: lists page by key in byte order (COLLATE "C"), which the primary keys of a database
-- with another collation cannot serve. A release before it never reads these and still starts.
-- The wait for a table is bounded: a start fails and runs again rather than queue every write.
SET LOCAL lock_timeout = '10s';
CREATE INDEX IF NOT EXISTS idx_cr_dashboard_row ON cr_dashboard (org_id, uid COLLATE "C");
CREATE INDEX IF NOT EXISTS idx_cr_dashboard_version_row
    ON cr_dashboard_version (org_id, ((dashboard_uid || ':' || version::text)) COLLATE "C");
CREATE INDEX IF NOT EXISTS idx_cr_folder_row ON cr_folder (org_id, uid COLLATE "C");
CREATE INDEX IF NOT EXISTS idx_cr_datasource_row ON cr_datasource (org_id, uid COLLATE "C");
CREATE INDEX IF NOT EXISTS idx_cr_library_panel_row ON cr_library_panel (org_id, uid COLLATE "C");
CREATE INDEX IF NOT EXISTS idx_cr_public_dashboard_row
    ON cr_public_dashboard (org_id, uid COLLATE "C");
CREATE INDEX IF NOT EXISTS idx_cr_import_report_row ON cr_import_report (org_id, id COLLATE "C");
