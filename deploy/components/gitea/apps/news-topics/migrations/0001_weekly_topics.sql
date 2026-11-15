-- news-topics' own tables (T-3352). The reconciler runs this as the schema's owner, in the App's
-- own schema, and records it; `if not exists` keeps a second run harmless.

-- One topic model per ISO week (`2026-W41`): how many articles it read and when.
create table if not exists topic_runs (
    week text primary key check (week ~ '^[0-9]{4}-W[0-9]{2}$'),
    articles integer not null check (articles >= 0),
    computed_at timestamptz not null default now()
);

-- The topics of a week's model, the largest first: its share of the week's articles and its
-- keywords with their weights.
create table if not exists week_topics (
    week text not null references topic_runs (week) on delete cascade,
    topic integer not null check (topic >= 0),
    share real not null check (share >= 0 and share <= 1),
    articles integer not null check (articles >= 0),
    keywords jsonb not null,
    primary key (week, topic)
);
