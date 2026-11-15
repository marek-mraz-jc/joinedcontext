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
        "The SHA256 fingerprint of the server's host key; the runner connects only to a server presenting this key. Ask the server's operator, or run ssh-keyscan -p 22 sftp.example.org | ssh-keygen -lf - and check it with them, e.g. SHA256:Nh0Me49Zh9fDw/VYUfq43IJmI1T+XrjiYONPND8GzaM.",
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

/** `fields` of input `name` with the runner's own fields placed after their anchors. */
export function withEkibenFields(name, fields) {
  const out = [...fields];
  for (const { after, ...field } of EKIBEN_FIELDS[name] ?? []) {
    const at = out.findIndex((f) => f.path === after);
    out.splice(at < 0 ? out.length : at + 1, 0, field);
  }
  return out;
}
