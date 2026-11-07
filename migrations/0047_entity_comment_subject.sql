-- The Keycloak `sub` of a comment's author (T-5697, F-POR4-024): it alone decides whose comment
-- it is, so a later account with a reused username can neither delete nor own an earlier one.
-- A comment stored before has none and stays readable; nobody can delete it from the Portal.
ALTER TABLE entity_comments ADD COLUMN IF NOT EXISTS author_subject text;
