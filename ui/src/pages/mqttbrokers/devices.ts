/**
 * A device's key pair for the Add device form (T-5532, PF-47): generated in this browser with
 * WebCrypto, or the public half pasted as a JWKS, a single JWK or a PEM. Only the public key ever
 * leaves the browser; the private key is the person's download. The rules mirror jc-core's
 * `Jwks::validate`, which the Portal applies again.
 */

/** One public signing key as the ServiceAccount stores it under `spec.credentials[].jwks`. */
export interface PublicJwk {
  kty: "EC" | "RSA";
  kid: string;
  alg: "ES256" | "RS256";
  use: "sig";
  crv?: "P-256";
  x?: string;
  y?: string;
  n?: string;
  e?: string;
}

export interface Jwks {
  keys: PublicJwk[];
}

/** Why a pasted key was refused: a key of `mqttbrokers.device.keyError.*`. */
export type KeyProblem = "empty" | "notKey" | "private" | "unsupported" | "tooMany" | "kidTwice";

export type Parsed = { ok: true; jwks: Jwks } | { ok: false; problem: KeyProblem };

/** JWK members of a private or symmetric key (RFC 7518 §6), refused on sight. */
const PRIVATE_MEMBERS = ["d", "p", "q", "dp", "dq", "qi", "oth", "k"];
const MAX_KEYS = 5;

function base64(bytes: ArrayBuffer): string {
  let text = "";
  for (const byte of new Uint8Array(bytes)) text += String.fromCharCode(byte);
  return btoa(text);
}

function base64url(bytes: ArrayBuffer): string {
  return base64(bytes).replaceAll("+", "-").replaceAll("/", "_").replace(/=+$/, "");
}

/** The RFC 7638 thumbprint of a public key: the `kid` a key without one gets. */
async function thumbprint(key: Omit<PublicJwk, "kid" | "alg" | "use">): Promise<string> {
  const members =
    key.kty === "EC"
      ? { crv: key.crv, kty: key.kty, x: key.x, y: key.y }
      : { e: key.e, kty: key.kty, n: key.n };
  return base64url(await crypto.subtle.digest("SHA-256", new TextEncoder().encode(JSON.stringify(members))));
}

/** One JWK as jc-core reads it, or why not: P-256 for ES256 or RSA-2048+ for RS256, public only. */
async function publicKey(raw: unknown): Promise<PublicJwk | KeyProblem> {
  if (typeof raw !== "object" || raw === null || Array.isArray(raw)) return "notKey";
  const key = raw as Record<string, unknown>;
  if (PRIVATE_MEMBERS.some((member) => member in key)) return "private";
  const text = (member: string) => (typeof key[member] === "string" ? (key[member] as string) : undefined);
  let picked: Omit<PublicJwk, "kid" | "alg" | "use">;
  if (key.kty === "EC") {
    const x = text("x");
    const y = text("y");
    if (key.crv !== "P-256" || x?.length !== 43 || y?.length !== 43 || (key.alg ?? "ES256") !== "ES256") {
      return "unsupported";
    }
    picked = { kty: "EC", crv: "P-256", x, y };
  } else if (key.kty === "RSA") {
    const n = text("n");
    const e = text("e");
    if (!n || n.length < 342 || !e || (key.alg ?? "RS256") !== "RS256") return "unsupported";
    picked = { kty: "RSA", n, e };
  } else {
    return key.kty === undefined ? "notKey" : "unsupported";
  }
  if ((key.use ?? "sig") !== "sig") return "unsupported";
  const kid = text("kid")?.trim() || (await thumbprint(picked));
  return { ...picked, kid: kid.slice(0, 128), alg: picked.kty === "EC" ? "ES256" : "RS256", use: "sig" };
}

