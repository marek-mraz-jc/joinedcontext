// Fields the pipeline runner (ekiben) adds to a Bento input, so the Data Sources form can fill
// them: each entry goes in after the field named by `after` (T-7090). The runner's lint refuses
// a config without the required ones, so an input whose form lacked them could never be saved.
export const EKIBEN_FIELDS = {
  sftp: [
    {
      after: "address",
      path: "host_key_fingerprint",
      type: "string",
      kind: "scalar",
      secret: false,
      advanced: false,
      optional: false,
      default: null,
      description:
        "The SHA256 fingerprint of the server's host key as its operator confirms it (ssh-keyscan sftp.example.org | ssh-keygen -lf - shows it); the runner connects only to that key, e.g. SHA256:Nh0Me49Zh9fDw/VYUfq43IJmI1T+XrjiYONPND8GzaM.",
    },
    {
      after: "credentials.password",
      path: "credentials.private_key",
      type: "string",
      kind: "scalar",
      secret: true,
      advanced: false,
      optional: false,
      default: "",
      description:
        "An OpenSSH Ed25519 or ECDSA private key (PEM, -----BEGIN OPENSSH PRIVATE KEY-----) to log in with instead of a password.",
    },
  ],
};

// Help the runner gives in place of Bento's, where Bento's has no example or none at all (T-7093:
// kafka_franz's topics has no description; its consumer group is joined under the organization).
export const EKIBEN_HELP = {
  kafka_franz: {
    seed_brokers: "The Kafka brokers to start from, as host:port; the runner reaches them, and every broker they advertise, only through the organization's outbound rule, e.g. kafka-1.example.org:9092.",
    topics: "The topics to read. With regexp_topics each one is a regular expression the whole topic name must match, e.g. vehicle-positions.",
    consumer_group: "The consumer group to read and commit offsets as; the runner joins it under the organization's id, as <organization id>.<name>, so grant that group to the SASL user, e.g. traffic-ingest.",
    sasl: "The login, as a YAML list with one mechanism: SCRAM-SHA-512, SCRAM-SHA-256, or PLAIN over TLS; the password only from a secret, e.g. - {mechanism: SCRAM-SHA-512, username: city-ingest, password: ${KAFKA_PASSWORD}}",
  },
};

/** `fields` of input `name` with the runner's own fields placed after their anchors. */
export function withEkibenFields(name, fields) {
  const help = EKIBEN_HELP[name] ?? {};
  const out = fields.map((f) => (help[f.path] ? { ...f, description: help[f.path] } : f));
  for (const { after, ...field } of EKIBEN_FIELDS[name] ?? []) {
    const at = out.findIndex((f) => f.path === after);
    out.splice(at < 0 ? out.length : at + 1, 0, field);
  }
  return out;
}
