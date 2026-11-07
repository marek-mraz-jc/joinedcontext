-- What the server of kpi-forecast keeps (T-3350). The reconciler runs this as the schema's owner
-- at publish; the App itself never runs DDL (ADR-N-044 §2.3). Every statement may run again and
-- change nothing.

-- Every indicator's forecast, once a day per period, kept with the day it was made, so a later
-- page can set it against what was measured. The Endpoint is public and its policies are its own
-- role: every visitor reads the same history, so the day's first visit records the day's forecast.
create table if not exists forecasts (
    kpi text not null check (length(kpi) <= 256),
    made_on date not null,
    days integer not null check (days in (7, 30, 90)),
    -- The model's regular step and the readings it was made from, milliseconds.
    step double precision not null check (step > 0),
    latest_t double precision not null,
    latest_v double precision not null,
    direction text check (direction is null or direction in ('up', 'down', 'flat')),
    -- `[{t, v, lo, hi}]`: the value expected at `t` and its 95 % interval.
    points jsonb not null,
    made_at timestamptz not null default now(),
    primary key (kpi, made_on, days)
);
create index if not exists forecasts_made_on on forecasts (made_on);

-- The monthly reports under `reports/{YYYY-MM}.csv`, and when each was last written.
create table if not exists reports (
    month text primary key check (month ~ '^[0-9]{4}-(0[1-9]|1[0-2])$'),
    written_at timestamptz not null default now()
);
