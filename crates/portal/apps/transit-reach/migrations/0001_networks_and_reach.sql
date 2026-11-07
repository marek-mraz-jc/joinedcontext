-- What the server of transit-reach keeps (T-3349). The reconciler runs this as the schema's owner
-- at publish; the App itself never runs DDL (ADR-N-044 §2.3). Every statement may run again and
-- change nothing.

-- Each version of HSL's stops and lines the server has read: the SHA-256 of the network as it was
-- read, its file under `networks/{version}.json`, and when the registers were last read and found
-- the same.
create table if not exists networks (
    version text primary key check (version ~ '^[0-9a-f]{64}$'),
    stops integer not null check (stops >= 0),
    routes integer not null check (routes >= 0),
    read_at timestamptz not null default now(),
    checked_at timestamptz not null default now()
);

-- The area reached from one stop in each band, on one version of the network, and its GeoJSON
-- under `tiles/{version}/{stop}.geojson`. A newer network drops the older one's rows.
create table if not exists reach (
    version text not null references networks (version) on delete cascade,
    stop text not null check (length(stop) <= 256),
    bands jsonb not null,
    tile text not null,
    computed_at timestamptz not null default now(),
    primary key (version, stop)
);
