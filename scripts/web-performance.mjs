// The public pages on a phone (T-5122): Lighthouse's mobile profile (simulated slow 4G, a 4x
// slower CPU) on dev, three runs a page, the median judged. A page over its ceiling fails the lane;
// over the target (LCP 2.5 s, CLS 0.1, TBT 300 ms, the "good" line of Core Web Vitals) it warns,
// until the page earns the target and its ceiling comes down to it. Ceilings only ever go down.
//
//   node scripts/web-performance.mjs urls dev.joinedcontext.com   the pages, one per line
//   node scripts/web-performance.mjs check <reports dir>          judge <slug>-<run>.json reports
import { readdirSync, readFileSync } from "node:fs";
import { join } from "node:path";
import { pathToFileURL } from "node:url";

export const TARGET = { lcp: 2500, cls: 0.1, tbt: 300 };

/**
 * Each page with its ceiling. Measured 2026-10-22 (T-5122) with Lighthouse 12.8.2 in this profile
 * after the edge compressed static code and the public pages got their own entry: the Portal's
 * catalogue 3.0 s (12.5 s before), its dataset page 3.3 s, its accessibility statement 2.54 s,
 * CKAN's home 1.8 s (9.4 s), the assistant 1.8 s (2.5 s). A ceiling is the measure plus a margin
 * for the runner, never above the measure before the fix.
 */
export const PAGES = [
  { slug: "portal-catalogue", path: (d) => `https://portal.${d}/catalogue`, ceiling: { lcp: 3600, cls: 0.1, tbt: 600 } },
  { slug: "portal-dataset", path: (d, ds) => `https://portal.${d}/catalogue/${ds}`, ceiling: { lcp: 4000, cls: 0.1, tbt: 600 } },
  { slug: "portal-accessibility", path: (d) => `https://portal.${d}/accessibility`, ceiling: { lcp: 3000, cls: 0.1, tbt: 600 } },
  { slug: "ckan-home", path: (d) => `https://opendata.${d}/`, ceiling: { lcp: 3500, cls: 0.1, tbt: 600 } },
  { slug: "ckan-dataset", path: (d, ds) => `https://opendata.${d}/dataset/${ds}`, ceiling: { lcp: 6000, cls: 0.1, tbt: 1500 } },
  { slug: "assistant", path: (d) => `https://ai.${d}/bbsk/`, ceiling: { lcp: 3500, cls: 0.1, tbt: 600 } },
];

/** The dataset every dataset page is measured on: the catalogue's first, so it always exists. */
export const DATASET = "banskabystrica-pocasie";

export function median(values) {
  const sorted = [...values].sort((a, b) => a - b);
  return sorted[Math.floor(sorted.length / 2)];
}

/** LCP, CLS and TBT of one Lighthouse report; null when the page did not load (runtimeError). */
export function metrics(report) {
  if (report.runtimeError?.code) return null;
  const value = (id) => report.audits?.[id]?.numericValue;
  const m = { lcp: value("largest-contentful-paint"), cls: value("cumulative-layout-shift"), tbt: value("total-blocking-time") };
  return Object.values(m).every((v) => typeof v === "number") ? m : null;
}

/** The verdict of one page from its runs' metrics: what crossed the ceiling, what missed the target. */
export function judge(page, runs) {
  if (runs.length === 0 || runs.includes(null)) return { page: page.slug, failed: ["the page did not load in every run"], warned: [], median: null };
  const med = Object.fromEntries(Object.keys(TARGET).map((k) => [k, median(runs.map((r) => r[k]))]));
  const failed = Object.keys(TARGET).filter((k) => med[k] > page.ceiling[k]).map((k) => `${k} ${med[k]} over its ceiling ${page.ceiling[k]}`);
  const warned = Object.keys(TARGET).filter((k) => med[k] > TARGET[k]).map((k) => `${k} ${med[k]} over the target ${TARGET[k]}`);
  return { page: page.slug, failed, warned, median: med };
}

if (import.meta.url === pathToFileURL(process.argv[1] ?? "").href) {
  const [command, arg] = process.argv.slice(2);
  if (command === "urls" && /^[a-z0-9.-]+$/.test(arg ?? "")) {
    for (const page of PAGES) console.log(`${page.slug} ${page.path(arg, DATASET)}`);
  } else if (command === "check" && arg) {
    const files = readdirSync(arg).filter((f) => f.endsWith(".json"));
    let failed = false;
    for (const page of PAGES) {
      const runs = files.filter((f) => f.startsWith(`${page.slug}-`)).map((f) => metrics(JSON.parse(readFileSync(join(arg, f), "utf8"))));
      // A run that wrote no report counts as one that did not load: three runs or the page fails.
      while (runs.length < 3) runs.push(null);
      const verdict = judge(page, runs);
      const shown = verdict.median ? `LCP ${Math.round(verdict.median.lcp)} ms, CLS ${verdict.median.cls.toFixed(3)}, TBT ${Math.round(verdict.median.tbt)} ms` : "no measure";
      console.log(`${page.slug.padEnd(22)} ${shown}`);
      for (const line of verdict.warned) console.log(`::warning title=${page.slug}::${line}`);
      for (const line of verdict.failed) console.log(`::error title=${page.slug}::${line}`);
      failed ||= verdict.failed.length > 0;
    }
    if (failed) process.exit(1);
  } else {
    console.error("usage: node scripts/web-performance.mjs urls <domain> | check <reports dir>");
    process.exit(2);
  }
}
