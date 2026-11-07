-- A pipeline access request may ask for a read and its history (PL-117).
ALTER TABLE access_requests DROP CONSTRAINT IF EXISTS access_requests_operations_check;
ALTER TABLE access_requests
  ADD CONSTRAINT access_requests_operations_check CHECK (operations IN ('read', 'write', 'history'));
