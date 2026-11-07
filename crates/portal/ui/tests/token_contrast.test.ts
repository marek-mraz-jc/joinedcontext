/**
 * T-1493, UI-16, UI-30: every tone a person reads text in clears 4.5:1, whatever an installation
 * brands the Portal with.
 *
 * axe on dev found `color-contrast` on the info chip, and axe on dev can only ever measure the one
 * brand dev is running. The tones are `color-mix` derivations of the four colours an installation
 * configures, so the question is not "does the chip pass today" but "does it pass for the colour
 * the next city sets". This resolves `src/tokens.css` the way a browser resolves it and measures.
 */
import { describe, expect, it } from "vitest";
import { blocksOf, colourOf, contrast, mixOklab, ratioOf, round } from "./tokenContrast";
import type { Brand } from "./tokenContrast";
import { NEUTRAL_BRANDING } from "../src/branding";

/** What the API answers when no branding is configured, which is also `tokens.css`'s own default. */
const DEFAULT: Brand = {
  primary: NEUTRAL_BRANDING.colours!.primary!,
  primaryForeground: NEUTRAL_BRANDING.primaryForeground!,
  primaryDark: NEUTRAL_BRANDING.primaryDark!,
  primaryForegroundDark: NEUTRAL_BRANDING.primaryForegroundDark!,
  secondary: NEUTRAL_BRANDING.colours!.secondary!,
  accent: NEUTRAL_BRANDING.colours!.accent!,
  background: NEUTRAL_BRANDING.colours!.background!,
  text: NEUTRAL_BRANDING.colours!.text!,
};

/**
 * Three more installations, each with the pair the API computes for it —
 * `branding.rs::the_dark_primary_is_the_oklab_mix_the_stylesheet_used_to_compute` is where those
 * four values are asserted against this file's own arithmetic, so a brand here answers what a
 * running Portal would answer for it.
 */
/** A city that brands the Portal in a pale civic colour, which is what broke the info chip. */
const PALE: Brand = {
  ...DEFAULT,
  primary: "#7dd3fc",
  secondary: "#7dd3fc",
  accent: "#fde68a",
  primaryForeground: "#0f172a",
  primaryDark: "#a5e0fd",
  primaryForegroundDark: "#0f172a",
};
/** One that brands it nearly black, and one in a saturated red-orange. */
const INK: Brand = {
  ...DEFAULT,
  primary: "#111827",
  secondary: "#1f2937",
  accent: "#374151",
  primaryDark: "#4a505d",
  // White, where the dark theme's own rule wrote near-black and left the label at 2.31:1.
  primaryForegroundDark: "#ffffff",
};
const HOT: Brand = {
  ...DEFAULT,
  primary: "#dc2626",
  secondary: "#ea580c",
  accent: "#f97316",
  primaryDark: "#ee7266",
  primaryForegroundDark: "#0f172a",
};

const BRANDS: [string, Brand][] = [
  ["the neutral default", DEFAULT],
  ["a pale civic brand", PALE],
  ["a near-black brand", INK],
  ["a saturated brand", HOT],
];

/**
 * Every pair a component puts text on, as the components write them: `text-fg-muted` on
 * `bg-surface`, `text-info` on `bg-info-soft`, and so on.
 */
const TEXT_ON: [string, string][] = [
  ["--portal-fg", "--portal-bg"],
  ["--portal-fg", "--portal-surface"],
  ["--portal-fg", "--portal-surface-raised"],
  ["--portal-fg-muted", "--portal-surface"],
  ["--portal-fg-muted", "--portal-surface-subtle"],
  ["--portal-fg-muted", "--portal-surface-muted"],
  ["--portal-fg-subtle", "--portal-surface"],
  ["--portal-fg-subtle", "--portal-bg"],
  ["--portal-danger", "--portal-danger-soft"],
  ["--portal-success", "--portal-success-soft"],
  ["--portal-warning", "--portal-warning-soft"],
  ["--portal-info", "--portal-info-soft"],
  ["--portal-primary-soft-fg", "--portal-primary-soft"],
  ["--portal-primary-fg", "--portal-primary"],
  ["--portal-danger-fg", "--portal-danger"],
  // The link tone, on every paper a link sits on. It replaced `text-primary` — the brand raw,
  // 1.67:1 for a pale civic brand — in the twenty-three files that wrote it (T-2323, UI-30).
  ["--portal-primary-soft-fg", "--portal-surface"],
  ["--portal-primary-soft-fg", "--portal-bg"],
  ["--portal-primary-soft-fg", "--portal-surface-raised"],
  ["--portal-primary-soft-fg", "--portal-surface-muted"],
];

/**
 * What is seen rather than read: the focus ring, and the outline of a selected node. WCAG 2.2
 * asks 3:1 of a focus indicator and of any graphic a person has to make out (1.4.11, 2.4.11),
 * not the 4.5:1 of text — and the ring has to clear it against every paper a control sits on,
 * because the ring is drawn outside the control, on whatever is behind it.
 */
const SEEN_ON: [string, string][] = [
  ["--portal-ring", "--portal-bg"],
  ["--portal-ring", "--portal-surface"],
  ["--portal-ring", "--portal-surface-raised"],
  ["--portal-ring", "--portal-surface-muted"],
];

/**
 * Pairs another task owns, named one by one so nothing is quietly excluded and the entry has to
 * be deleted the day it is fixed: `knows_its_own_gaps` below fails if one of these starts passing.
 *
 * Empty. The one entry it held was the dark theme's label on a near-black brand, and T-2324
 * closed it by letting the API choose that label against the colour the dark theme paints.
 */
