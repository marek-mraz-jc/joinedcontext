/**
 * T-2815, UI-02: an example written into a page reads as nobody's organization.
 *
 * T-2752 made the arrangements' examples neutral and `form_help` guards them; an example typed
 * as a literal `placeholder` in page code escaped both, and the Helsinki instance asked for
 * `jana.kovacova@example.sk`. This reads the source: a literal placeholder may name no country
 * domain and carry no letter outside ASCII. The component gallery shows components, not a city,
 * and is left out.
 */
import { readdirSync, readFileSync, statSync } from "node:fs";
import { join, relative } from "node:path";
import { describe, expect, it } from "vitest";

const SRC = join(__dirname, "../src");
const LEFT_OUT = ["pages/gallery/"];

function sources(dir: string): string[] {
  return readdirSync(dir).flatMap((name) => {
    const path = join(dir, name);
    if (statSync(path).isDirectory()) return sources(path);
    return /\.tsx$/.test(name) ? [path] : [];
  });
}

/** `placeholder="…"` and every string literal inside `placeholder={…}` (a ternary, say). */
function placeholders(text: string): string[] {
  const found: string[] = [];
  for (const match of text.matchAll(/placeholder=(?:"([^"]*)"|\{([^{}]*)\})/g)) {
    if (match[1] !== undefined) found.push(match[1]);
    for (const literal of (match[2] ?? "").matchAll(/"([^"]*)"|'([^']*)'/g)) {
      found.push(literal[1] ?? literal[2]);
    }
  }
  return found;
}

const FOREIGN = /\.(sk|cz|fi|de|at|pl|hu|fr|it|es|nl|se|dk|no)\b|[^\x20-\x7e]/;

describe("examples written into pages (T-2815)", () => {
  const files = sources(SRC).filter((path) => !LEFT_OUT.some((part) => relative(SRC, path).startsWith(part)));

  it("reads enough of the source to mean something", () => {
    expect(files.flatMap((path) => placeholders(readFileSync(path, "utf8"))).length).toBeGreaterThan(10);
  });

  it("names no country domain and no letter outside ASCII", () => {
    const foreign = files.flatMap((path) =>
      placeholders(readFileSync(path, "utf8"))
        .filter((example) => FOREIGN.test(example))
        .map((example) => `${relative(SRC, path)}: ${example}`),
    );
    expect(foreign).toEqual([]);
  });
});
