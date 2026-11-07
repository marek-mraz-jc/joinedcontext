#!/usr/bin/env node
// Writes ui/src/labels.ts: the statistics office's own names for the codes the records carry
// (T-2966). The pipelines keep the codes (`indicator`, `refArea`, `dimensionKey`); the names are
// the cube's category labels, read in Slovak and English from the same URL each pipeline reads.
//
//   node labels.mjs <deployment>/components/context-gateway/seed
//
// Run again when a pipeline starts reading another cube or indicator.
import { readdirSync, readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";

const seed = process.argv[2];
if (!seed) {
  console.error("usage: node labels.mjs <deployment>/components/context-gateway/seed");
  process.exit(2);
}

const BODIES = ["banskabystrica", "bbsk"];
const LANGUAGES = ["sk", "en"];

/** The `let name = "value"` / `let name = [...]` lines of a mapping. */
function lets(text) {
  const found = {};
  for (const [, name, value] of text.matchAll(/^\s*let (\w+) = ("[^"]*"|\[[^\]]*\])\s*$/gm)) {
    found[name] = JSON.parse(value);
  }
  return found;
}

async function cube(url, language) {
  const answer = await fetch(url.replace(/lang=\w+/, `lang=${language}`), { signal: AbortSignal.timeout(60_000) });
  if (!answer.ok) throw new Error(`${url}: HTTP ${answer.status}`);
  return answer.json();
}

const datasets = {};
const codes = {};
const areas = {};
const bodies = {};
for (const body of BODIES) {
  const folder = join(seed, body);
  for (const file of readdirSync(folder).filter((name) => name.endsWith("-bento.yaml")).sort()) {
    const found = lets(readFileSync(join(folder, file), "utf8"));
    const url = found.source_url;
    if (!found.cube || !url?.startsWith("https://data.statistics.sk/")) continue;
    const named = [found.indicator_dim, ...(found.key_dims ?? [])];
    (bodies[body] ??= []).push(found.cube);
    for (const language of LANGUAGES) {
      const answer = await cube(url, language);
      (datasets[found.cube] ??= {})[language] = answer.label;
      for (const dim of named) {
        for (const [code, label] of Object.entries(answer.dimension[dim].category.label ?? {})) {
          if (label == null) continue;
          // `dimensionKey` joins the key codes with "-", so a code carrying one could not be split.
          if (code.includes("-")) throw new Error(`${found.cube}/${dim}: code ${code} holds a "-"`);
          ((codes[found.cube] ??= {})[code] ??= {})[language] = label;
        }
      }
      for (const [code, label] of Object.entries(answer.dimension[found.area_dim].category.label ?? {})) {
        if (label == null) continue;
        (areas[code] ??= {})[language] = label;
      }
    }
    console.error(`${body}/${file}: ${found.cube}`);
  }
}

const sorted = (object) => Object.fromEntries(Object.entries(object).sort(([a], [b]) => a.localeCompare(b)));
const out = `/**
 * The statistics office's own names for the codes the records carry (T-2966): each cube's title,
 * the labels of its indicator and key dimensions, and the territories, in Slovak and English.
 * Written by \`../../labels.mjs\` from the URLs the pipelines read (data.statistics.sk); do not
 * edit by hand.
 */
export type Named = { sk?: string; en?: string };

export const DATASETS: Record<string, Named> = ${JSON.stringify(sorted(datasets), null, 2)};

export const CODES: Record<string, Record<string, Named>> = ${JSON.stringify(
  sorted(Object.fromEntries(Object.entries(codes).map(([key, value]) => [key, sorted(value)]))),
  null,
  2,
)};

export const AREAS: Record<string, Named> = ${JSON.stringify(sorted(areas), null, 2)};

/** The cubes each body's pipelines read, in the order of their files. */
export const BODY_DATASETS: Record<string, string[]> = ${JSON.stringify(bodies, null, 2)};
`;
writeFileSync(new URL("./ui/src/labels.ts", import.meta.url), out);
