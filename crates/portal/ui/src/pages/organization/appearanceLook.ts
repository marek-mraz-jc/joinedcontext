/**
 * The open data portal's look as the Appearance editor holds it (T-7413): the theme and layout
 * members of `spec.appearance` (T-7410), the checks jc-core runs on them said again in the
 * browser so the person sees a refusal while typing, and the manifest a publish proposes. The
 * server checks every value again; nothing here is a control.
 */

export const COLOURS = ["primary", "accent", "background", "text"] as const;
export type ColourName = (typeof COLOURS)[number];
export const FONTS = ["system", "humanist", "serif", "mono"] as const;
export type Font = (typeof FONTS)[number];
export type DatasetList = "cards" | "table";
export type Link = { label: string; url: string };

/** The members this editor owns; the others of `spec.appearance` (name, logo, pages) stay as stored. */
export const LOOK_KEYS = ["colours", "font", "radius", "hero", "datasetList", "navLinks", "footerLinks"] as const;

export interface Look {
  colours?: Partial<Record<ColourName | "secondary", string>>;
  font?: Font;
  radius?: number;
  hero?: { title?: string; intro?: string };
  datasetList?: DatasetList;
  navLinks?: Link[];
  footerLinks?: Link[];
}

/** What the form holds: every member as text, empty for "not set". */
export interface LookForm {
  colours: Record<ColourName, string>;
  font: Font | "";
  radius: string;
  heroTitle: string;
  heroIntro: string;
  datasetList: DatasetList | "";
  navLinks: Link[];
  footerLinks: Link[];
}

/** jc-core's `MIN_CONTRAST`: WCAG 2.1 AA for body text. */
export const MIN_CONTRAST = 4.5;
export const MAX_RADIUS = 24;
export const MAX_LINKS = 8;
export const MAX_INTRO_BYTES = 2048;

/** jc-core `AppearanceFont::stack`, the same system stacks, no font host. */
export const FONT_STACKS: Record<Font, string> = {
  system: 'system-ui, -apple-system, "Segoe UI", Roboto, sans-serif',
  humanist: 'Seravek, "Gill Sans Nova", Ubuntu, Calibri, "DejaVu Sans", sans-serif',
  serif: 'Charter, "Bitstream Charter", "Sitka Text", Cambria, Georgia, serif',
  mono: 'ui-monospace, "Cascadia Code", Menlo, Consolas, monospace',
};

const HEX = /^#(?:[0-9a-fA-F]{3}|[0-9a-fA-F]{6})$/;

export function isHexColour(value: string): boolean {
  return HEX.test(value);
}

/** jc-core `contrast_ratio`: WCAG 2.1 contrast of two hex colours, 1 to 21; `null` when either is none. */
export function contrastRatio(a: string, b: string): number | null {
  const luminance = (hex: string): number | null => {
    if (!isHexColour(hex)) return null;
    const digits = hex.length === 4 ? [...hex.slice(1)].map((d) => d + d) : [1, 3, 5].map((i) => hex.slice(i, i + 2));
    const [r, g, b] = digits.map((pair) => {
      const s = parseInt(pair, 16) / 255;
      return s <= 0.03928 ? s / 12.92 : ((s + 0.055) / 1.055) ** 2.4;
    });
    return 0.2126 * r + 0.7152 * g + 0.0722 * b;
  };
  const la = luminance(a);
  const lb = luminance(b);
  if (la === null || lb === null) return null;
  const [hi, lo] = la > lb ? [la, lb] : [lb, la];
  return (hi + 0.05) / (lo + 0.05);
}

/** dataroom `yiq_foreground`: black or white text on a filled `hex`, as the catalogue picks it. */
export function foregroundOn(hex: string): "black" | "white" {
  if (!isHexColour(hex)) return "white";
  const long = hex.length === 4 ? [...hex.slice(1)].map((d) => d + d).join("") : hex.slice(1);
  const [r, g, b] = [0, 2, 4].map((i) => parseInt(long.slice(i, i + 2), 16));
  return Math.floor((r * 299 + g * 587 + b * 114) / 1000) >= 128 ? "black" : "white";
}

