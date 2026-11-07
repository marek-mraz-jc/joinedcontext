-- The weekly link check (T-5133): per pipeline the newest check of the links its entities carry,
-- what the source health view shows (counts and the first dead links) and each failing link's
-- failures in a row. Public page addresses and HTTP statuses only: never a token or a record.
CREATE TABLE IF NOT EXISTS link_check (
  project    text        NOT NULL,
  pipeline   text        NOT NULL,
  checked_at timestamptz NOT NULL,
  result     jsonb       NOT NULL,
  PRIMARY KEY (project, pipeline)
);
