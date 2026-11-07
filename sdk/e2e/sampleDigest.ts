import { createHash } from "node:crypto";
import { readdirSync, readFileSync } from "node:fs";
import { join } from "node:path";

/**
 * The digest of what a template's screenshots show (T-3306): the sample's own files, in name
 * order, without its tests and without the screenshots themselves.
 */
export function sourceDigest(dir: string): string {
  const hash = createHash("sha256");
  const walk = (at: string): void => {
    for (const entry of readdirSync(at, { withFileTypes: true }).sort((a, b) => a.name.localeCompare(b.name))) {
      const path = join(at, entry.name);
      if (entry.isDirectory()) walk(path);
      else if (!/\.test\.tsx?$|\.png$/.test(entry.name)) hash.update(entry.name).update(readFileSync(path));
    }
  };
  walk(dir);
  return hash.digest("hex");
}
