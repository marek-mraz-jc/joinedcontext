-- Roles and edits (T-3828, API/07): a signed-in reader follows topics of their own, each by its
-- top keyword; a steward names a topic by its top keyword, and merges topics by giving them one
-- name, shown to everyone; every change leaves a row in app_audit. Every statement may run again
-- and change nothing.
create table if not exists follows (
    id bigserial primary key,
    owner_id text not null,
    term text not null check (length(term) between 1 and 60),
    created_at timestamptz not null default now(),
    unique (owner_id, term)
);

create table if not exists topic_names (
    id bigserial primary key,
    owner_id text not null,
    owner_name text not null,
    term text not null unique check (length(term) between 1 and 60),
    name text not null check (length(name) between 1 and 80),
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
