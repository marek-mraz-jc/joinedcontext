// The merged tasks a "What's new" entry may be written from (T-3271): the Portal's tasks merged
// since a day, in groups a person sees, newest first. A person reads the list, writes the entries
// people need into ui/src/whatsNew.ts and the locale files, and leaves the rest out.
//
//   node scripts/whats-new-candidates.mjs <tasks/finished> <YYYY-MM-DD>
import { readdirSync, readFileSync } from "node:fs";
import { join } from "node:path";
import { pathToFileURL } from "node:url";

/** Groups whose work a person using the Portal never sees: checks, red lanes, audits. */
const UNSEEN = /ci|red|security|attack|verify|edge|test|conformance|perf|deploy|compliance|budget/i;

const field = (text, name) => new RegExp(`^${name}:\\s*"?(.*?)"?\\s*$`, "m").exec(text)?.[1] ?? "";

/** One task file as a candidate, or null when it is not one. */
export function candidate(name, text, since) {
  if (field(text, "repo") !== "joinedcontext-portal" || UNSEEN.test(field(text, "group"))) return null;
  const merged = [...text.matchAll(/^## (\d{4}-\d{2}-\d{2})[^\n]*merged on main/gm)].map((m) => m[1]).sort().at(-1);
  if (!merged || merged < since) return null;
  return { id: name.slice(0, 6), merged, group: field(text, "group"), title: field(text, "title") };
}

export function candidates(dir, since) {
  return readdirSync(dir)
    .filter((name) => /^T-\d+.*\.md$/.test(name))
    .map((name) => candidate(name, readFileSync(join(dir, name), "utf8"), since))
    .filter(Boolean)
    .sort((a, b) => b.merged.localeCompare(a.merged) || a.id.localeCompare(b.id));
}

if (import.meta.url === pathToFileURL(process.argv[1] ?? "").href) {
  const [dir, since] = process.argv.slice(2);
  if (!dir || !/^\d{4}-\d{2}-\d{2}$/.test(since ?? "")) {
    console.error("usage: node scripts/whats-new-candidates.mjs <tasks/finished> <YYYY-MM-DD>");
    process.exit(2);
  }
  for (const c of candidates(dir, since)) console.log(`${c.merged}  ${c.id}  ${c.group}  ${c.title}`);
}
