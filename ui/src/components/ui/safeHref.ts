/**
 * PF-50: the schemes a link may carry. A URL reaches the Portal from a manifest, a status field
 * or a catalogue answer, so `href` is data, and `javascript:` in an `href` runs on click with the
 * page's own origin. Only what can navigate is allowed through.
 */
const SCHEMES = new Set(["http:", "https:", "mailto:"]);

/** A host no address of this page can name: what a relative path resolves against. */
const BASE = "https://portal.invalid";

/**
 * `path` when it is a path on this origin, `undefined` when it is not. A browser reads `/\\host`
 * and `/<tab>/host` as `//host`, another origin (F-UI1-001, F-UI3-007), so the path is resolved
 * the way the browser resolves it and kept only when it stays on the base.
 */
export function sameOriginPath(path: string): string | undefined {
  if (!/^[./?#]/.test(path)) return undefined;
  try {
    return new URL(path, `${BASE}/`).origin === BASE ? path : undefined;
  } catch {
    return undefined;
  }
}

/**
 * `href` when it is safe to put in a link, `undefined` when it is not (PF-50).
 *
 * Relative paths and fragments pass: they stay on this origin. An absolute URL passes only with
 * `http`, `https` or `mailto`. Everything else — `javascript:`, `data:`, `vbscript:`, a scheme
 * with a tab or a newline inside it — comes back `undefined`, and the caller renders the text
 * without a link rather than a link that does something else.
 */
export function safeHref(href: string | undefined | null): string | undefined {
  if (typeof href !== "string") return undefined;
  const trimmed = href.trim();
  if (trimmed === "") return undefined;
  // A relative path, a query or a fragment: no scheme, no authority, nothing to mistake.
  if (/^[./?#\\]/.test(trimmed)) return sameOriginPath(trimmed);
  try {
    // `URL` resolves the escapes and the control characters a hand-written check misses:
    // `java\tscript:alert(1)` parses as the `javascript:` it is.
    const parsed = new URL(trimmed, BASE);
    if (!SCHEMES.has(parsed.protocol)) return undefined;
    return trimmed;
  } catch {
    return undefined;
  }
}

/**
 * The address without its `user:password@` part, also when the password holds a raw `@`, `/`, `?` or
 * `#` (T-4991 review). The userinfo ends at the first `@` whose following host part has no `@`; an
 * `@` only in the path (`https://host/users/@me`) is left alone.
 */
export function withoutUserinfo(one: string): string {
  const scheme = /^[a-z][a-z0-9+.-]*:\/\//i.exec(one)?.[0];
  if (!scheme) return one;
  const rest = one.slice(scheme.length);
  const authority = (rest.split(/[/?#]/)[0] ?? "").replace(/\[[^\]]*\]/, "");
  if (!authority.includes("@") && !/:(?!\d*$)/.test(authority)) return one;
  for (let at = rest.indexOf("@"); at >= 0; at = rest.indexOf("@", at + 1)) {
    if (!(rest.slice(at + 1).split(/[/?#]/)[0] ?? "").includes("@")) return scheme + rest.slice(at + 1);
  }
  return one;
}

/**
 * The host to print beside a link's label, or none: a link inside the Portal, one never drawn, or
 * a label that names its own host as a whole word. Model or data text writes the label, so the
 * label may only stand for the target when one of its host-shaped words is the host itself;
 * `[hel.fi](https://el.fi/login)` contains "el.fi" and still prints it (F-R2UI2-004).
 */
export function hostToShow(label: string, href: string): string | undefined {
  const safe = safeHref(href);
  if (safe === undefined || safe.startsWith("/")) return undefined;
  let url: URL;
  try {
    url = new URL(safe);
  } catch {
    return undefined;
  }
  if (!url.host) return undefined;
  const words = label.toLowerCase().match(/[a-z0-9-]+(?:\.[a-z0-9-]+)+(?::\d+)?/g) ?? [];
  return words.some((word) => word === url.host || word === url.hostname) ? undefined : url.host;
}
