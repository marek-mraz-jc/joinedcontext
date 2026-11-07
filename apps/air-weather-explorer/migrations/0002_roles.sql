-- Roles and edits (T-3826, T-3827, API/07): a signed-in reader keeps places of their own, an air
-- quality station with a pollutant and the level above which it alerts them; a steward marks a
-- station's readings faulty for a period, shown to everyone and left out of every comparison;
-- every change leaves a row in app_audit. Every statement may run again and change nothing.
create table if not exists places (
    id bigserial primary key,
    owner_id text not null,
    station text not null check (length(station) <= 256),
    name text not null check (length(name) between 1 and 80),
    attr text not null check (attr in ('pm10', 'pm25', 'airQualityIndex')),
    threshold double precision not null check (threshold >= 0 and threshold <= 1000),
    created_at timestamptz not null default now()
);
create index if not exists places_owner on places (owner_id, id desc);

-- Epoch milliseconds, as `hourly` keeps its hours.
create table if not exists faults (
    id bigserial primary key,
    owner_id text not null,
    station text not null check (length(station) <= 256),
    from_ms bigint not null,
    to_ms bigint not null,
    note text not null check (length(note) between 1 and 200),
    created_at timestamptz not null default now(),
    check (from_ms < to_ms)
);
create index if not exists faults_station on faults (station, from_ms desc);

-- jc_app_sdk::access::AUDIT_TABLE
create table if not exists app_audit (
    id bigserial primary key,
    at timestamptz not null default now(),
    actor_id text not null,
    actor_name text not null,
    action text not null check (action in ('insert', 'update', 'delete', 'edit')),
    target text not null,
    summary text not null check (length(summary) <= 500)
);
