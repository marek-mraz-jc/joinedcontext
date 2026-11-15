-- Roles and edits (T-3827, API/07): a steward marks a finding resolved, with a note, or corrects
-- the attribute through the App's Endpoint and so resolves it; every reader sees what was
-- resolved, and any steward opens a finding again. Every change leaves a row in app_audit. Every
-- statement may run again and change nothing.
create table if not exists resolutions (
    id bigserial primary key,
    owner_id text not null,
    resolver_name text not null check (length(resolver_name) <= 200),
    entity text not null check (length(entity) <= 256),
    attribute text not null check (length(attribute) between 1 and 64),
    note text not null check (length(note) between 1 and 200),
    -- Whether the steward corrected the attribute through the Endpoint, or only marked it.
    corrected boolean not null default false,
    created_at timestamptz not null default now()
);
create index if not exists resolutions_finding on resolutions (entity, attribute);

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
