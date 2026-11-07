/**
 * Light and dark (T-3331, as alerts-heatmap): the SDK sets the tokens it is given and has no dark
 * variant, so the App picks the set the reader's system asks for and applies the other when the
 * system switches. The dark colours keep WCAG AA contrast on their surfaces.
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
  map: { point: "#2dd4bf", selected: "#93c5fd", low: "#38bdf8", high: "#fb923c", stroke: "#0f172a" },
};

const QUERY = "(prefers-color-scheme: dark)";

export function prefersDark(): boolean {
  return typeof window !== "undefined" && typeof window.matchMedia === "function" && window.matchMedia(QUERY).matches;
}

/**
 * The map's own colours: the bands near to far, the stops, the starting point, the outlines. The
 * SDK keeps only the token keys it knows, so these live here, per scheme.
 */
export const REACH = {
  light: { near: "#0f766e", mid: "#0284c7", far: "#7c3aed", stop: "#1e293b", origin: "#dc2626", stroke: "#ffffff" },
  dark: { near: "#2dd4bf", mid: "#38bdf8", far: "#c4b5fd", stop: "#f1f5f9", origin: "#fca5a5", stroke: "#0f172a" },
};

/** The map's colours for the scheme in force. */
export function reachColours(): typeof REACH.light {
  return prefersDark() ? REACH.dark : REACH.light;
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
