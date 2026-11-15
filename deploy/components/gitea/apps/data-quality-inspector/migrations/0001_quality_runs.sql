-- data-quality-inspector's own tables (T-3354). The reconciler runs this as the schema's owner, in
-- the App's own schema, and records it; `if not exists` keeps a second run harmless.

-- Every quality run over the Endpoint's data, so the page shows the trend: what it read, how
-- complete and valid it was, how many findings it had. The full per-entity report of a run is a
-- file under the App's prefix, `runs/<id>/report.json`.
create table if not exists quality_runs (
    id bigserial primary key,
    ran_at timestamptz not null default now(),
    types integer not null check (types >= 0),
    entities integer not null check (entities >= 0),
    findings integer not null check (findings >= 0),
    completeness real not null check (completeness >= 0 and completeness <= 1),
    valid real check (valid >= 0 and valid <= 1)
);

-- One run's scores per entity type.
create table if not exists run_types (
    run_id bigint not null references quality_runs (id) on delete cascade,
    type text not null,
    entities integer not null check (entities >= 0),
    completeness real not null check (completeness >= 0 and completeness <= 1),
    valid real check (valid >= 0 and valid <= 1),
    findings integer not null check (findings >= 0),
    freshness_median_seconds double precision,
    primary key (run_id, type)
);
