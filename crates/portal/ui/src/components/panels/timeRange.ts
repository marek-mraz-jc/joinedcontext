/**
 * Grafana's time words (`now`, `now-6h`, `now-1d/d`, epoch milliseconds, ISO 8601) as the instants
 * a chart's axis spans, and the ranges and refresh intervals the dashboard page offers.
 */

const UNIT_MS: Record<string, number> = {
  s: 1000,
  m: 60_000,
  h: 3_600_000,
  d: 86_400_000,
  w: 604_800_000,
  M: 2_592_000_000,
  y: 31_536_000_000,
};

/** Rounds `at` down to the start of its `unit` (`/d`, `/h` ...), in the reader's time zone. */
function roundDown(at: number, unit: string): number {
  const date = new Date(at);
  switch (unit) {
    case "y":
      return new Date(date.getFullYear(), 0, 1).getTime();
    case "M":
      return new Date(date.getFullYear(), date.getMonth(), 1).getTime();
    case "w": {
      const start = new Date(date.getFullYear(), date.getMonth(), date.getDate());
      start.setDate(start.getDate() - ((start.getDay() + 6) % 7));
      return start.getTime();
    }
    case "d":
      return new Date(date.getFullYear(), date.getMonth(), date.getDate()).getTime();
    default: {
      const size = UNIT_MS[unit] ?? 1;
      return Math.floor(at / size) * size;
    }
  }
}

/** The instant `text` names at `now`; `null` for a word Grafana does not use. */
export function resolveTime(text: string, now: number): number | null {
  const trimmed = text.trim();
  if (/^\d+$/.test(trimmed)) return Number(trimmed);
  const relative = /^now(?:([+-])(\d+)([smhdwMy]))?(?:\/([smhdwMy]))?$/.exec(trimmed);
  if (relative) {
    const [, sign, amount, unit, round] = relative;
    let at = now;
    if (sign && amount && unit) at += (sign === "-" ? -1 : 1) * Number(amount) * UNIT_MS[unit];
    return round ? roundDown(at, round) : at;
  }
  const parsed = Date.parse(trimmed);
  return Number.isNaN(parsed) ? null : parsed;
}

/** The ranges the picker offers, from five minutes to a year. */
export const RANGES = ["now-5m", "now-15m", "now-1h", "now-3h", "now-6h", "now-12h", "now-24h", "now-2d", "now-7d", "now-30d", "now-90d", "now-1y"] as const;

/** The refresh intervals the picker offers; `""` is off. */
export const REFRESHES = ["", "10s", "30s", "1m", "5m", "15m", "1h"] as const;

/** A refresh interval (`30s`, `5m`, `1h`) in milliseconds; 0 for off or a word it does not know. */
export function refreshMs(text: string | undefined): number {
  const match = /^(\d+)([smhd])$/.exec(text ?? "");
  return match ? Number(match[1]) * UNIT_MS[match[2]] : 0;
}

/** `now-6h` as its amount and unit word, for the picker's label: `{ n: 6, unit: "h" }`. */
export function rangeParts(from: string): { n: number; unit: string } | null {
  const match = /^now-(\d+)([smhdwMy])$/.exec(from);
  return match ? { n: Number(match[1]), unit: match[2] } : null;
}
