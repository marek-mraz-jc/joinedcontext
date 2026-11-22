-- The 4.22 reap of history reads an index (T-7517). `expiresAt` is text in
-- `data` and try_timestamptz is STABLE, so nothing could index it and every
-- tick read the whole history (dev: ~145 GB).
--
-- This function is the index key: IMMUTABLE, so it reads only a DateTime
-- that starts with a date (never `now` or `today`, which PostgreSQL's cast
-- takes) and always under UTC and ISO. Anything else is NULL: no expiry, as
-- try_timestamptz answers. A changed body needs each `*_expiry` index
-- rebuilt.
--
-- No index and no backfill here: the maintenance job builds the partial
-- index `{leaf}_expiry` on each history leaf with CREATE INDEX CONCURRENTLY
-- (store/pg/maintenance.rs `history_expiry_index`), which takes no lock
-- against writes and covers the rows already stored.
CREATE FUNCTION public.history_expires_at(data jsonb) RETURNS timestamptz
  LANGUAGE plpgsql IMMUTABLE PARALLEL SAFE
  SET search_path = pg_catalog, pg_temp
  SET TimeZone = 'UTC'
  SET DateStyle = 'ISO, MDY'
AS $$
DECLARE
  stamp text := data->>'expiresAt';
BEGIN
  IF stamp ~ '^[0-9]{4}-[0-9]{2}-[0-9]{2}([Tt ][0-9:.,]+([Zz]|[+-][0-9:]+)?)?$' THEN
    RETURN replace(stamp, ',', '.')::timestamptz;
  END IF;
  RETURN NULL;
EXCEPTION WHEN data_exception THEN
  RETURN NULL;
END $$;
