/**
 * The colour tokens of `src/tokens.css`, resolved the way a browser resolves them, so a contrast
 * ratio can be measured without one (UI-16, UI-30).
 *
 * Every tone of the Portal is derived from the four colours an installation configures, with
 * `color-mix(in oklab, …)`. That means a tone is only as readable as the brand it came from: the
 * info chip was the brand's secondary colour on a 12 % tint of itself, which is 4.6:1 for the
 * default teal and 1.6:1 for a pale sky blue (T-1493). Reading the real file and doing the real
 * arithmetic is the only way to see that before an installation does.
 */
import { readFileSync } from "node:fs";
import { join } from "node:path";

export type Rgb = [number, number, number];

/** The four colours `applyBranding` writes, which every token below is derived from. */
export interface Brand {
  primary: string;
  primaryForeground: string;
  /**
   * The dark theme's pair, computed by the API from `primary` (T-2324). Optional, because a
   * body cached from a Portal that predates the two fields carries neither, and the stylesheet
   * has to keep painting the brand when that is what arrives.
   */
  primaryDark?: string;
  primaryForegroundDark?: string;
  secondary: string;
  accent: string;
  background: string;
  text: string;
  danger?: string;
  dangerForeground?: string;
}

const TOKENS = join(__dirname, "..", "src", "tokens.css");

/** The declarations of one block, in the order they are written. */
export type Block = Record<string, string>;

/**
 * The light and the dark block of `tokens.css`.
 *
 * The file is two blocks: `:root` at the top level, and `:root` inside
 * `@media (prefers-color-scheme: dark)`, which redefines the inputs and lets the scales re-derive.
 */
export function blocksOf(css = readFileSync(TOKENS, "utf8")): { light: Block; dark: Block } {
  const declarations = (from: string): Block => {
    const block: Block = {};
    for (const [, name, value] of from.matchAll(/(--[\w-]+)\s*:\s*([^;]+);/g)) {
      block[name] = value.trim();
    }
    return block;
  };
  const darkAt = css.indexOf("@media (prefers-color-scheme: dark)");
  const light = declarations(darkAt === -1 ? css : css.slice(0, darkAt));
  const dark = darkAt === -1 ? {} : declarations(css.slice(darkAt));
  return { light, dark };
}

const NAMED: Record<string, Rgb> = { white: [255, 255, 255], black: [0, 0, 0] };

function hexToRgb(hex: string): Rgb {
  const digits = hex.slice(1);
  const full = digits.length === 3 ? [...digits].map((d) => d + d).join("") : digits;
  return [0, 2, 4].map((at) => Number.parseInt(full.slice(at, at + 2), 16)) as Rgb;
}

/** The arguments of one `…(a, b, c)` call, split on the commas that are not inside a call. */
function argumentsOf(call: string): string[] {
  const inner = call.slice(call.indexOf("(") + 1, call.lastIndexOf(")"));
  const parts: string[] = [];
  let depth = 0;
  let current = "";
  for (const character of inner) {
    if (character === "(") depth += 1;
    if (character === ")") depth -= 1;
    if (character === "," && depth === 0) {
      parts.push(current.trim());
      current = "";
      continue;
    }
    current += character;
  }
  parts.push(current.trim());
  return parts;
}

function srgbToLinear(channel: number): number {
  const c = channel / 255;
  return c <= 0.04045 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4;
}

function linearToSrgb(channel: number): number {
  const c = channel <= 0.0031308 ? channel * 12.92 : 1.055 * channel ** (1 / 2.4) - 0.055;
  return Math.max(0, Math.min(255, Math.round(c * 255)));
}

const TO_LMS = [
  [0.4122214708, 0.5363325363, 0.0514459929],
  [0.2119034982, 0.6806995451, 0.1073969566],
  [0.0883024619, 0.2817188376, 0.6299787005],
];
const LMS_TO_LAB = [
  [0.2104542553, 0.793617785, -0.0040720468],
  [1.9779984951, -2.428592205, 0.4505937099],
  [0.0259040371, 0.7827717662, -0.808675766],
];
const LAB_TO_LMS = [
  [1, 0.3963377774, 0.2158037573],
  [1, -0.1055613458, -0.0638541728],
  [1, -0.0894841775, -1.291485548],
];
const FROM_LMS = [
  [4.0767416621, -3.3077115913, 0.2309699292],
  [-1.2684380046, 2.6097574011, -0.3413193965],
  [-0.0041960863, -0.7034186147, 1.707614701],
];

