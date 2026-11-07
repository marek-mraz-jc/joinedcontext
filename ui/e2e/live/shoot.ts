/**
 * T-4156 — the UX photo audit's camera (T-4157..T-4163). `shot` saves one full-page PNG under
 * `$SHOTS_DIR` named `<area>-<route-slug>-<state>-<who>-<lang>-<width>.png` and records what it
 * shows in `shots.jsonl` beside it, because a slug full of dashes cannot be read back into its
 * route. `writeIndex` turns a folder into one static contact sheet, grouped by route, and refuses
 * a `findings.json` that does not match `findings.schema.json`.
 *
 * Read-only like the walker; whoever calls `shot` makes sure no secret is on the screen.
 */
import { appendFileSync, existsSync, mkdirSync, readFileSync, readdirSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import type { Locator, Page } from "@playwright/test";

export interface Shot {
  area: string;
  route: string;
  state: string;
  who: "steward" | "viewer" | "anonymous" | "author" | "neighbour";
  lang: string;
  width: number;
}

const slug = (text: string) =>
  text
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-|-$/g, "") || "root";

export function shotName({ area, route, state, who, lang, width }: Shot): string {
  return `${[area, route, state, who, lang].map(slug).join("-")}-${width}.png`;
}

/**
 * What no picture shows (T-4950): a typed password (the same mask as guide.ts) and any text on the
 * page shaped like a token, a JWT or a bearer credential, wherever a page happens to print one.
 */
export function secretMasks(page: Page): Locator[] {
  return [page.locator("input[type=password]"), page.getByText(/\beyJ[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,}|\bBearer\s+[A-Za-z0-9._~+/=-]{16,}/)];
}

/** Saves the page as it stands; returns the file path, or undefined when `SHOTS_DIR` is unset. */
export async function shot(page: Page, what: Omit<Shot, "width">, dir = process.env.SHOTS_DIR): Promise<string | undefined> {
  if (!dir) return undefined;
  const width = page.viewportSize()?.width ?? 0;
  const entry: Shot = { ...what, width };
  const file = shotName(entry);
  mkdirSync(dir, { recursive: true });
  await page.screenshot({ path: join(dir, file), fullPage: true, mask: secretMasks(page) });
  appendFileSync(join(dir, "shots.jsonl"), `${JSON.stringify({ file, ...entry })}\n`);
  return join(dir, file);
}

interface SchemaNode {
  type?: string;
  enum?: unknown[];
  minLength?: number;
  required?: string[];
  additionalProperties?: boolean;
  properties?: Record<string, SchemaNode>;
  items?: SchemaNode;
}

const schema = JSON.parse(readFileSync(join(dirname(fileURLToPath(import.meta.url)), "findings.schema.json"), "utf8")) as SchemaNode;

/** The part of JSON Schema `findings.schema.json` uses; each line returned is one violation. */
function check(value: unknown, node: SchemaNode, at: string): string[] {
  const kind = value === null ? "null" : Array.isArray(value) ? "array" : Number.isInteger(value) ? "integer" : typeof value;
  if (node.type && kind !== node.type) return [`${at}: is ${kind}, not ${node.type}`];
  if (node.enum && !node.enum.includes(value)) return [`${at}: ${JSON.stringify(value)} is not one of ${node.enum.join(", ")}`];
  if (node.minLength && typeof value === "string" && value.trim().length < node.minLength) return [`${at}: is empty`];
  if (kind === "array" && node.items) return (value as unknown[]).flatMap((item, i) => check(item, node.items as SchemaNode, `${at}[${i}]`));
  if (kind !== "object") return [];
  const record = value as Record<string, unknown>;
  return [
    ...(node.required ?? []).filter((key) => !(key in record)).map((key) => `${at}: ${key} is missing`),
    ...Object.entries(record).flatMap(([key, field]) => {
      const sub = node.properties?.[key];
      if (sub) return check(field, sub, `${at}.${key}`);
      return node.additionalProperties === false ? [`${at}: ${key} is not a known field`] : [];
    }),
  ];
}

export const findingErrors = (findings: unknown): string[] => check(findings, schema, "findings");

const escape = (text: string) =>
  text.replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[c] as string);

/** Writes `<dir>/index.html` and returns how many PNGs it lists; throws on an invalid findings.json. */
export function writeIndex(dir: string): number {
  const findingsFile = join(dir, "findings.json");
  if (existsSync(findingsFile)) {
    const errors = findingErrors(JSON.parse(readFileSync(findingsFile, "utf8")));
    if (errors.length) throw new Error(`findings.json does not match findings.schema.json:\n${errors.join("\n")}`);
  }
  const recorded = new Map<string, Shot>();
  const log = join(dir, "shots.jsonl");
  if (existsSync(log)) {
    for (const line of readFileSync(log, "utf8").split("\n").filter(Boolean)) {
      const { file, ...entry } = JSON.parse(line) as Shot & { file: string };
      recorded.set(file, entry);
    }
  }
  const groups = new Map<string, string[]>();
  const pngs = readdirSync(dir).filter((file) => file.endsWith(".png")).sort();
  for (const file of pngs) {
    const entry = recorded.get(file);
    const route = entry?.route ?? "(not recorded by shot)";
    const caption = entry ? `${entry.state} · ${entry.who} · ${entry.lang} · ${entry.width}px` : file;
    const figure = `<figure><a href="${escape(encodeURI(file))}"><img src="${escape(encodeURI(file))}" alt="${escape(`${route}, ${caption}`)}" loading="lazy"></a><figcaption>${escape(caption)}</figcaption></figure>`;
    groups.set(route, [...(groups.get(route) ?? []), figure]);
  }
  const sections = [...groups].map(([route, figures]) => `<section><h2>${escape(route)}</h2><div class="grid">${figures.join("")}</div></section>`);
  writeFileSync(
    join(dir, "index.html"),
    `<!doctype html><html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width, initial-scale=1"><title>UX photo audit</title><style>body{font:14px system-ui,sans-serif;margin:16px;background:#fff;color:#111}.grid{display:grid;grid-template-columns:repeat(auto-fill,minmax(220px,1fr));gap:12px}figure{margin:0}img{width:100%;height:160px;object-fit:cover;object-position:top;border:1px solid #ccc}</style></head><body><h1>UX photo audit: ${pngs.length} screenshots</h1>${sections.join("")}</body></html>\n`,
  );
  return pngs.length;
}

