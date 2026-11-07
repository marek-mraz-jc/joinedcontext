/**
 * The keys whose string value is a secret somebody pasted instead of referencing (MF-24).
 *
 * The same list the Portal refuses on, `SECRET_KEYS` in `src/api/mutate.rs`. It is here so the
 * person is told before the upload leaves the browser rather than after the whole bundle is
 * refused; the server's refusal is the one that counts and is not weakened by this.
 */
const SECRET_KEYS = [
  "password",
  "token",
  "secret",
  "clientSecret",
  "apiKey",
  "client_secret",
  "api_key",
  "apiToken",
  "api_token",
];

/**
 * A literal secret in the text of a bundle, as `key: value` on one line.
 *
 * Text rather than parsed YAML: an archive is bytes, a stream is many documents, and the check
 * is a courtesy before the upload, not the gate. `secretRef:` and any other key whose value is
 * an object are left alone — a reference is what the manifest is supposed to carry.
 */
export function pastedSecrets(text: string): string[] {
  const found: string[] = [];
  for (const line of text.split(/\r?\n/)) {
    const match = /^\s*(?:-\s*)?([A-Za-z_][A-Za-z0-9_]*)\s*:\s*(\S.*)$/.exec(line);
    if (!match) {
      continue;
    }
    const [, key, value] = match;
    const written = value.trim();
    // An object or an empty value is a reference or a nested block, not a pasted credential.
    if (!SECRET_KEYS.includes(key) || written === "" || written.startsWith("{")) {
      continue;
    }
    if (!found.includes(key)) {
      found.push(key);
    }
  }
  return found;
}
