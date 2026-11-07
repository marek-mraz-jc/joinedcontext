-- A shared link names a comparison by an unguessable token, never by its id (T-3828): the id is a
-- bigserial anyone could count through. Two random UUIDs give 64 hex characters, 244 random bits;
-- a row kept before this file gets its own token when the column is added (a volatile default is
-- evaluated per row). Every statement may run again and change nothing.
alter table comparisons add column if not exists share text not null
    default replace(gen_random_uuid()::text || gen_random_uuid()::text, '-', '')
    check (share ~ '^[0-9a-f]{64}$');
create unique index if not exists comparisons_share on comparisons (share);
