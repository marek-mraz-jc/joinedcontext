import { useClient } from "@joinedcontext/sdk";

/** The languages the page is written in; the data keeps its own (fi, sv, en in the city's feeds). */
export type Lang = "fi" | "en" | "sv";

export const LANGS: readonly Lang[] = ["fi", "en", "sv"];

/** Every date and time the page shows is Helsinki's, whatever the reader's clock says. */
export const ZONE = "Europe/Helsinki";

/** The page's language: `?lang=` when it names one, else the Portal's, else the browser's, else Finnish. */
export function langOf(search: string, configured?: string, browser?: string): Lang {
  const asked = new URLSearchParams(search).get("lang");
  for (const candidate of [asked, configured, browser?.slice(0, 2)]) {
    if (LANGS.includes(candidate as Lang)) return candidate as Lang;
  }
  return "fi";
}

/** The locale dates are written in: Finnish, Swedish as Finland writes it, or English as Finland writes it (day before month). */
export function localeOf(lang: Lang): string {
  return `${lang}-FI`;
}

/**
 * A number as Finland writes it, `1 234,5`, in either language: `digits` after the decimal comma.
 * A value that rounds to zero is written `0`, never `−0`.
 */
export function number(value: number, digits = 0): string {
  const shown = Number(value.toFixed(digits)) === 0 ? 0 : value;
  return new Intl.NumberFormat("fi-FI", { maximumFractionDigits: digits, minimumFractionDigits: digits }).format(shown);
}

/** The page's language from the address and the Portal's configuration. */
export function useLang(): Lang {
  const client = useClient();
  const search = typeof window === "undefined" ? "" : window.location.search;
  const browser = typeof navigator === "undefined" ? undefined : navigator.language;
  return langOf(search, client.config.language, browser);
}
