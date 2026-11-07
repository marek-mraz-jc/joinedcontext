import { createContext, useContext, useEffect, useMemo } from "react";
import type { ReactNode } from "react";
import { useQuery } from "@tanstack/react-query";
import i18n from "./i18n";
import { api, unwrap } from "./api/client";
import { setInstanceName } from "./documentTitle";
import type { components } from "./api/schema";

export type Branding = components["schemas"]["Branding"];

/**
 * What an installation without a branding ConfigMap looks like. It mirrors the API's own
 * defaults so the first paint is never wrong: the answer only ever changes the values, never
 * the shape (UI-30).
 */
export const NEUTRAL_BRANDING: Branding = {
  instanceName: "joinedcontext",
  shortName: "joinedcontext",
  city: "",
  organisation: "",
  orgDomain: "",
  domain: "",
  contactEmail: "",
  licenseDefault: "CC-BY-4.0",
  logo: "",
  favicon: "",
  colours: {
    primary: "#1d4ed8",
    secondary: "#0f766e",
    accent: "#f59e0b",
    background: "#ffffff",
    text: "#0f172a",
  },
  fonts: { heading: "system-ui, sans-serif", body: "system-ui, sans-serif" },
  languages: { default: "en", offered: ["en"] },
  primaryForeground: "#ffffff",
  primaryDark: "#5985e7",
  primaryForegroundDark: "#0f172a",
  validation: "strict",
  // An installation that serves no User Guide; no form offers a link (UI-02).
  documentationBaseUrl: "",
  appsOrigin: null,
  hiddenSections: [],
};

const BrandingContext = createContext<Branding>(NEUTRAL_BRANDING);

/** The branding of this installation, applied and provided by {@link BrandingProvider}. */
export function useBranding(): Branding {
  return useContext(BrandingContext);
}

/**
 * The address of one guide page, when this installation serves the guide and the kind's
 * arrangement names a page (UI-02, DP-11). Nothing otherwise, so a form that cannot build a
 * live link offers none.
 *
 * Both halves are already checked where they enter: the base URL is served only when it is an
 * absolute `http(s)` address (the API's `branding.rs`), and the path only when it stays inside
 * the site (`forms/uischema.ts`). This joins them and does nothing else.
 */
export function guideUrl(branding: Branding, guide: string | undefined): string | undefined {
  const base = branding.documentationBaseUrl?.trim().replace(/\/+$/, "");
  if (!base || !guide) {
    return undefined;
  }
  return `${base}/${guide}`;
}

/** `/api/v1/branding/logo` when a logo is configured, nothing otherwise. */
export function logoUrl(branding: Branding): string | undefined {
  return branding.logo ? "/api/v1/branding/logo" : undefined;
}

/** The families a font stack may end in that the browser resolves on its own. */
const GENERIC_FAMILIES = new Set(["system-ui", "-apple-system", "blinkmacsystemfont", "sans-serif", "serif", "monospace", "ui-sans-serif", "ui-serif"]);

/**
 * A branding font stack with the Portal's bundled Inter before its first generic family, so a
 * brand font the browser does not have falls back to the same letters everywhere (T-0756).
 */
/** `#rgb` or `#rrggbb`, which is the whole of what may reach a colour token (PF-50). */
export function isHexColour(value: string): boolean {
  return /^#(?:[0-9a-fA-F]{3}|[0-9a-fA-F]{6})$/.test(value);
}

/**
 * A font stack that is only family names (PF-50).
 *
 * A custom property accepts almost any token stream, so `url(https://elsewhere/x)` written into
 * `--portal-font-sans` is fetched the moment something reads it — a way to learn that this Portal
 * was opened, from a value an administrator typed into a branding file. Only letters, digits,
 * spaces, hyphens, underscores, quotes and the commas between families pass.
 */
