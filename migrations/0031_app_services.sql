-- Platform services for Apps (ADR-N-045, AP-165, AP-168, T-3583): what each App used a day, per
-- App (`subject` empty) and per person it reached, and the people who stopped an App's mail.
CREATE TABLE IF NOT EXISTS app_service_usage (
  project text   NOT NULL,
  app     text   NOT NULL,
  service text   NOT NULL,
  subject text   NOT NULL DEFAULT '',
  day     date   NOT NULL,
  used    bigint NOT NULL,
  PRIMARY KEY (project, app, service, subject, day)
);

CREATE TABLE IF NOT EXISTS email_unsubscribes (
  project text        NOT NULL,
  app     text        NOT NULL,
  subject text        NOT NULL,
  at      timestamptz NOT NULL DEFAULT now(),
  PRIMARY KEY (project, app, subject)
);
