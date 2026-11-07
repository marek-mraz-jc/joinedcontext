-- Roles and edits (T-3828, API/07): a signed-in reader keeps filters of their own, a name and the
-- page's address; a steward, the city's operator, acknowledges an alert with a comment, shown to
-- everyone, once per alert; every change leaves a row in app_audit. Every statement may run again
-- and change nothing.
create table if not exists filters (
    id bigserial primary key,
    owner_id text not null,
    name text not null check (length(name) between 1 and 80),
    view text not null check (length(view) <= 500),
    created_at timestamptz not null default now()
);
create index if not exists filters_owner on filters (owner_id, id desc);

create table if not exists acks (
    id bigserial primary key,
    owner_id text not null,
    owner_name text not null,
    alert text not null unique check (length(alert) between 19 and 300),
    comment text not null check (length(comment) between 1 and 300),
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