const KNOWN_GAPS = new Map<string, string>();

const { light, dark } = blocksOf();
const THEMES: [string, Record<string, string>][] = [
  ["light", light],
  // The dark block redefines the inputs and lets the scales above re-derive themselves.
  ["dark", { ...light, ...dark }],
];

describe("what a person can read", () => {
  it.each(BRANDS)("every text tone clears 4.5:1 for %s, in both themes", (name, brand) => {
    const failing: string[] = [];
    for (const [theme, block] of THEMES) {
      for (const [foreground, background] of TEXT_ON) {
        const pair = `${name}|${theme}|${foreground} on ${background}`;
        if (KNOWN_GAPS.has(pair)) continue;
        const ratio = ratioOf(foreground, background, block, brand);
        if (ratio < 4.5) {
          failing.push(`${theme}: ${foreground} on ${background} is ${round(ratio)}:1`);
        }
      }
    }
    expect(failing, "derive the tone from the ink instead of using the brand colour raw").toEqual([]);
  });

  it.each(BRANDS)("the focus ring clears 3:1 for %s, on every paper and in both themes", (_name, brand) => {
    const failing: string[] = [];
    for (const [theme, block] of THEMES) {
      for (const [tone, behind] of SEEN_ON) {
        const ratio = ratioOf(tone, behind, block, brand);
        if (ratio < 3) failing.push(`${theme}: ${tone} on ${behind} is ${round(ratio)}:1`);
      }
    }
    expect(failing, "a keyboard user has nothing else to go by; derive the ring from the ink").toEqual([]);
  });

  it("knows its own gaps: a pair another task owns is still the pair it was", () => {
    const fixed: string[] = [];
    for (const [pair, task] of KNOWN_GAPS) {
      const [name, theme, tones] = pair.split("|");
      const [foreground, background] = tones.split(" on ");
      const brand = BRANDS.find(([label]) => label === name)?.[1];
      const block = THEMES.find(([label]) => label === theme)?.[1];
      expect(brand && block, `${pair} names a brand and a theme this file has`).toBeTruthy();
      if (ratioOf(foreground, background, block!, brand!) >= 4.5) {
        fixed.push(`${pair} passes now: delete the entry, ${task} is done`);
      }
    }
    expect(fixed).toEqual([]);
  });

  it("an answer that predates the dark pair still paints the brand, not another one's blue", () => {
    // `GET /api/v1/branding` is cacheable for five minutes, so a page can hold a body written by
    // a Portal that had no `primaryDark`. The dark theme's two tokens then fall back to the
    // derivation this file used before T-2324 — the brand lightened — and never to a literal.
    const night = { ...light, ...dark };
    const old: Brand = { ...INK, primaryDark: undefined, primaryForegroundDark: undefined };
    expect(colourOf("--portal-primary", night, old)).toEqual(colourOf("--portal-primary", night, INK));
    // And it is the old label with it, which is the pair that shipped: unreadable, but nobody's
    // else's colour. The Portal answering the new fields is what fixes the ratio.
    expect(round(ratioOf("--portal-primary-fg", "--portal-primary", night, old))).toBeLessThan(4.5);
    expect(round(ratioOf("--portal-primary-fg", "--portal-primary", night, INK))).toBeGreaterThan(4.5);
  });

  it("the info chip is readable for a pale brand, which is the finding it was", () => {
    // 1.57:1 before: the chip was `--portal-color-secondary` on a 12 % tint of itself.
    expect(round(ratioOf("--portal-info", "--portal-info-soft", light, PALE))).toBeGreaterThan(4.5);
    expect(round(ratioOf("--portal-info", "--portal-info-soft", light, DEFAULT))).toBeGreaterThan(4.5);
  });

  it("the primary chip is readable in the dark theme, which the same measurement found", () => {
    // 1.58:1 before: `primary-300` on `primary-100`, two steps of one scale over a dark paper.
    const night = { ...light, ...dark };
    for (const [, brand] of BRANDS) {
      expect(round(ratioOf("--portal-primary-soft-fg", "--portal-primary-soft", night, brand))).toBeGreaterThan(4.5);
    }
  });
});

describe("the resolver itself", () => {
  it("mixes in oklab, not in sRGB", () => {
    // Grey is where the two spaces disagree most: sRGB's midpoint of black and white is #808080,
    // oklab's is lighter, and the whole point of the file is to compute what the browser computes.
    // sRGB's own midpoint of black and white is #808080; oklab's perceptual half is darker.
    expect(mixOklab([0, 0, 0], [255, 255, 255], 0.5)).toEqual([99, 99, 99]);
    expect(mixOklab([255, 255, 255], [0, 0, 0], 1)).toEqual([255, 255, 255]);
  });

  it("reads the brand through the token chain, not the stylesheet's default", () => {
    // `applyBranding` writes the brand as an inline property, which beats the rule in the file.
    expect(colourOf("--portal-brand", light, { ...DEFAULT, primary: "#ff0000" })).toEqual([255, 0, 0]);
    expect(colourOf("--portal-paper", light, DEFAULT)).toEqual([255, 255, 255]);
  });

  it("computes the ratio WCAG computes", () => {
    expect(round(contrast([0, 0, 0], [255, 255, 255]))).toBe(21);
    expect(round(contrast([255, 255, 255], [255, 255, 255]))).toBe(1);
  });

  it("refuses a token it cannot resolve rather than guessing a colour", () => {
    expect(() => colourOf("--portal-not-a-token", light, DEFAULT)).toThrow(/no token/);
    expect(() => colourOf("--portal-overlay", light, DEFAULT)).toThrow(/transparent/);
  });
});
