-- Roles and edits (T-3827, API/07): a steward publishes an indicator's forecast made with the
-- parameters they chose (the period and the horizon), with a note, which every reader sees and any
-- steward withdraws; a signed-in reader follows indicators of their own; every change leaves a row
-- in app_audit. Every statement may run again and change nothing.
create table if not exists published (
    id bigserial primary key,
    owner_id text not null,
    publisher_name text not null check (length(publisher_name) <= 200),
    kpi text not null check (length(kpi) <= 256),
    days integer not null check (days in (7, 30, 90)),
    horizon integer not null check (horizon between 1 and 48),
    note text not null check (length(note) between 1 and 200),
    -- The model's regular step, milliseconds, and `[{t, v, lo, hi}]` as `forecasts` keeps them.
    step double precision not null check (step > 0),
    points jsonb not null,
    created_at timestamptz not null default now()
);
create index if not exists published_kpi on published (kpi, id desc);

create table if not exists follows (
    id bigserial primary key,
    owner_id text not null,
    kpi text not null check (length(kpi) <= 256),
    created_at timestamptz not null default now(),
    unique (owner_id, kpi)
);

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
