/** The design tokens with the dark colours when the reader's system asks for a dark page. */
const DARK = {
  accent: "#8fa8ff",
  ink: "#e5e7eb",
  muted: "#9ca3af",
  surface: "#111318",
  card: "#1b1e26",
  line: "#2e3340",
  danger: "#f87171",
  success: "#34d399",
  warning: "#fbbf24",
};

export function themed<T extends { color?: Record<string, string> }>(tokens: T, dark: boolean): T {
  return dark ? { ...tokens, color: { ...tokens.color, ...DARK } } : tokens;
}

/** Whether the reader's system asks for a dark page. */
export function prefersDark(): boolean {
  return typeof window !== "undefined" && typeof window.matchMedia === "function" && window.matchMedia("(prefers-color-scheme: dark)").matches;
}
