-- bike-weather-demand's own table (T-3355). The reconciler runs this as the schema's owner, in
-- the App's own schema, and records it; `if not exists` keeps a second run harmless.

-- One model per station and day on Helsinki's calendar, trained on the seven days before: the
-- weather coefficients, the spread of what they leave, the hour-of-week profile, and the key of
-- the training data under the App's prefix. Kept, so a station's model has a history.
create table if not exists station_models (
    id bigserial primary key,
    station text not null check (length(station) between 5 and 300),
    trained_on date not null,
    trained_at timestamptz not null default now(),
    hours integer not null check (hours >= 0),
    enough boolean not null,
    per_degree double precision,
    rain double precision,
    sigma double precision,
    weather_station text,
    profile jsonb not null,
    snapshot text not null,
    unique (station, trained_on)
);
