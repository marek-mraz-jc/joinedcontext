import defaultTokens from "./tokens.json";

export interface DesignTokens {
  color: {
    accent: string;
    ink: string;
    muted: string;
    surface: string;
    card: string;
    line: string;
    danger: string;
    success: string;
    warning: string;
  };
  font: {
    body: string;
    mono: string;
    size: { sm: string; md: string; lg: string; xl: string };
  };
  space: Record<"1" | "2" | "3" | "4" | "6" | "8", string>;
  radius: { sm: string; md: string; lg: string };
  chart: { palette: string[] };
  map: { point: string; selected: string; low: string; high: string; stroke: string };
}

export const DEFAULT_TOKENS: DesignTokens = defaultTokens as DesignTokens;

let appliedTokens: DesignTokens = DEFAULT_TOKENS;

function isSafeCss(val: unknown): val is string {
  return typeof val === "string" && !/[;{}]|url\(/i.test(val);
}

/** Families a stack may end in that the browser resolves to whatever the system has. */
const GENERIC_FAMILIES = new Set(["system-ui", "-apple-system", "blinkmacsystemfont", "sans-serif", "serif", "monospace", "ui-sans-serif", "ui-serif"]);

/**
 * A body font stack with the bundled Inter before its first generic family, so an app that names
 * a font the viewer does not have still draws the same letters as the Portal around it (T-0756).
 */
export function withBundledFont(stack: string): string {
  const families = stack.split(",").map((family) => family.trim()).filter(Boolean);
  const bare = (family: string): string => family.replace(/["']/g, "").toLowerCase();
  if (families.some((family) => bare(family) === "inter")) {
    return families.join(", ");
  }
  const generic = families.findIndex((family) => GENERIC_FAMILIES.has(bare(family)));
  const at = generic === -1 ? families.length : generic;
  return [...families.slice(0, at), '"Inter"', ...families.slice(at)].join(", ");
}

function cloneTokens(t: DesignTokens): DesignTokens {
  return JSON.parse(JSON.stringify(t)) as DesignTokens;
}

export function resolveTokens(input: unknown): { tokens: DesignTokens; warnings: string[] } {
  const warnings: string[] = [];
  const merged = cloneTokens(DEFAULT_TOKENS);

  if (typeof input !== "object" || input === null || Array.isArray(input)) {
    if (input !== undefined) {
      warnings.push("designTokens: must be an object");
    }
    return { tokens: merged, warnings };
  }

  const raw = input as Record<string, unknown>;
  const knownRoots = new Set(["color", "font", "space", "radius", "chart", "map"]);
  for (const k of Object.keys(raw)) {
    if (!knownRoots.has(k)) {
      warnings.push(`${k}: unknown property`);
    }
  }

  if (raw.color !== undefined) {
    if (typeof raw.color !== "object" || raw.color === null || Array.isArray(raw.color)) {
      warnings.push("color: must be an object");
    } else {
      const c = raw.color as Record<string, unknown>;
      const keys = ["accent", "ink", "muted", "surface", "card", "line", "danger", "success", "warning"] as const;
      const knownKeys = new Set<string>(keys);
      for (const k of Object.keys(c)) {
        if (!knownKeys.has(k)) warnings.push(`color.${k}: unknown property`);
      }
      for (const k of keys) {
        if (c[k] !== undefined) {
          if (isSafeCss(c[k])) merged.color[k] = c[k] as string;
          else warnings.push(`color.${k}: invalid value`);
        }
      }
    }
  }

  if (raw.font !== undefined) {
    if (typeof raw.font !== "object" || raw.font === null || Array.isArray(raw.font)) {
      warnings.push("font: must be an object");
    } else {
      const f = raw.font as Record<string, unknown>;
      for (const k of Object.keys(f)) {
        if (k !== "body" && k !== "mono" && k !== "size") warnings.push(`font.${k}: unknown property`);
      }
      if (f.body !== undefined) {
        if (isSafeCss(f.body)) merged.font.body = f.body as string;
        else warnings.push("font.body: invalid value");
      }
      if (f.mono !== undefined) {
        if (isSafeCss(f.mono)) merged.font.mono = f.mono as string;
        else warnings.push("font.mono: invalid value");
      }
      if (f.size !== undefined) {
        if (typeof f.size !== "object" || f.size === null || Array.isArray(f.size)) {
          warnings.push("font.size: must be an object");
        } else {
          const s = f.size as Record<string, unknown>;
          const sizeKeys = ["sm", "md", "lg", "xl"] as const;
          for (const k of Object.keys(s)) {
            if (!sizeKeys.includes(k as typeof sizeKeys[number])) warnings.push(`font.size.${k}: unknown property`);
          }
          for (const k of sizeKeys) {
            if (s[k] !== undefined) {
              if (isSafeCss(s[k])) merged.font.size[k] = s[k] as string;
              else warnings.push(`font.size.${k}: invalid value`);
            }
          }
        }
      }
    }
  }

  if (raw.space !== undefined) {
    if (typeof raw.space !== "object" || raw.space === null || Array.isArray(raw.space)) {
      warnings.push("space: must be an object");
    } else {
      const sp = raw.space as Record<string, unknown>;
      const spaceKeys = ["1", "2", "3", "4", "6", "8"] as const;
      for (const k of Object.keys(sp)) {
        if (!spaceKeys.includes(k as typeof spaceKeys[number])) warnings.push(`space.${k}: unknown property`);
      }
      for (const k of spaceKeys) {
        if (sp[k] !== undefined) {
          if (isSafeCss(sp[k])) merged.space[k] = sp[k] as string;
          else warnings.push(`space.${k}: invalid value`);
        }
      }
    }
  }

  if (raw.radius !== undefined) {
    if (typeof raw.radius !== "object" || raw.radius === null || Array.isArray(raw.radius)) {
      warnings.push("radius: must be an object");
    } else {
      const r = raw.radius as Record<string, unknown>;
      const radKeys = ["sm", "md", "lg"] as const;
      for (const k of Object.keys(r)) {
        if (!radKeys.includes(k as typeof radKeys[number])) warnings.push(`radius.${k}: unknown property`);
      }
      for (const k of radKeys) {
        if (r[k] !== undefined) {
          if (isSafeCss(r[k])) merged.radius[k] = r[k] as string;
          else warnings.push(`radius.${k}: invalid value`);
        }
      }
    }
  }

  if (raw.chart !== undefined) {
    if (typeof raw.chart !== "object" || raw.chart === null || Array.isArray(raw.chart)) {
      warnings.push("chart: must be an object");
    } else {
      const ch = raw.chart as Record<string, unknown>;
      for (const k of Object.keys(ch)) {
        if (k !== "palette") warnings.push(`chart.${k}: unknown property`);
      }
      if (ch.palette !== undefined) {
        if (!Array.isArray(ch.palette)) {
          warnings.push("chart.palette: must be an array");
        } else if (ch.palette.length > 0) {
          const valid = ch.palette.filter((p): p is string => isSafeCss(p));
          if (valid.length !== ch.palette.length) {
            warnings.push("chart.palette: contains invalid color values");
          }
          if (valid.length > 0) {
            merged.chart.palette = valid;
          }
        }
      }
    }
  }

  if (raw.map !== undefined) {
    if (typeof raw.map !== "object" || raw.map === null || Array.isArray(raw.map)) {
      warnings.push("map: must be an object");
    } else {
      const m = raw.map as Record<string, unknown>;
      const mapKeys = ["point", "selected", "low", "high", "stroke"] as const;
      for (const k of Object.keys(m)) {
        if (!mapKeys.includes(k as typeof mapKeys[number])) warnings.push(`map.${k}: unknown property`);
      }
      for (const k of mapKeys) {
        if (m[k] !== undefined) {
          if (isSafeCss(m[k])) merged.map[k] = m[k] as string;
          else warnings.push(`map.${k}: invalid value`);
        }
      }
    }
  }

  return { tokens: merged, warnings };
}

export function applyTokens(input: unknown, root?: HTMLElement): DesignTokens {
  const { tokens } = resolveTokens(input);
  appliedTokens = tokens;

  const target = root ?? (typeof document !== "undefined" ? document.documentElement : undefined);
  if (target) {
    target.style.setProperty("--jc-color-accent", tokens.color.accent);
    target.style.setProperty("--jc-color-ink", tokens.color.ink);
    target.style.setProperty("--jc-color-muted", tokens.color.muted);
    target.style.setProperty("--jc-color-surface", tokens.color.surface);
    target.style.setProperty("--jc-color-card", tokens.color.card);
    target.style.setProperty("--jc-color-line", tokens.color.line);
    target.style.setProperty("--jc-color-danger", tokens.color.danger);
    target.style.setProperty("--jc-color-success", tokens.color.success);
    target.style.setProperty("--jc-color-warning", tokens.color.warning);

    target.style.setProperty("--jc-font-body", withBundledFont(tokens.font.body));
    target.style.setProperty("--jc-font-mono", tokens.font.mono);
    target.style.setProperty("--jc-font-size-sm", tokens.font.size.sm);
    target.style.setProperty("--jc-font-size-md", tokens.font.size.md);
    target.style.setProperty("--jc-font-size-lg", tokens.font.size.lg);
    target.style.setProperty("--jc-font-size-xl", tokens.font.size.xl);

    target.style.setProperty("--jc-space-1", tokens.space["1"]);
    target.style.setProperty("--jc-space-2", tokens.space["2"]);
    target.style.setProperty("--jc-space-3", tokens.space["3"]);
    target.style.setProperty("--jc-space-4", tokens.space["4"]);
    target.style.setProperty("--jc-space-6", tokens.space["6"]);
    target.style.setProperty("--jc-space-8", tokens.space["8"]);

    target.style.setProperty("--jc-radius-sm", tokens.radius.sm);
    target.style.setProperty("--jc-radius-md", tokens.radius.md);
    target.style.setProperty("--jc-radius-lg", tokens.radius.lg);

    // Legacy kit custom properties
    target.style.setProperty("--accent", tokens.color.accent);
    target.style.setProperty("--ink", tokens.color.ink);
    target.style.setProperty("--muted", tokens.color.muted);
    target.style.setProperty("--surface", tokens.color.surface);
    target.style.setProperty("--card", tokens.color.card);
    target.style.setProperty("--line", tokens.color.line);
  }

  return tokens;
}

export function currentTokens(): DesignTokens {
  return appliedTokens;
}

export function echartsTheme(tokens?: DesignTokens): Record<string, unknown> {
  const t = tokens ?? currentTokens();
  return {
    color: t.chart.palette,
    backgroundColor: "transparent",
    textStyle: {
      color: t.color.ink,
      fontFamily: t.font.body,
    },
    title: {
      textStyle: { color: t.color.ink },
      subtextStyle: { color: t.color.muted },
    },
    legend: {
      textStyle: { color: t.color.ink },
    },
    categoryAxis: {
      axisLine: { lineStyle: { color: t.color.line } },
      splitLine: { lineStyle: { color: t.color.line } },
      axisLabel: { color: t.color.muted },
    },
    valueAxis: {
      axisLine: { lineStyle: { color: t.color.line } },
      splitLine: { lineStyle: { color: t.color.line } },
      axisLabel: { color: t.color.muted },
    },
  };
}

export function rechartsPalette(tokens?: DesignTokens): string[] {
  return (tokens ?? currentTokens()).chart.palette;
}

export function mapColors(tokens?: DesignTokens): DesignTokens["map"] {
  return (tokens ?? currentTokens()).map;
}
