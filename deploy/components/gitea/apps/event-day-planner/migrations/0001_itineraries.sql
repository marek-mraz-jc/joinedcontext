-- The days visitors shared (T-3347): each under a short id a link carries, its calendar file under
-- the App's prefix, `shares/{code}.ics`. The reconciler runs this as the schema's owner at
-- publish; the App itself never runs DDL (ADR-N-044 §2.3). Every statement may run again and
-- change nothing.
create table if not exists itineraries (
    code text primary key check (code ~ '^[a-z0-9]{12}$'),
    -- The Helsinki day, and the language the names were read in.
    day date not null,
    lang text not null check (lang in ('fi', 'en', 'sv')),
    -- The events picked, by the end of their id, as the page keeps them in `?pick=`.
    picks jsonb not null,
    -- The day as the planner made it when it was shared.
    items jsonb not null,
    conflicts jsonb not null default '[]',
    walk_km double precision not null check (walk_km >= 0),
    walk_minutes integer not null check (walk_minutes >= 0),
    created_at timestamptz not null default now()
);
-- A shared day is kept a week past the day itself; saving a new one clears the old ones.
create index if not exists itineraries_day on itineraries (day);
