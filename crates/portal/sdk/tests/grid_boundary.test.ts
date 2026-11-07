/**
 * T-6305 (G-R2GRID2-002, owner 2026-10-31): the grid is usable outside the Portal, so nothing the
 * `./grid` entry reaches may import a Portal path or a package an App does not get with the SDK.
 * The walk follows every relative import from src/sdk/grid.ts; CSS and `import type` count too.
 */
import { readFileSync, statSync } from "node:fs";
import { dirname, join, relative, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import ts from "typescript";
import { describe, expect, it } from "vitest";

const src = resolve(dirname(fileURLToPath(import.meta.url)), "../src");
/** React, and the map stack GridMap draws with: the SDK's peers and its own dependencies. */
const PACKAGES = /^(react|react-dom|maplibre-gl|terra-draw|terra-draw-maplibre-gl-adapter)(\/|$)/;

function resolveFile(from: string, spec: string): string | null {
  const base = resolve(dirname(from), spec);
  return [base, `${base}.ts`, `${base}.tsx`, join(base, "index.ts")].find((candidate) => statSync(candidate, { throwIfNoEntry: false })?.isFile()) ?? null;
}

/** Every module a file names: static imports, re-exports and dynamic `import()`, as TypeScript reads them. */
const specifiers = (file: string): string[] =>
  ts.preProcessFile(readFileSync(file, "utf8"), true, true).importedFiles.map((ref) => ref.fileName);

function walk(): { files: string[]; refused: string[] } {
  const seen = new Set<string>();
  const refused: string[] = [];
  const queue = [join(src, "sdk/grid.ts")];
  while (queue.length > 0) {
    const file = queue.pop()!;
    if (seen.has(file)) continue;
    seen.add(file);
    if (file.endsWith(".css")) continue;
    for (const spec of specifiers(file)) {
      const where = relative(src, file);
      if (spec.startsWith(".")) {
        const target = resolveFile(file, spec);
        if (target === null) refused.push(`${where}: ${spec} does not resolve`);
        else if (relative(src, target).startsWith("..")) refused.push(`${where}: ${spec} leaves the SDK`);
        else queue.push(target);
      } else if (!PACKAGES.test(spec)) {
        refused.push(`${where}: ${spec}`);
      }
    }
  }
  return { files: [...seen], refused };
}

describe("the grid entry's imports", () => {
  it("reach only SDK files, React and the SDK's map stack", () => {
    const { files, refused } = walk();
    expect(files.length).toBeGreaterThan(20);
    expect(refused).toEqual([]);
  });
});
