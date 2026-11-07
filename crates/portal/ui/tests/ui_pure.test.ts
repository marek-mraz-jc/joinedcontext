// T-5122: vite.config.ts declares src/components/ui free of side effects, so the bundler drops the
// controls a page does not use. A top-level call there would be dropped with them: none is allowed.
import { readFileSync, readdirSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it } from "vitest";

// Vitest runs from ui/; jsdom gives import.meta.url no file scheme.
const DIR = join(process.cwd(), "src/components/ui");

describe("the shared controls run nothing when imported", () => {
  it("has no top-level statement other than declarations, imports and exports", () => {
    const offenders = readdirSync(DIR)
      .filter((name) => /\.tsx?$/.test(name))
      .flatMap((name) =>
        readFileSync(join(DIR, name), "utf8")
          .split("\n")
          .filter((line) => /^[A-Za-z_$][\w$.]*\s*\(/.test(line) || /^import\s+["']/.test(line))
          .map((line) => `${name}: ${line}`),
      );
    expect(offenders).toEqual([]);
  });
});
