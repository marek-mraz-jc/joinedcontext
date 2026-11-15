-- district-compare's own table (T-3353). The reconciler runs this as the schema's owner, in the
-- App's own schema, and records it; `if not exists` keeps a second run harmless.

-- What each district counted on a day on Helsinki's calendar, as the comparison computed it from
-- the feeds that day: kept after the feeds have moved on, so a district has a history.
create table if not exists district_metrics (
    day date not null,
    code text not null check (length(code) between 1 and 20),
    name text not null,
    area_km2 double precision not null check (area_km2 >= 0),
    events integer not null check (events >= 0),
    bikes integer not null check (bikes >= 0),
    bike_slots integer not null check (bike_slots >= 0),
    alerts integer not null check (alerts >= 0),
    pm25 double precision,
    aqi double precision,
    computed_at timestamptz not null default now(),
    primary key (day, code)
);
