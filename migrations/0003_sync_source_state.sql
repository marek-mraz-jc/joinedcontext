-- What a `SyncSource` run left behind, so a restarted Portal does not re-propose what it has
-- already proposed (MF-28, MF-30). The manifests stay in Git; this is the run's memory and
-- nothing else — no credential, no checkout, no copy of the source.
--
-- `last_error` is what makes the `Error` phase of MF-30 say something: a source whose origin
-- did not answer has no run to point at, only a reason.
--
-- `open_revision` is here because the branch name carries only the first seven characters of
-- the revision: when the merge request merges, this is what the source is then observed at.
CREATE TABLE IF NOT EXISTS sync_source_state (
    namespace         text    NOT NULL,
    name              text    NOT NULL,
    observed_revision text,
    last_run_at       bigint,
    open_proposal     text,
    open_revision     text,
    merge_request     text,
    last_error        text,
    paused            boolean NOT NULL DEFAULT false,
    updated_at        timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (namespace, name)
);