/** A PEM `PUBLIC KEY` (SPKI) read through WebCrypto as P-256, then as RSA. */
async function fromPem(pem: string): Promise<unknown> {
  const body = pem.replace(/-----(BEGIN|END) PUBLIC KEY-----/g, "").replace(/\s+/g, "");
  let der: Uint8Array<ArrayBuffer>;
  try {
    der = Uint8Array.from(atob(body), (char) => char.charCodeAt(0));
  } catch {
    return undefined;
  }
  for (const algorithm of [
    { name: "ECDSA", namedCurve: "P-256" },
    { name: "RSASSA-PKCS1-v1_5", hash: "SHA-256" },
  ]) {
    try {
      const key = await crypto.subtle.importKey("spki", der, algorithm, true, ["verify"]);
      return await crypto.subtle.exportKey("jwk", key);
    } catch {
      // The next algorithm, or not a key this form takes.
    }
  }
  return undefined;
}

/** A pasted public key set: JWKS JSON, one JWK or a PEM public key. */
export async function parsePublicKey(input: string): Promise<Parsed> {
  const text = input.trim();
  if (text === "") return { ok: false, problem: "empty" };
  if (/PRIVATE KEY-----/.test(text)) return { ok: false, problem: "private" };
  let raws: unknown[];
  if (text.startsWith("-----BEGIN PUBLIC KEY-----")) {
    const jwk = await fromPem(text);
    if (jwk === undefined) return { ok: false, problem: "unsupported" };
    raws = [jwk];
  } else {
    let json: unknown;
    try {
      json = JSON.parse(text);
    } catch {
      return { ok: false, problem: "notKey" };
    }
    const keys = (json as { keys?: unknown } | null)?.keys;
    raws = Array.isArray(keys) ? keys : [json];
  }
  if (raws.length === 0) return { ok: false, problem: "notKey" };
  if (raws.length > MAX_KEYS) return { ok: false, problem: "tooMany" };
  const keys: PublicJwk[] = [];
  for (const raw of raws) {
    const key = await publicKey(raw);
    if (typeof key === "string") return { ok: false, problem: key };
    if (keys.some((seen) => seen.kid === key.kid)) return { ok: false, problem: "kidTwice" };
    keys.push(key);
  }
  return { ok: true, jwks: { keys } };
}

function pem(label: string, der: ArrayBuffer): string {
  const lines = base64(der).match(/.{1,64}/g) ?? [];
  return [`-----BEGIN ${label}-----`, ...lines, `-----END ${label}-----`, ""].join("\n");
}

/** A fresh P-256 key pair: the public JWKS for the account, the private key as PKCS#8 PEM. */
export async function generateDeviceKey(): Promise<{ jwks: Jwks; privatePem: string }> {
  const pair = await crypto.subtle.generateKey({ name: "ECDSA", namedCurve: "P-256" }, true, ["sign", "verify"]);
  const key = await publicKey(await crypto.subtle.exportKey("jwk", pair.publicKey));
  if (typeof key === "string") throw new Error("the browser made a key it cannot read back");
  return { jwks: { keys: [key] }, privatePem: pem("PRIVATE KEY", await crypto.subtle.exportKey("pkcs8", pair.privateKey)) };
}

/**
 * The commands a device runs to get its access token (RFC 7523 client assertion, PF-47): a JWT
 * signed with its private key file, sent to the realm's token endpoint. No secret is in them.
 */
export function tokenCommand(tokenUrl: string, issuer: string, client: string, key: PublicJwk, keyFile: string): string {
  const claims = `{"iss": "${client}", "sub": "${client}", "aud": "${issuer}", "jti": str(uuid.uuid4()), "exp": int(time.time()) + 60}`;
  return [
    `ASSERTION=$(python3 -c 'import jwt, time, uuid; print(jwt.encode(${claims}, open("${keyFile}").read(), algorithm="${key.alg}", headers={"kid": "${key.kid}"}))')`,
    `TOKEN=$(curl -s ${tokenUrl} \\`,
    `  -d grant_type=client_credentials -d client_id=${client} \\`,
    `  -d client_assertion_type=urn:ietf:params:oauth:client-assertion-type:jwt-bearer \\`,
    `  -d client_assertion="$ASSERTION" | jq -r .access_token)`,
  ].join("\n");
}
