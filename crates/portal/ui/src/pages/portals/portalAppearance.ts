/**
 * The style members of `spec.appearance` a Portal sets over its Organization's (T-7744): names,
 * colour tokens, fonts, size, width, corners, buttons, cards and the dataset list. Each member left
 * out shows the Organization's, then the installation's. jc-core checks every value again at
 * propose (hex colours, the enums, 14 to 18 px, 0 to 24 px, WCAG AA contrast of each pair).
 */

export const FONTS = ["system", "humanist", "serif", "mono"] as const;
export const WIDTHS = ["narrow", "normal", "wide"] as const;
export const BUTTON_STYLES = ["filled", "outline"] as const;
export const CARD_SHADOWS = ["none", "soft"] as const;
export const DATASET_LISTS = ["cards", "table"] as const;

/** One editable member: where it sits in `spec.appearance` and what kind of value it takes. */
export interface StyleField {
  /** The member's path under `spec.appearance`, e.g. `colours.link`. */
  path: string;
  kind: "text" | "email" | "licence" | "colour" | "choice" | "number";
  /** The values of a `choice`. */
  choices?: readonly string[];
  /** The range of a `number`. */
  min?: number;
  max?: number;
}

export interface StyleGroup {
  key: "names" | "colours" | "type";
  fields: StyleField[];
}

const colour = (path: string): StyleField => ({ path: `colours.${path}`, kind: "colour" });

export const STYLE_GROUPS: StyleGroup[] = [
  {
    key: "names",
    fields: [
      { path: "instanceName", kind: "text" },
      { path: "shortName", kind: "text" },
      { path: "city", kind: "text" },
      { path: "publisher", kind: "text" },
      { path: "contactEmail", kind: "email" },
      { path: "licenseDefault", kind: "licence" },
    ],
  },
  {
    key: "colours",
    fields: [
      colour("primary"),
      colour("secondary"),
      colour("accent"),
      colour("background"),
      colour("text"),
      colour("link"),
      colour("headerBackground"),
      colour("headerText"),
      colour("footerBackground"),
      colour("footerText"),
    ],
  },
  {
    key: "type",
    fields: [
      { path: "headingFont", kind: "choice", choices: FONTS },
      { path: "bodyFont", kind: "choice", choices: FONTS },
      { path: "fontSize", kind: "number", min: 14, max: 18 },
      { path: "width", kind: "choice", choices: WIDTHS },
      { path: "radius", kind: "number", min: 0, max: 24 },
      { path: "buttonStyle", kind: "choice", choices: BUTTON_STYLES },
      { path: "cardShadow", kind: "choice", choices: CARD_SHADOWS },
      { path: "datasetList", kind: "choice", choices: DATASET_LISTS },
    ],
  },
];

export type Appearance = Record<string, unknown>;

/** The member at `path` (`colours.link`) of an appearance, `undefined` when unset. */
export function valueAt(appearance: Appearance | undefined, path: string): string | number | undefined {
  let at: unknown = appearance;
  for (const part of path.split(".")) at = (at as Record<string, unknown> | undefined)?.[part];
  return typeof at === "string" || typeof at === "number" ? at : undefined;
}

/** `appearance` with the member at `path` set, or removed (with an emptied parent) for `undefined`. */
export function withValue(appearance: Appearance, path: string, value: string | number | undefined): Appearance {
  const [head, tail] = path.split(".", 2) as [string, string | undefined];
  const next = { ...appearance };
  if (tail === undefined) {
    if (value === undefined || value === "") delete next[head];
    else next[head] = value;
    return next;
  }
  const inner = withValue({ ...((appearance[head] as Appearance | undefined) ?? {}) }, tail, value);
  if (Object.keys(inner).length === 0) delete next[head];
  else next[head] = inner;
  return next;
}

/** A member's legacy fallback: `font` sets both heading and body fonts until each is set alone. */
export function inherited(appearance: Appearance | undefined, path: string): string | number | undefined {
  const own = valueAt(appearance, path);
  if (own !== undefined) return own;
  if (path === "headingFont" || path === "bodyFont") return valueAt(appearance, "font");
  return undefined;
}

/** Where a member shows from: this portal, its Organization, or the installation's default. */
export function sourceOf(own: Appearance | undefined, organization: Appearance | undefined, path: string): "portal" | "organization" | "builtIn" {
  if (inherited(own, path) !== undefined) return "portal";
  return inherited(organization, path) !== undefined ? "organization" : "builtIn";
}

const HEX = /^#([0-9a-fA-F]{3}|[0-9a-fA-F]{6})$/;
const LICENCE = /^[A-Za-z0-9.+-]{1,64}$/;
const EMAIL = /^[^\s@<>"']+@[^\s@<>"']+\.[^\s@<>"']+$/;

/** The members the editor refuses before sending, by path, as locale keys. */
export function styleFaults(appearance: Appearance): Record<string, string> {
  const found: Record<string, string> = {};
  for (const group of STYLE_GROUPS) {
    for (const field of group.fields) {
      const value = valueAt(appearance, field.path);
      if (value === undefined) continue;
      const text = String(value);
      const bad =
        field.kind === "colour"
          ? !HEX.test(text)
          : field.kind === "number"
            ? !Number.isInteger(value) || (value as number) < (field.min ?? 0) || (value as number) > (field.max ?? 0)
            : field.kind === "choice"
              ? !(field.choices ?? []).includes(text)
              : field.kind === "email"
                ? !EMAIL.test(text) || text.length > 254
                : field.kind === "licence"
                  ? !LICENCE.test(text)
                  : text.trim() === "" || text.length > 120 || /[<>]/.test(text);
      if (bad) found[field.path] = `portals.appearance.fault.${field.kind}`;
    }
  }
  for (const [fore, back] of CONTRAST_PAIRS) {
    const [f, b] = [valueAt(appearance, fore), valueAt(appearance, back)];
    if (found[fore] || typeof f !== "string" || typeof b !== "string") continue;
    const ratio = contrast(f, b);
    if (ratio !== undefined && ratio < 4.5) found[fore] = "portals.appearance.fault.contrast";
  }
  return found;
}

/** WCAG 2.1 contrast of two hex colours, 1 to 21. */
export function contrast(a: string, b: string): number | undefined {
  const lum = (hex: string) => {
    if (!HEX.test(hex)) return undefined;
    const full = hex.length === 4 ? hex.slice(1).replace(/./g, (c) => c + c) : hex.slice(1);
    const [r, g, bl] = [0, 2, 4].map((i) => {
      const c = parseInt(full.slice(i, i + 2), 16) / 255;
      return c <= 0.03928 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4;
    }) as [number, number, number];
    return 0.2126 * r + 0.7152 * g + 0.0722 * bl;
  };
  const [x, y] = [lum(a), lum(b)];
  if (x === undefined || y === undefined) return undefined;
  return (Math.max(x, y) + 0.05) / (Math.min(x, y) + 0.05);
}

/** The colour pairs jc-core holds to WCAG AA (4.5:1) when both are set, foreground first. */
export const CONTRAST_PAIRS: [string, string][] = [
  ["colours.text", "colours.background"],
  ["colours.primary", "colours.background"],
  ["colours.link", "colours.background"],
  ["colours.headerText", "colours.headerBackground"],
  ["colours.footerText", "colours.footerBackground"],
];
