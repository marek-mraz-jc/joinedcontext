-- What a person told the platform's owners from a page of the Portal (T-3272, API/01 §38).
--
-- No author, no e-mail, no address: the text is scrubbed of e-mail addresses, phone numbers and
-- credential-shaped words before it is written, and the page is kept without its query.
CREATE TABLE IF NOT EXISTS feedback (
  id         bigserial   PRIMARY KEY,
  created_at timestamptz NOT NULL DEFAULT now(),
  page       text        NOT NULL,
  version    text        NOT NULL,
  body       text        NOT NULL,
  screenshot bytea
);
