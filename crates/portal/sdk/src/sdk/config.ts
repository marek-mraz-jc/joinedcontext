export interface JcUser {
  id: string;
  name?: string;
  email?: string;
  roles?: string[];
}

/** One endpoint of an application that reads several (SDK-02): the first one is the primary. */
export interface JcEndpoint {
  name: string;
  slug: string;
  space: string;
  /** The types the application's data needs read through this endpoint. */
  types: string[];
}

export interface JcConfig {
  slug: string;
  orgDomain: string;
  space: string;
  transport: "origin" | "bridge";
  appName?: string;
  endpointName?: string;
  basemap?: string;
  language?: string;
  user?: JcUser | null;
  /** Every endpoint of an application that reads several, the primary first; absent for one. */
  endpoints?: JcEndpoint[];
  /** The Portal's page of the App's project, where the entity panel links an entity (SDK-40). */
  portal?: string;
}

export class ConfigError extends Error {
  constructor(message: string) {
    super(message);
    this.name = "ConfigError";
  }
}

const SLUG_RE = /^[a-z0-9]{1,64}$/;
const APP_NAME_RE = /^[a-z0-9]([a-z0-9-]{0,61}[a-z0-9])?$/;

export function parseConfig(input: unknown): JcConfig {
  const errors: string[] = [];

  if (typeof input !== "object" || input === null || Array.isArray(input)) {
    throw new ConfigError("Config must be an object");
  }

  const raw = input as Record<string, unknown>;

  if (typeof raw.slug !== "string" || !SLUG_RE.test(raw.slug)) {
    errors.push("slug: must match ^[a-z0-9]{1,64}$");
  }

  if (typeof raw.orgDomain !== "string" || raw.orgDomain.trim() === "" || /[:/]/.test(raw.orgDomain)) {
    errors.push("orgDomain: must be non-empty and contain no ':' or '/'");
  }

  if (typeof raw.space !== "string" || raw.space.trim() === "" || /[:/]/.test(raw.space)) {
    errors.push("space: must be non-empty and contain no ':' or '/'");
  }

  if (raw.transport !== "origin" && raw.transport !== "bridge") {
    errors.push("transport: must be 'origin' or 'bridge'");
  }

  if (raw.transport === "origin") {
    if (typeof raw.appName !== "string" || !APP_NAME_RE.test(raw.appName)) {
      errors.push("appName: required when transport is 'origin', must match ^[a-z0-9]([a-z0-9-]{0,61}[a-z0-9])?$");
    }
  } else if (raw.appName !== undefined) {
    if (typeof raw.appName !== "string" || !APP_NAME_RE.test(raw.appName)) {
      errors.push("appName: must match ^[a-z0-9]([a-z0-9-]{0,61}[a-z0-9])?$");
    }
  }

  if (raw.basemap !== undefined) {
    if (typeof raw.basemap !== "string") {
      errors.push("basemap: must be an http(s) URL");
    } else {
      // A pattern, not `new URL`: the SDK runs where no Web API exists too (jc-functions).
      if (!/^https?:\/\/[^\s/?#]+/i.test(raw.basemap)) {
        errors.push("basemap: must be an http(s) URL");
      }
    }
  }

  // An https address of a project in the Portal, nothing else: the panel puts it in a link.
  if (raw.portal !== undefined && (typeof raw.portal !== "string" || !/^https:\/\/[^\s/?#]+(?:\/[^\s?#]*)?\/projects\/[a-z0-9-]+$/i.test(raw.portal))) {
    errors.push("portal: must be https://{host}/projects/{project}");
  }

  if (raw.endpointName !== undefined && typeof raw.endpointName !== "string") {
    errors.push("endpointName: must be a string");
  }

  if (raw.language !== undefined && typeof raw.language !== "string") {
    errors.push("language: must be a string");
  }

  let endpoints: JcEndpoint[] | undefined;
  if (raw.endpoints !== undefined) {
    if (!Array.isArray(raw.endpoints) || raw.endpoints.length === 0 || raw.endpoints.length > 5) {
      errors.push("endpoints: must be a list of 1 to 5 endpoints");
    } else {
      endpoints = [];
      raw.endpoints.forEach((item: unknown, i: number) => {
        const e = (typeof item === "object" && item !== null ? item : {}) as Record<string, unknown>;
        if (typeof e.name !== "string" || e.name.trim() === "") {
          errors.push(`endpoints[${i}].name: must be a non-empty string`);
        }
        if (typeof e.slug !== "string" || !SLUG_RE.test(e.slug)) {
          errors.push(`endpoints[${i}].slug: must match ^[a-z0-9]{1,64}$`);
        }
        if (typeof e.space !== "string" || /[:/]/.test(e.space)) {
          errors.push(`endpoints[${i}].space: must be a string without ':' or '/'`);
        }
        if (!Array.isArray(e.types) || !e.types.every((t) => typeof t === "string")) {
          errors.push(`endpoints[${i}].types: must be an array of strings`);
        }
        endpoints?.push(e as unknown as JcEndpoint);
      });
    }
  }

  let user: JcUser | null | undefined = undefined;
  if (raw.user !== undefined && raw.user !== null) {
    if (typeof raw.user !== "object" || Array.isArray(raw.user)) {
      errors.push("user: must be an object or null");
    } else {
      const u = raw.user as Record<string, unknown>;
      if (typeof u.id !== "string" || u.id === "") {
        errors.push("user.id: must be a non-empty string");
      }
      if (u.name !== undefined && typeof u.name !== "string") {
        errors.push("user.name: must be a string");
      }
      if (u.email !== undefined && typeof u.email !== "string") {
        errors.push("user.email: must be a string");
      }
      if (u.roles !== undefined && (!Array.isArray(u.roles) || !u.roles.every((r) => typeof r === "string"))) {
        errors.push("user.roles: must be an array of strings");
      }
      user = raw.user as JcUser;
    }
  } else if (raw.user === null) {
    user = null;
  }

  if (errors.length > 0) {
    throw new ConfigError(errors.join(", "));
  }

  const slug = raw.slug as string;
  return {
    slug,
    orgDomain: raw.orgDomain as string,
    space: raw.space as string,
    transport: raw.transport as "origin" | "bridge",
    appName: raw.appName as string | undefined,
    endpointName: (typeof raw.endpointName === "string" && raw.endpointName.trim() !== "" ? raw.endpointName : slug),
    basemap: raw.basemap as string | undefined,
    language: typeof raw.language === "string" && raw.language !== "" ? raw.language : "en",
    user,
    ...(endpoints ? { endpoints } : {}),
    ...(typeof raw.portal === "string" ? { portal: raw.portal } : {}),
  };
}

/**
 * The language a visitor asked for (T-4199): `?lang=` on the address, then the browser's own
 * languages, then the page's `lang`; the primary subtag, lower case, or undefined. The static host
 * writes no language into `#jc-config`, so without this every App answered a Slovak visit in its
 * English fallback.
 */
export function visitorLanguage(win?: { location?: { search?: string }; navigator?: { languages?: readonly string[]; language?: string }; document?: Document }): string | undefined {
  const w = win ?? (typeof window !== "undefined" ? window : undefined);
  if (!w) return undefined;
  const tag = (value: unknown) => (typeof value === "string" && /^[A-Za-z]{2,3}(-|$)/.test(value) ? value.split("-")[0].toLowerCase() : undefined);
  // A plain match, not URLSearchParams: this file is in the server bundle, and QuickJS has none.
  const asked = tag(/[?&]lang=([^&#]*)/.exec(w.location?.search ?? "")?.[1]);
  if (asked) return asked;
  for (const language of w.navigator?.languages ?? [w.navigator?.language]) {
    const found = tag(language);
    if (found) return found;
  }
  return tag(w.document?.documentElement?.lang);
}

export function readConfig(doc?: Document): JcConfig {
  const d = doc ?? (typeof document !== "undefined" ? document : undefined);
  if (!d) {
    throw new ConfigError("Document is not available");
  }
  const element = d.getElementById("jc-config");
  if (!element || !element.textContent?.trim()) {
    throw new ConfigError("Missing or empty #jc-config element");
  }
  let parsed: unknown;
  try {
    parsed = JSON.parse(element.textContent);
  } catch (err) {
    throw new ConfigError(`Invalid JSON in #jc-config: ${err instanceof Error ? err.message : String(err)}`);
  }
  const config = parseConfig(parsed);
  // `?lang=` is the visitor's own choice and wins; the browser's languages only fill a config that
  // names none (T-4199).
  const configured = typeof (parsed as Record<string, unknown>).language === "string" && (parsed as Record<string, unknown>).language !== "";
  const view = d.defaultView ?? undefined;
  const asked = visitorLanguage(view ? { location: view.location } : undefined);
  const visitor = visitorLanguage(view ?? undefined);
  const language = asked ?? (configured ? config.language : visitor);
  return language ? { ...config, language } : config;
}
