/**
 * The UI's data client is the Portal API's own contract (UI-06, UI-07).
 *
 * `ui/openapi.json` is held to the routes the Portal serves by `tests/openapi_tests.rs`
 * (`committed_openapi_spec_is_current`). This file holds the other half: the TypeScript types
 * every call is checked against are what `openapi-typescript` makes of that document, so a
 * route or a field that changed in the API cannot keep compiling in the UI against a copy that
 * did not.
 */
import { execFileSync } from "node:child_process";
import { mkdtempSync, readFileSync, readdirSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";

const ui = resolve(dirname(fileURLToPath(import.meta.url)), "..");

/** The command `pnpm generate:api` runs, from package.json, so the two cannot drift apart. */
function generateCommand(): string[] {
  const scripts = (JSON.parse(readFileSync(join(ui, "package.json"), "utf8")) as {
    scripts: Record<string, string>;
  }).scripts;
  return scripts["generate:api"].split(/\s+/);
}

describe("the generated API client (UI-06)", () => {
  // UI-06: the committed types are exactly what openapi-typescript generates from openapi.json.
  it("is what openapi-typescript makes of the committed OpenAPI document", () => {
    const [bin, input, flag, output] = generateCommand();
    expect([bin, flag]).toEqual(["openapi-typescript", "-o"]);
    const scratch = mkdtempSync(join(tmpdir(), "jc-schema-"));
    try {
      const fresh = join(scratch, "schema.d.ts");
      execFileSync(join(ui, "node_modules/.bin", bin), [input, flag, fresh], {
        cwd: ui,
        stdio: "pipe",
      });
      expect(
        readFileSync(join(ui, output), "utf8"),
        "src/api/schema.d.ts is stale: run `pnpm generate:api` in ui/",
      ).toBe(readFileSync(fresh, "utf8"));
    } finally {
      rmSync(scratch, { recursive: true, force: true });
    }
  });

  // UI-06: the client is typed by those paths, so a call to a route the document lacks fails tsc.
  it("types the client by the generated paths", () => {
    const client = readFileSync(join(ui, "src/api/client.ts"), "utf8");
    expect(client).toMatch(/import type \{[^}]*\bpaths\b[^}]*\} from "\.\/schema"/);
    expect(client).toMatch(/createClient<paths>\(/);
  });
});

/**
 * The first argument of every raw `fetch(` call in a source text (not `refetch(`, not `x.fetch(`).
 * ponytail: it reads the argument's text, so a URL built by a helper (`fetch(urlOf(x))`) is not
 * seen; resolve the helper's body if one ever slips through.
 */
function fetchTargets(source: string): string[] {
  const targets: string[] = [];
  const call = /(^|[^A-Za-z0-9_$.])fetch\(/g;
  for (let match = call.exec(source); match; match = call.exec(source)) {
    let depth = 0;
    let end = call.lastIndex;
    for (; end < source.length; end += 1) {
      const c = source[end];
      if (c === "(" || c === "[" || c === "{") depth += 1;
      else if (c === ")" || c === "]" || c === "}") {
        if (depth === 0) break;
        depth -= 1;
      } else if (c === "," && depth === 0) break;
    }
    targets.push(source.slice(call.lastIndex, end).trim());
  }
  return targets;
}

function sources(directory: string): string[] {
  return readdirSync(join(ui, directory), { withFileTypes: true }).flatMap((entry) => {
    const path = `${directory}/${entry.name}`;
    if (entry.isDirectory()) return sources(path);
    return /\.(ts|tsx)$/.test(entry.name) && !entry.name.endsWith(".d.ts") ? [path] : [];
  });
}

describe("every call to the Portal API is a typed one (UI-07)", () => {
  it("reads the target of a raw fetch and nothing else", () => {
    expect(fetchTargets('await fetch(`/api/v1/projects/${p}/import`, { method: "POST" })')).toEqual([
      "`/api/v1/projects/${p}/import`",
    ]);
    expect(fetchTargets("fetch(new Request(url(a, b), init))")).toEqual(["new Request(url(a, b), init)"]);
    expect(fetchTargets("void projects.refetch(); api.fetch(x); prefetch(y)")).toEqual([]);
  });

  // UI-07: a raw fetch to /api/v1 bypasses the generated types and the client's workspace,
  // session and csrf middleware. The gateway's own surfaces (/api/endpoint, /cs) are not the
  // Portal API and are read with fetch on purpose.
  it("names /api/v1 only through the typed client", () => {
    const raw = sources("src")
      .filter((path) => path !== "src/api/client.ts")
      .flatMap((path) =>
        fetchTargets(readFileSync(join(ui, path), "utf8"))
          .filter((target) => target.includes("/api/v1"))
          .map((target) => `${path}: fetch(${target})`),
      );
    expect(raw, "call these through `api` from src/api/client.ts").toEqual([]);
  });
});
