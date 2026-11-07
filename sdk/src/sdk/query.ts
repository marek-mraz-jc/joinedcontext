// A query string without `URLSearchParams`: jc-functions runs QuickJS, which has no Web APIs, and
// the client is the same code in the browser and in a function. The output is byte for byte what
// `URLSearchParams#toString` writes (application/x-www-form-urlencoded).

/** One value encoded as a form field: `encodeURIComponent`, plus `!'()~` escaped and space as `+`. */
export function formEncode(value: string): string {
  return encodeURIComponent(value)
    .replace(/[!'()~]/g, (c) => `%${c.charCodeAt(0).toString(16).toUpperCase()}`)
    .replace(/%20/g, "+");
}

/** `name=value` pairs in insertion order; an `undefined` value leaves its name out. */
export function queryString(params: Record<string, string | undefined>): string {
  return Object.entries(params)
    .filter((entry): entry is [string, string] => entry[1] !== undefined)
    .map(([name, value]) => `${formEncode(name)}=${formEncode(value)}`)
    .join("&");
}

/** A v4 UUID from `crypto.randomUUID` where the runtime has it, from `Math.random` where it does not. */
export function randomId(): string {
  const webCrypto = (globalThis as { crypto?: { randomUUID?: () => string } }).crypto;
  if (typeof webCrypto?.randomUUID === "function") {
    return webCrypto.randomUUID();
  }
  // ponytail: Math.random ids for functions only; fine for entity local ids, never for secrets.
  return "xxxxxxxx-xxxx-4xxx-yxxx-xxxxxxxxxxxx".replace(/[xy]/g, (c) => {
    const r = Math.floor(Math.random() * 16);
    return (c === "x" ? r : (r & 0x3) | 0x8).toString(16);
  });
}
