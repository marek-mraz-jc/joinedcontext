-- The certificate wave of the host registry (evidence/T-7129/design.md §3, T-7138). Each host on
-- its way to `served` says where it stands in `detail` (waiting for DNS, waiting for a
-- certificate slot and until when, a conflicting object), which the operator pages show.
-- `organization_host_certificates` is the ledger the wave paces new certificates by: at most 10
-- per rolling 24 hours per instance and 20 per rolling 7 days per registered domain (eTLD+1),
-- so two instances on one domain stay under Let's Encrypt's 50 a week.
ALTER TABLE organization_hosts ADD COLUMN IF NOT EXISTS detail text;
ALTER TABLE organization_hosts ADD COLUMN IF NOT EXISTS detail_at timestamptz;

CREATE TABLE IF NOT EXISTS organization_host_certificates (
    id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    host text NOT NULL,
    registered_domain text NOT NULL,
    requested_at timestamptz NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS organization_host_certificates_at
    ON organization_host_certificates (requested_at);
CREATE INDEX IF NOT EXISTS organization_host_certificates_domain
    ON organization_host_certificates (registered_domain, requested_at);
