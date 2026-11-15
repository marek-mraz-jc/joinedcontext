-- What the server of air-weather-explorer keeps (T-3348). The reconciler runs this as the
-- schema's owner at publish; the App itself never runs DDL (ADR-N-044 §2.3). Every statement may
-- run again and change nothing.

-- Each station whose readings are cached, and up to when they were read. The App's Endpoint is
-- public and its policies are the Endpoint's own role, so every caller reads the same readings:
-- what one caller cached, any other may be shown.
create table if not exists stations (
    id text primary key check (length(id) <= 256),
    kind text not null check (kind in ('air', 'weather')),
    -- Epoch milliseconds: readings before this instant are in `hourly`.
    fetched_until bigint not null
);

-- The hourly mean of each attribute, keyed by the hour's start in epoch milliseconds; a week and a
-- day are kept per station.
create table if not exists hourly (
    station text not null references stations (id) on delete cascade,
    attr text not null,
    hour bigint not null,
    value double precision not null,
    primary key (station, attr, hour)
);

-- A comparison saved under a code a link carries.
create table if not exists comparisons (
    code text primary key check (code ~ '^[a-z0-9]{12}$'),
    name text check (name is null or length(name) between 1 and 80),
    station text not null check (length(station) <= 256),
    weather text not null check (length(weather) <= 256),
    days integer not null check (days in (1, 3, 7)),
    smoothing integer not null check (smoothing between 1 and 12),
    air text check (air is null or air in ('pm10', 'pm25', 'airQualityIndex')),
    variable text check (variable is null or variable in ('temperature', 'windSpeed', 'relativeHumidity', 'precipitation')),
    created_at timestamptz not null default now()
);
create index if not exists comparisons_created on comparisons (created_at);

-- The CSV exports under `exports/{code}.csv`, cleared a day after they were written.
create table if not exists exports (
    code text primary key check (code ~ '^[a-z0-9]{12}$'),
    created_at timestamptz not null default now()
);
create index if not exists exports_created on exports (created_at);
