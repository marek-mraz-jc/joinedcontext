-- No API keys (PF-36, PF-104, owner 2026-10-18, T-4362): a ServiceAccount authenticates with its
-- Keycloak client only, and nothing honours a `jc_` key any more. The keys' hashes and the
-- waiting claims are dropped with their tables; no data is kept for a key nothing accepts.
DROP TABLE IF EXISTS service_account_key_claims;
DROP TABLE IF EXISTS service_account_keys;
