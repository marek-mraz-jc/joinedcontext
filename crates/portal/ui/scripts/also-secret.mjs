// Shared by the input catalog and the processor forms (T-2239, T-3088).
// Fields the runner's own documentation does not mark `is_secret` and that carry a credential all
// the same (T-2239, MF-24). Matched by path suffix, so one entry covers every input that has the
// field. A field this list misses renders as a plain text box in the Data Sources form and escapes
// the `${VAR}` rule, which is how an OAuth bearer reached a manifest and Git.
export const ALSO_SECRET = [
  "oauth.access_token", // the bearer itself (http_client, websocket)
  "digest_auth.password", // a password (http_client)
  "sasl.access_token", // the bearer itself (kafka)
  "credentials.token", // an AWS session token (aws_*, sql_*, kafka's sasl.aws)
  "credentials.id", // an AWS access key id: the other half of a credential pair
  "auth.token.token", // pulsar's token
  "api_key", // twitter_search
];

/** Whether a field path is one this platform calls a secret although the runner does not. */
export function alsoSecret(path) {
  return ALSO_SECRET.some((suffix) => path === suffix || path.endsWith(`.${suffix}`));
}
