-- The saved rebalancing plans and the routes a van actually drove (T-3346). The reconciler runs
-- this as the schema's owner at publish; the App itself never runs DDL (ADR-N-044 §2.3). Every
-- statement may run again and change nothing.
create table if not exists plans (
    id bigserial primary key,
    -- Who the plan is for: a van or a crew, as the operator names it. A label, not an account.
    operator text not null check (length(operator) between 1 and 60),
    van_capacity integer not null check (van_capacity between 1 and 200),
    -- The station the van starts at, the stations added to and left out of the route.
    start_station text,
    include jsonb not null default '[]',
    exclude jsonb not null default '[]',
    -- The stops as the planner made them from the counts read at `created_at`.
    stops jsonb not null,
    km double precision not null check (km >= 0),
    moved integer not null check (moved >= 0),
    -- The route sheet under the App's prefix, `sheets/{id}.csv`.
    sheet text,
    created_at timestamptz not null default now()
);
create index if not exists plans_operator on plans (operator, id desc);

create table if not exists drives (
    id bigserial primary key,
    plan_id bigint not null references plans (id) on delete cascade,
    -- The station ids in the order the van reached them, a subset of the plan's stops.
    stops jsonb not null,
    km double precision check (km is null or km between 0 and 2000),
    note text check (note is null or length(note) <= 500),
    driven_at timestamptz not null default now()
);
create index if not exists drives_plan on drives (plan_id, id desc);
