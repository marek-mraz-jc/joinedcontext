-- Roles and edits (T-3827, API/07): a steward tunes a station's model inputs, a weather station to
-- use instead of the nearest, how far the nearest may be and how many days a model trains on; each
-- model keeps the days it trained on; every change leaves a row in app_audit. Every statement may
-- run again and change nothing.
create table if not exists station_inputs (
    station text primary key check (length(station) between 5 and 300),
    weather_station text check (length(weather_station) <= 300),
    radius_km double precision not null check (radius_km >= 1 and radius_km <= 25),
    days integer not null check (days between 1 and 7),
    note text not null default '' check (length(note) <= 200),
    updated_by text not null,
    updated_by_name text not null,
    updated_at timestamptz not null default now()
);

alter table station_models add column if not exists days integer check (days between 1 and 7);

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
