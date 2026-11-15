-- alerts-heatmap's own tables (T-3351). The reconciler runs this as the schema's owner, in the
-- App's own schema, and records it; `if not exists` keeps a second run harmless.

-- A hotspot report a reader saved: the view's address, what it kept and its top repeat places,
-- and the key of its map snapshot under the App's prefix once one was uploaded.
create table if not exists reports (
    id bigserial primary key,
    title text not null check (length(title) between 1 and 120),
    view text not null check (length(view) <= 500),
    kept integer not null check (kept >= 0),
    places jsonb not null default '[]'::jsonb,
    snapshot text,
    created_at timestamptz not null default now()
);

-- The repeat places of each week on Helsinki's calendar (Monday's date), kept after the feed has
-- dropped the week's alerts.
create table if not exists weekly_places (
    week date primary key,
    alerts integer not null check (alerts >= 0),
    places jsonb not null,
    computed_at timestamptz not null default now()
);
