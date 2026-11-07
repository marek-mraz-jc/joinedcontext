-- Comments on a space's entities and the notifications their mentions send (API/01 §34,
-- ADR-N-042 §3.1, T-3106). A comment names its entity by (project, space, urn) and changes no
-- data; a notification goes with the comment that sent it.
CREATE TABLE IF NOT EXISTS entity_comments (
  id          bigserial   PRIMARY KEY,
  project     text        NOT NULL,
  space       text        NOT NULL,
  urn         text        NOT NULL,
  author      text        NOT NULL,
  author_name text        NOT NULL,
  body        text        NOT NULL,
  mentions    text[]      NOT NULL DEFAULT '{}',
  created_at  timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX IF NOT EXISTS entity_comments_entity ON entity_comments (project, space, urn, id);

CREATE TABLE IF NOT EXISTS notifications (
  id          bigserial   PRIMARY KEY,
  recipient   text        NOT NULL,
  project     text        NOT NULL,
  space       text        NOT NULL,
  urn         text        NOT NULL,
  comment_id  bigint      NOT NULL REFERENCES entity_comments (id) ON DELETE CASCADE,
  author      text        NOT NULL,
  author_name text        NOT NULL,
  excerpt     text        NOT NULL,
  created_at  timestamptz NOT NULL DEFAULT now(),
  read_at     timestamptz
);
CREATE INDEX IF NOT EXISTS notifications_recipient ON notifications (recipient, id DESC);