const apply = (matrix: number[][], v: number[]): number[] =>
  matrix.map((row) => row.reduce((sum, factor, at) => sum + factor * v[at], 0));

function toOklab(rgb: Rgb): number[] {
  const linear = rgb.map(srgbToLinear);
  const lms = apply(TO_LMS, linear).map((v) => Math.cbrt(v));
  return apply(LMS_TO_LAB, lms);
}

function fromOklab(lab: number[]): Rgb {
  const lms = apply(LAB_TO_LMS, lab).map((v) => v ** 3);
  return apply(FROM_LMS, lms).map(linearToSrgb) as Rgb;
}

/** `color-mix(in oklab, a p%, b)`: the browser's own interpolation space for these tokens. */
export function mixOklab(a: Rgb, b: Rgb, share: number): Rgb {
  const [first, second] = [toOklab(a), toOklab(b)];
  return fromOklab(first.map((channel, at) => channel * share + second[at] * (1 - share)));
}

/**
 * One token, resolved to a colour.
 *
 * `var()` follows the block, then the brand; `color-mix(in oklab, …)` is computed. Anything else
 * (a `transparent` mix, a gradient, a length) throws, because a caller asking for its contrast is
 * asking the wrong question.
 */
export function colourOf(name: string, block: Block, brand: Brand, seen: string[] = []): Rgb {
  const branded: Record<string, string | undefined> = {
    "--portal-color-primary": brand.primary,
    "--portal-color-primary-fg": brand.primaryForeground,
    "--portal-color-primary-dark": brand.primaryDark,
    "--portal-color-primary-fg-dark": brand.primaryForegroundDark,
    "--portal-color-secondary": brand.secondary,
    "--portal-color-accent": brand.accent,
    "--portal-color-surface": brand.background,
    "--portal-color-surface-fg": brand.text,
    "--portal-color-danger": brand.danger ?? "#b91c1c",
    "--portal-color-danger-fg": brand.dangerForeground ?? "#ffffff",
  };
  if (seen.includes(name)) {
    throw new Error(`the token ${name} is defined in terms of itself: ${seen.join(" → ")}`);
  }
  // The brand wins over the stylesheet's own defaults, because `applyBranding` writes these
  // eight as inline properties on the root element and an inline property beats a rule.
  const expression = branded[name] ?? block[name];
  if (expression === undefined) {
    throw new Error(`no token ${name} in tokens.css and no branding input of that name`);
  }
  return valueOf(expression, block, brand, [...seen, name]);
}

function valueOf(expression: string, block: Block, brand: Brand, seen: string[]): Rgb {
  const value = expression.trim();
  if (value.startsWith("#")) return hexToRgb(value);
  if (NAMED[value]) return NAMED[value];
  if (value.startsWith("var(")) {
    const [reference, fallback] = argumentsOf(value);
    try {
      return colourOf(reference, block, brand, seen);
    } catch (missing) {
      if (fallback === undefined) throw missing;
      return valueOf(fallback, block, brand, seen);
    }
  }
  if (value.startsWith("color-mix(")) {
    const [space, first, second] = argumentsOf(value);
    if (space.trim() !== "in oklab") {
      throw new Error(`only oklab mixes are resolved here, not ${space}`);
    }
    const share = /(-?[\d.]+)%\s*$/.exec(first);
    if (!share) throw new Error(`the first colour of ${value} carries no percentage`);
    const a = valueOf(first.slice(0, share.index), block, brand, seen);
    const b = valueOf(second, block, brand, seen);
    return mixOklab(a, b, Number(share[1]) / 100);
  }
  throw new Error(`${value} is not a colour this resolver computes`);
}

/** WCAG 2.2 relative luminance. */
function luminance(rgb: Rgb): number {
  const [r, g, b] = rgb.map(srgbToLinear);
  return 0.2126 * r + 0.7152 * g + 0.0722 * b;
}

/** The WCAG contrast ratio between two tokens, as a browser would compute it. */
export function contrast(foreground: Rgb, background: Rgb): number {
  const [first, second] = [luminance(foreground), luminance(background)];
  const [lighter, darker] = first > second ? [first, second] : [second, first];
  return (lighter + 0.05) / (darker + 0.05);
}

/** The ratio of one pair of token names, in one block. */
export function ratioOf(foreground: string, background: string, block: Block, brand: Brand): number {
  return contrast(colourOf(foreground, block, brand), colourOf(background, block, brand));
}

export const round = (ratio: number): number => Math.round(ratio * 100) / 100;
