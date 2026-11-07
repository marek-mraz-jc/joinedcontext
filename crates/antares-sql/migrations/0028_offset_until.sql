-- The newest observed_at of a temporal Entity's instance whose observedAt is
-- not a UTC ('Z') RFC 3339 stamp, or that has none (T-5616). lastN ranks the
-- observedAt text, which orders like observed_at only for UTC stamps, so it
-- read every row up to 28 hours (two RFC 3339 offsets) older than the N-th
-- newest: ~250 rows an Attribute for N = 10 on the helsinki-bikes history,
-- past the 30 s statement timeout for 577 stations. Where this column lies
-- more than 28 hours before the N-th newest, every row that could rank is
-- UTC and the read takes the N newest alone (store/pg/temporal.rs
-- `newest_rows`). The instance write keeps it.
--
-- Rows written before this migration are not recorded: an existing Entity
-- takes the migration's time (a stable default is stored once, in the
-- catalog: no rewrite, no scan), so its reads keep the 28 hours until its
-- N-th newest is 28 hours past it. A new Entity starts at -infinity.
ALTER TABLE temporal_entities ADD COLUMN offset_until timestamptz NOT NULL DEFAULT now();
ALTER TABLE temporal_entities ALTER COLUMN offset_until SET DEFAULT '-infinity';
