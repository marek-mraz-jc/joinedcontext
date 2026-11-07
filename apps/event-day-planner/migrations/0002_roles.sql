-- Roles and edits (T-3826, API/07): a day shared while signed in is the sharer's own ("My days"),
-- one shared signed out is nobody's; a steward curates the featured events every visitor sees;
-- every change leaves a row in app_audit. Every statement may run again and change nothing.
alter table itineraries add column if not exists owner_id text not null default '';
alter table itineraries add column if not exists id bigserial;
create unique index if not exists itineraries_id on itineraries (id);
create index if not exists itineraries_owner on itineraries (owner_id, id desc);

-- The events a steward recommends, by their full id, with a short note, in the order added.
create table if not exists featured (
    id bigserial primary key,
    owner_id text not null default '',
    event_id text not null unique check (length(event_id) between 1 and 256),
    note text not null default '' check (length(note) <= 200),
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
