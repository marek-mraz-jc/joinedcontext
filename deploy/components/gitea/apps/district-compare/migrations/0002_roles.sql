-- Roles and edits (T-3828, API/07): a signed-in reader keeps comparisons of their own, a name,
-- up to six district codes and a measure, and shares one by its link; a steward pins the
-- comparison everyone opens first (the newest pin wins); every change leaves a row in app_audit.
-- Every statement may run again and change nothing.
create table if not exists comparisons (
    id bigserial primary key,
    owner_id text not null,
    owner_name text not null,
    name text not null check (length(name) between 1 and 80),
    codes text not null check (length(codes) between 1 and 130),
    measure text not null check (measure in ('events', 'bikes', 'bikeSlots', 'alerts', 'pm25', 'aqi')),
    created_at timestamptz not null default now()
);
create index if not exists comparisons_owner on comparisons (owner_id, id desc);

create table if not exists pins (
    id bigserial primary key,
    owner_id text not null,
    owner_name text not null,
    name text not null check (length(name) between 1 and 80),
    codes text not null check (length(codes) between 1 and 130),
    measure text not null check (measure in ('events', 'bikes', 'bikeSlots', 'alerts', 'pm25', 'aqi')),
    created_at timestamptz not null default now()
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
