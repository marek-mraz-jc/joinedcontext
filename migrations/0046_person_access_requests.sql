-- A person's access request on an Endpoint (EP-55, PF-58, T-5615): beside a pipeline's, a person
-- who reads an Endpoint but holds no grant through it asks its space's owners for a read of named
-- types. A person's row names the person and keeps `pipeline` empty, so a previous release still
-- reads every row; the grant goes to that person and nobody else.
ALTER TABLE access_requests ADD COLUMN IF NOT EXISTS person text;
ALTER TABLE access_requests ADD CONSTRAINT access_requests_one_requester
  CHECK ((person IS NULL) = (pipeline <> ''));
-- One open or granted request per pipeline, and per person, and Endpoint (PL-90).
DROP INDEX IF EXISTS access_requests_one_open;
CREATE UNIQUE INDEX IF NOT EXISTS access_requests_one_open
  ON access_requests (project, pipeline, endpoint_project, endpoint)
  WHERE state IN ('pending', 'approved') AND person IS NULL;
CREATE UNIQUE INDEX IF NOT EXISTS access_requests_one_open_person
  ON access_requests (endpoint_project, endpoint, person)
  WHERE state IN ('pending', 'approved') AND person IS NOT NULL;
CREATE INDEX IF NOT EXISTS access_requests_of_person
  ON access_requests (person, requested_at DESC) WHERE person IS NOT NULL;
