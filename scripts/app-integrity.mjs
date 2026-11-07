// Writes `integrity.json` beside a built static app (AP-12): every file of the bundle, by its
// bundle-relative path, to the `sha384-<base64>` Subresource Integrity digest the Portal's static
// host checks before it serves the file (src/apps/static_host.rs). A file missing from the map is
// never served, so the map is the whole bundle or the app serves nothing.
//
//   node scripts/app-integrity.mjs <dist-dir>

import { createHash } from "node:crypto";
import { readdirSync, readFileSync, writeFileSync } from "node:fs";
import { join, relative, sep } from "node:path";

export const MANIFEST = "integrity.json";

export function sri(bytes) {
  return `sha384-${createHash("sha384").update(bytes).digest("base64")}`;
}

export function digests(root) {
  const map = {};
  const walk = (dir) => {
    for (const entry of readdirSync(dir, { withFileTypes: true })) {
      const path = join(dir, entry.name);
      if (entry.isDirectory()) {
        walk(path);
      } else if (entry.isFile()) {
        const name = relative(root, path).split(sep).join("/");
        if (name !== MANIFEST) map[name] = sri(readFileSync(path));
      }
    }
  };
  walk(root);
  if (!map["index.html"]) {
    throw new Error(`${root} has no index.html, so there is no app to publish`);
  }
  return Object.fromEntries(Object.entries(map).sort(([a], [b]) => a.localeCompare(b)));
}

if (import.meta.url === `file://${process.argv[1]}`) {
  const root = process.argv[2];
  if (!root) {
    console.error("usage: node scripts/app-integrity.mjs <dist-dir>");
    process.exit(2);
  }
  const map = digests(root);
  writeFileSync(join(root, MANIFEST), `${JSON.stringify(map, null, 2)}\n`);
  console.log(`${join(root, MANIFEST)}: ${Object.keys(map).length} files`);
}
