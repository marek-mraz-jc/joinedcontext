-- The builds of the Apps on the forge, started by the Portal itself (AP-130, jc build lane): a
-- push to an App's default branch (its head differs from status.build.commit) or a Rebuild queues
-- one; the Portal starts its Job and the pod reports back with a token of this build alone. The
-- token is kept only as its SHA-256. At most one build per App waits and one runs.
CREATE TABLE IF NOT EXISTS app_builds (
    id bigserial PRIMARY KEY,
    project text NOT NULL,
    app text NOT NULL,
    commit text NOT NULL,
    class text NOT NULL CHECK (class IN ('node', 'rust')),
    trigger text NOT NULL CHECK (trigger IN ('push', 'rebuild')),
    requested_by text,
    state text NOT NULL CHECK (state IN ('pending', 'running', 'verifying', 'succeeded', 'failed')),
    token_sha256 bytea,
    expires_at timestamptz,
    created_at timestamptz NOT NULL DEFAULT now(),
    started_at timestamptz,
    finished_at timestamptz,
    digest text,
    reason text,
    log text
);
CREATE UNIQUE INDEX IF NOT EXISTS app_builds_one_pending ON app_builds (project, app)
    WHERE state = 'pending';
CREATE UNIQUE INDEX IF NOT EXISTS app_builds_one_running ON app_builds (project, app)
    WHERE state IN ('running', 'verifying');
CREATE INDEX IF NOT EXISTS app_builds_by_app ON app_builds (project, app, id DESC);
