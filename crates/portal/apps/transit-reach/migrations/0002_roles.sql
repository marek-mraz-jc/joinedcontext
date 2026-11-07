-- Roles and edits (T-3826, API/07): a signed-in visitor keeps origins of their own with the areas
-- reached from them, side by side to compare; a steward publishes a shared set every visitor sees;
-- every change leaves a row in app_audit. Every statement may run again and change nothing.
create table if not exists origins (
    id bigserial primary key,
    owner_id text not null,
    name text not null check (length(name) between 1 and 80),
    lon double precision not null check (lon between 24.4 and 25.6),
    lat double precision not null check (lat between 59.9 and 60.5),
    source text not null check (source in ('network', 'derived')),
    -- [{minutes, areaKm2, stops}] for 10, 20 and 30 minutes, as the page's analysis gave them.
    bands jsonb not null check (jsonb_typeof(bands) = 'array'),
    published boolean not null default false,
    created_at timestamptz not null default now()
);
create index if not exists origins_owner on origins (owner_id, id desc);
create index if not exists origins_published on origins (id desc) where published;

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
