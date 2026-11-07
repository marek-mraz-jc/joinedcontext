/**
 * Light and dark (T-3332, as alerts-heatmap): the SDK sets the tokens it is given and has no dark variant, so the App
 * picks the set the reader's system asks for and applies the other when the system switches.
 * The dark colours keep WCAG AA contrast on their surfaces.
 */
import { useEffect, useState } from "react";
import { applyTokens } from "@joinedcontext/sdk";
import light from "./design-tokens.json";

export const LIGHT = light;
export const DARK = {
  ...light,
  color: {
    accent: "#93c5fd",
    ink: "#f1f5f9",
    muted: "#a3b1c6",
    surface: "#0f172a",
    card: "#1e293b",
    line: "#334155",
    danger: "#fca5a5",
    success: "#6ee7b7",
    warning: "#fcd34d",
  },
};

const QUERY = "(prefers-color-scheme: dark)";

export function prefersDark(): boolean {
  return typeof window !== "undefined" && typeof window.matchMedia === "function" && window.matchMedia(QUERY).matches;
}

/**
 * The chart's own colours: measured, model, forecast, odd readings. The SDK keeps only the token
 * keys it knows, so these live here, per scheme.
 */
export const SERIES = {
  light: { history: "#0f766e", fitted: "#64748b", forecast: "#2563eb", anomaly: "#dc2626" },
  dark: { history: "#2dd4bf", fitted: "#a3b1c6", forecast: "#93c5fd", anomaly: "#fca5a5" },
};

/** The chart's colours for the scheme in force. */
export function seriesColours(): typeof SERIES.light {
  return prefersDark() ? SERIES.dark : SERIES.light;
}

/** The tokens to start with. */
export function startTokens(): typeof LIGHT {
  return prefersDark() ? DARK : LIGHT;
}

/** The scheme in force, kept in step with the system; a change applies the other tokens. */
export function useScheme(): "light" | "dark" {
  const [dark, setDark] = useState(prefersDark);
  useEffect(() => {
    if (typeof window.matchMedia !== "function") return;
    const media = window.matchMedia(QUERY);
    const changed = () => {
      applyTokens(media.matches ? DARK : LIGHT);
      setDark(media.matches);
    };
    media.addEventListener("change", changed);
    return () => media.removeEventListener("change", changed);
  }, []);
  return dark ? "dark" : "light";
}