/** jc-core `is_link_url`: `http(s)://host…` or a path on the portal, never another scheme or `//host`. */
export function isLinkUrl(url: string): boolean {
  if (url === "" || url.length > 512 || ![...url].every((c) => c > " " && c <= "~" && !"\"'<>\\`".includes(c))) {
    return false;
  }
  const rest = url.startsWith("https://") ? url.slice(8) : url.startsWith("http://") ? url.slice(7) : null;
  if (rest === null) return url.startsWith("/") && !url.startsWith("//");
  const host = rest.split(/[/?#]/)[0];
  return host !== "" && !host.includes("@");
}

export function toForm(look: Look | undefined): LookForm {
  const colours = Object.fromEntries(COLOURS.map((name) => [name, look?.colours?.[name] ?? ""])) as Record<
    ColourName,
    string
  >;
  return {
    colours,
    font: look?.font ?? "",
    radius: look?.radius === undefined ? "" : String(look.radius),
    heroTitle: look?.hero?.title ?? "",
    heroIntro: look?.hero?.intro ?? "",
    datasetList: look?.datasetList ?? "",
    navLinks: look?.navLinks ?? [],
    footerLinks: look?.footerLinks ?? [],
  };
}

/** The form as the members to write; an empty field is no member, so the look under it shows. */
export function fromForm(form: LookForm, stored: Look | undefined): Look {
  const look: Look = {};
  // `secondary` has no field here (the catalogue draws nothing with it): it stays as stored.
  const colours: Look["colours"] = { ...(stored?.colours?.secondary ? { secondary: stored.colours.secondary } : {}) };
  for (const name of COLOURS) if (form.colours[name].trim()) colours[name] = form.colours[name].trim();
  if (Object.keys(colours).length > 0) look.colours = colours;
  if (form.font) look.font = form.font;
  if (form.radius.trim() !== "") look.radius = Number(form.radius);
  const hero = {
    ...(form.heroTitle.trim() ? { title: form.heroTitle.trim() } : {}),
    ...(form.heroIntro.trim() ? { intro: form.heroIntro } : {}),
  };
  if (Object.keys(hero).length > 0) look.hero = hero;
  if (form.datasetList) look.datasetList = form.datasetList;
  const links = (list: Link[]) => list.filter((link) => link.label.trim() || link.url.trim());
  if (links(form.navLinks).length > 0) look.navLinks = links(form.navLinks);
  if (links(form.footerLinks).length > 0) look.footerLinks = links(form.footerLinks);
  return look;
}

/** Whether the stored look sets any member this editor owns: only then is there something to revert. */
export function hasLook(appearance: Record<string, unknown> | undefined): boolean {
  return LOOK_KEYS.some((key) => appearance?.[key] !== undefined);
}

/**
 * The manifest to propose: the stored one with `spec.appearance`'s theme members replaced by
 * `look` (none of them for a revert) and every other member kept; `status` never goes back.
 */
export function withLook(stored: unknown, look: Look): unknown {
  const manifest = { ...((stored ?? {}) as Record<string, unknown>) };
  delete manifest.status;
  const spec = { ...((manifest.spec ?? {}) as Record<string, unknown>) };
  const appearance = { ...((spec.appearance ?? {}) as Record<string, unknown>) };
  for (const key of LOOK_KEYS) delete appearance[key];
  const next = { ...appearance, ...look };
  if (Object.keys(next).length > 0) spec.appearance = next;
  else delete spec.appearance;
  return { ...manifest, spec };
}

/** The refusals jc-core would answer, keyed by field id, as i18n keys with their values. */
export function lookProblems(form: LookForm): { field: string; key: string; values?: Record<string, string | number> }[] {
  const found: { field: string; key: string; values?: Record<string, string | number> }[] = [];
  for (const name of COLOURS) {
    const value = form.colours[name].trim();
    if (value && !isHexColour(value)) found.push({ field: `colour-${name}`, key: "colourInvalid" });
  }
  // jc-core refuses a pair below AA when both members are set; a one-sided colour falls back at render.
  const background = form.colours.background.trim();
  for (const name of ["text", "primary"] as const) {
    const value = form.colours[name].trim();
    if (!value || !background) continue;
    const ratio = contrastRatio(value, background);
    if (ratio !== null && ratio < MIN_CONTRAST) {
      found.push({ field: `colour-${name}`, key: "contrastLow", values: { ratio: ratio.toFixed(2), min: MIN_CONTRAST } });
    }
  }
  if (form.radius.trim() !== "") {
    const radius = Number(form.radius);
    if (!Number.isInteger(radius) || radius < 0 || radius > MAX_RADIUS) {
      found.push({ field: "radius", key: "radiusInvalid", values: { max: MAX_RADIUS } });
    }
  }
  if (new TextEncoder().encode(form.heroIntro).length > MAX_INTRO_BYTES) {
    found.push({ field: "hero-intro", key: "introTooLong", values: { max: MAX_INTRO_BYTES } });
  }
  for (const member of ["navLinks", "footerLinks"] as const) {
    form[member].forEach((link, at) => {
      if (!link.label.trim() && !link.url.trim()) return;
      if (!link.label.trim()) found.push({ field: `${member}-${at}-label`, key: "linkLabelMissing" });
      if (!isLinkUrl(link.url.trim())) found.push({ field: `${member}-${at}-url`, key: "linkUrlInvalid" });
    });
  }
  return found;
}

/** `text` broken at spaces into lines of at most `width` characters, at most `lines` of them, the last cut with "…". */
export function wrapWords(text: string, width: number, lines: number): string[] {
  const out: string[] = [];
  for (const word of text.split(/\s+/).filter(Boolean)) {
    const last = out.at(-1);
    if (last !== undefined && (last + " " + word).length <= width) out[out.length - 1] = `${last} ${word}`;
    else out.push(word.length > width ? `${word.slice(0, width - 1)}…` : word);
  }
  if (out.length <= lines) return out;
  const kept = out.slice(0, lines);
  kept[lines - 1] = `${kept[lines - 1].slice(0, width - 1).trimEnd()}…`;
  return kept;
}