export function isFontStack(value: string): boolean {
  return value.length <= 200 && /^[\w\s,'"-]+$/.test(value) && !/url|\(|\)|;|\/|@/i.test(value);
}

export function withBundledFont(stack: string): string {
  const families = stack.split(",").map((family) => family.trim()).filter(Boolean);
  if (families.some((family) => family.replace(/["']/g, "").toLowerCase() === "inter")) {
    return families.join(", ");
  }
  const generic = families.findIndex((family) => GENERIC_FAMILIES.has(family.replace(/["']/g, "").toLowerCase()));
  const at = generic === -1 ? families.length : generic;
  return [...families.slice(0, at), '"Inter"', ...families.slice(at)].join(", ");
}

/**
 * Writes the branding into the document: the title, and the colour and font tokens every
 * component already reads through Tailwind's theme.
 *
 * The values are custom properties on the root element, and a browser evaluates what lands
 * here, so each one is checked again before it is written (PF-50). The API validates the same
 * things; this is the side that holds even when the answer did not come from it — a cached
 * body, a proxy, or a branding file an administrator edited by hand. A value that is not a
 * colour or a font stack is dropped with a word in the console, and the token keeps the
 * default it already had rather than taking something unreadable.
 */
export function applyBranding(branding: Branding, doc: Document = document): void {
  setInstanceName(branding.instanceName, doc);
  const root = doc.documentElement;
  const colours: Record<string, string | undefined> = {
    "--portal-color-primary": branding.colours?.primary,
    "--portal-color-primary-fg": branding.primaryForeground,
    // The dark theme's pair. Both are computed by the API against the colour a dark page
    // actually paints, because a label chosen for the configured colour is not readable on the
    // lightened one — a near-black brand left it at 2.31:1 (T-2324, UI-30).
    "--portal-color-primary-dark": branding.primaryDark,
    "--portal-color-primary-fg-dark": branding.primaryForegroundDark,
    "--portal-color-secondary": branding.colours?.secondary,
    "--portal-color-accent": branding.colours?.accent,
    "--portal-color-surface": branding.colours?.background,
    "--portal-color-surface-fg": branding.colours?.text,
  };
  const fonts: Record<string, string | undefined> = {
    "--portal-font-sans": branding.fonts?.body,
    "--portal-font-heading": branding.fonts?.heading,
  };
  const write = (token: string, value: string | undefined, ok: (v: string) => boolean) => {
    if (!value) return;
    if (!ok(value)) {
      // Named, not shown: the value is what was wrong with it, and it goes nowhere near a style.
      console.warn(`branding: ${token} is not a value this token takes; keeping the default`);
      return;
    }
    root.style.setProperty(token, token.startsWith("--portal-font") ? withBundledFont(value) : value);
  };
  for (const [token, value] of Object.entries(colours)) write(token, value, isHexColour);
  for (const [token, value] of Object.entries(fonts)) write(token, value, isFontStack);
  const favicon = doc.querySelector<HTMLLinkElement>("link[rel~='icon']");
  if (favicon && branding.favicon) {
    favicon.href = "/api/v1/branding/favicon";
  }
}

/** The locales the switcher offers: what the installation configured, in its own order. */
export function offeredLocales(branding: Branding): string[] {
  const offered = branding.languages?.offered ?? [];
  return offered.length > 0 ? offered : NEUTRAL_BRANDING.languages!.offered!;
}

const brandingQuery = {
  queryKey: ["branding"],
  queryFn: async () => unwrap(await api.GET("/api/v1/branding")),
  staleTime: 5 * 60 * 1000,
};

/**
 * The project sections this installation hides (T-2874), or `undefined` while the answer is on
 * its way, so a page that may be hidden is not drawn before it is known. A branding answer that
 * failed hides nothing: the flag tidies the navigation, it guards nothing.
 */
export function useHiddenSections(): readonly string[] | undefined {
  const { data, isPending } = useQuery(brandingQuery);
  if (isPending) {
    return undefined;
  }
  const hidden = (data as { hiddenSections?: unknown } | undefined)?.hiddenSections;
  return Array.isArray(hidden) ? hidden.filter((section): section is string => typeof section === "string") : [];
}

/**
 * Fetches the branding once, applies it, and hands it to the tree.
 *
 * The endpoint is public, so this runs before the session is known: the login page is
 * branded too. A failure is not an error state, it is the neutral look.
 */
export function BrandingProvider({ children }: { children: ReactNode }): React.JSX.Element {
  const { data } = useQuery(brandingQuery);
  // The answer is merged into the neutral block rather than replacing it: a field the API
  // did not send must not blank out a name or a colour the page needs.
  const branding: Branding = useMemo(() => ({ ...NEUTRAL_BRANDING, ...(data ?? {}) }), [data]);

  useEffect(() => {
    applyBranding(branding);
  }, [branding]);

  // The installation's own default, from the answer and not from the neutral block: merging the
  // two made the first render apply `NEUTRAL_BRANDING`'s `en` before the API had said anything,
  // and `changeLanguage` writes `jc-lang` on its way through, so by the time the real default
  // arrived there was a "chosen" language in the way and an installation that speaks German
  // opened in English for good (T-1801).
  const configured = data?.languages?.default;

  useEffect(() => {
    // The installation's default locale is a starting point, not a preference: a visitor who
    // has already chosen a language keeps it.
    const chosen =
      typeof window !== "undefined" ? window.localStorage.getItem("jc-lang") : null;
    if (!chosen && configured && i18n.language !== configured) {
      void i18n.changeLanguage(configured);
    }
  }, [configured]);

  return <BrandingContext.Provider value={branding}>{children}</BrandingContext.Provider>;
}
