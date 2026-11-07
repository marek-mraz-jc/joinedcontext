-- Roles and edits (T-3826, API/07): a plan and a drive belong to the person who saved them, the
-- planner's levels are one row a steward changes for everyone, and every change leaves a row in
-- app_audit. Every statement may run again and change nothing.
alter table plans add column if not exists owner_id text not null default '';
alter table drives add column if not exists owner_id text not null default '';
create index if not exists plans_owner on plans (owner_id, id desc);

-- The shares of a station's places the planner reads: below `low` it is about to run empty, above
-- `high` about to be full, and the van brings it back to `target`.
create table if not exists levels (
    id bigint primary key check (id = 1),
    owner_id text not null default '',
    low double precision not null,
    high double precision not null,
    target double precision not null,
    check (0 <= low and low < target and target < high and high <= 1)
);
insert into levels (id, low, high, target) values (1, 0.15, 0.85, 0.5) on conflict (id) do nothing;

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
