/**
 * A gap names a spacing the theme defines (T-6917): Tailwind draws no rule for `gap-card` when
 * no `--spacing-card` exists, so the class silently leaves the cards touching.
 */
import { readdirSync, readFileSync, statSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it } from "vitest";

const SRC = join(__dirname, "../src");

function files(dir: string): string[] {
  return readdirSync(dir).flatMap((name) => {
    const path = join(dir, name);
    return statSync(path).isDirectory() ? files(path) : /\.tsx?$/.test(name) ? [path] : [];
  });
}

describe("named gaps", () => {
  it("use only a spacing the theme defines", () => {
    const defined = new Set([...readFileSync(join(SRC, "index.css"), "utf8").matchAll(/--spacing-([a-z][a-z-]*):/g)].map((m) => m[1]));
    defined.add("px");
    const unknown = files(SRC).flatMap((path) =>
      [...readFileSync(path, "utf8").matchAll(/(?<![\w-])gap-(?:[xy]-)?([a-z][a-z-]*[a-z])(?![\w-])/g)]
        .map((m) => m[1])
        .filter((name) => !defined.has(name))
        .map((name) => `${path.slice(SRC.length + 1)}: gap-${name}`),
    );
    expect(unknown).toEqual([]);
  });
});
