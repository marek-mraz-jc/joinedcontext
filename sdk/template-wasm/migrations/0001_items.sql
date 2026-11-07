-- The App's own table (AP-148). The reconciler runs this as the schema's owner, in the App's own
-- schema; `if not exists` keeps a second run harmless.
create table if not exists items (
    id bigserial primary key,
    text text not null check (length(text) between 1 and 500),
    created_at timestamptz not null default now()
);
