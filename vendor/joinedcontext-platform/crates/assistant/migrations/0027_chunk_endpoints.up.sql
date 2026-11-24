-- T-7789: the Endpoint addresses each passage names, so a citation's page (endpoint_page,
-- dataset_page) and the narrowing check (dataset_reads_projects) look a slug and a type up in an
-- index. They matched `c.text ~ '/api/endpoint/…'` over every passage of the sources: on prod the
-- dataset page took 1.5 s and the narrowing waited out its 2 s inside a 36 s answer.
--
-- One row per passage, slug and type: `kind` is '' for the slug alone (every address names one),
-- else the type of `/api/endpoint/{slug}/ngsi-ld/v1/entities?type={kind}` or
-- `/api/endpoint/{slug}/odata/v4/{kind}`; `narrows` when the NGSI-LD address goes on with
-- `&q=isPartOf`, as a CKAN page that keeps its own rows writes it. The passage's URL, site and
-- visibility ride on each row, so a public channel filters by the page's own visibility and no
-- internal page is named to a visitor.
CREATE TABLE chunk_endpoints (
    project    text NOT NULL,
    chunk_id   bigint NOT NULL REFERENCES chunks ON DELETE CASCADE,
    site_id    bigint NOT NULL,
    url        text NOT NULL,
    visibility text NOT NULL CHECK (visibility IN ('public', 'internal')),
    slug       text NOT NULL CHECK (slug ~ '^[a-z0-9-]{1,63}$'),
    kind       text NOT NULL CHECK (kind = '' OR kind ~ '^[A-Za-z][A-Za-z0-9_]{0,99}$'),
    narrows    boolean NOT NULL DEFAULT false,
    PRIMARY KEY (chunk_id, slug, kind)
);
CREATE INDEX chunk_endpoints_lookup ON chunk_endpoints (project, slug, kind);

-- The addresses of a passage's text. A slug longer than a DNS label, or a type that is no plain
-- name, is left out: no lookup asks for one.
CREATE FUNCTION jc_chunk_endpoints(text text)
    RETURNS TABLE (slug text, kind text, narrows boolean)
    LANGUAGE sql IMMUTABLE PARALLEL SAFE
    AS $$
        WITH m AS (
            SELECT r[1] AS slug, r[3] AS kind,
                   r[2] = 'ngsi-ld/v1/entities?type=' AND r[4] IS NOT NULL AS narrows
            FROM regexp_matches(text,
                '/api/endpoint/([a-z0-9-]+)(?:/(ngsi-ld/v1/entities\?type=|odata/v4/)([A-Za-z][A-Za-z0-9_]*)(&q=isPartOf)?)?',
                'g') AS r
        ), named AS (
            SELECT slug, '' AS kind, false AS narrows FROM m
            UNION ALL
            SELECT slug, kind, narrows FROM m WHERE kind IS NOT NULL
        )
        SELECT slug, kind, bool_or(narrows) FROM named
        WHERE char_length(slug) <= 63 AND char_length(kind) <= 100
        GROUP BY slug, kind
    $$;

-- A stored or rewritten passage's addresses, in the statement that writes it.
CREATE FUNCTION jc_chunk_endpoints_write() RETURNS trigger
    LANGUAGE plpgsql AS $$
BEGIN
    IF TG_OP = 'UPDATE' THEN
        DELETE FROM chunk_endpoints WHERE chunk_id = OLD.id;
    END IF;
    INSERT INTO chunk_endpoints (project, chunk_id, site_id, url, visibility, slug, kind, narrows)
    SELECT NEW.project, NEW.id, NEW.site_id, NEW.url, NEW.visibility, e.slug, e.kind, e.narrows
    FROM jc_chunk_endpoints(NEW.text) AS e;
    RETURN NULL;
END $$;

CREATE TRIGGER chunks_endpoints AFTER INSERT OR UPDATE OF text, url, visibility, site_id ON chunks
    FOR EACH ROW EXECUTE FUNCTION jc_chunk_endpoints_write();

-- The passages stored before: only those that name an Endpoint at all, a few seconds on prod's
-- 112k. The owner reads every project's rows here, so the forced row-level security is lifted for
-- this statement and put back in the same transaction; no session sees the tables unforced.
ALTER TABLE chunks NO FORCE ROW LEVEL SECURITY;
INSERT INTO chunk_endpoints (project, chunk_id, site_id, url, visibility, slug, kind, narrows)
SELECT c.project, c.id, c.site_id, c.url, c.visibility, e.slug, e.kind, e.narrows
FROM chunks c, jc_chunk_endpoints(c.text) AS e
WHERE strpos(c.text, '/api/endpoint/') > 0;
ALTER TABLE chunks FORCE ROW LEVEL SECURITY;

ALTER TABLE chunk_endpoints ENABLE ROW LEVEL SECURITY;
ALTER TABLE chunk_endpoints FORCE ROW LEVEL SECURITY;
CREATE POLICY chunk_endpoints_by_project ON chunk_endpoints
    USING (project = current_setting('jc.project', true))
    WITH CHECK (project = current_setting('jc.project', true));
