/**
 * T-2136: every route of the Portal is opened by a spec (UI-15, UI-16, TS-19).
 *
 * A page with a test can still be a page nobody opens the way a person opens it. This gate reads
 * `src/router.tsx` and asks two things of every route that renders something: that one mocked
 * Playwright spec under `e2e/` opens it, and that one live journey under `e2e/live/` walks it.
 * What is missing today is in `gate_routes.allow.json` with what covers the page instead, and
 * that list only shrinks.
 *
 * A route that renders nothing because it redirects (UI-28 leaves the Portal no Federation page
 * and no playground) is held to a different rule: a vitest test has to name its address, because
 * the redirect is the whole behaviour.
 *
 * Matching an address to a route takes the most specific route that matches, the way the router
 * does. Without that, `/projects/$project/$plural` would stand in for every page of the Portal.
 */
import { readFileSync, readdirSync } from "node:fs";
import { join, resolve, dirname } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";
import { addressesIn, routeOf, routerPaths, routesOpenedBy, verdict } from "./gates";

const ui = resolve(dirname(fileURLToPath(import.meta.url)), "..");

/** What was missing when the gate was written; the lists may shrink, never grow. */
const ALLOWED_ON_2026_09_20 = { mocked: 1, live: 0 };

function specs(directory: string): Record<string, string> {
  return Object.fromEntries(
    readdirSync(join(ui, directory))
      .filter((name) => name.endsWith(".spec.ts"))
      .map((name) => [name, readFileSync(join(ui, directory, name), "utf8")]),
  );
}

function unitTests(): Record<string, string> {
  return Object.fromEntries(
    readdirSync(join(ui, "tests"))
      .filter((name) => /\.test\.tsx?$/.test(name))
      .map((name) => [name, readFileSync(join(ui, "tests", name), "utf8")]),
  );
}

const router = readFileSync(join(ui, "src/router.tsx"), "utf8");
const paths = routerPaths(router);

/** A route that renders nothing: its block declares no component and throws a redirect. */
function redirectOnly(path: string): boolean {
  const at = router.indexOf(`path: "${path}"`);
  const next = paths
    .map((other) => router.indexOf(`path: "${other}"`))
    .filter((index) => index > at)
    .sort((a, b) => a - b)[0];
  const block = router.slice(at, next === undefined ? router.length : next);
  return !block.includes("component:") && block.includes("redirect(");
}

/**
 * Routes a deployed Portal does not serve, so no live journey against an instance can walk one.
 * Excepted by name rather than by the allow-list, the way the module gate excepts the bundle
 * entry: the list is for what is owed and shrinks as it is paid, and this is never owed.
 * `/__gallery` is built only under `import.meta.env.DEV` (`src/router.tsx`); the mocked spec
 * opens it against the dev server, which is the only place it exists (T-1729).
 */
const DEVELOPMENT_ONLY = ["/__gallery"];

const rendering = paths.filter((path) => !redirectOnly(path));
const served = rendering.filter((path) => !DEVELOPMENT_ONLY.includes(path));
const redirecting = paths.filter(redirectOnly);
const allow = JSON.parse(readFileSync(join(ui, "tests/gate_routes.allow.json"), "utf8")) as {
  mocked: Record<string, { task?: string; why: string }>;
  live: Record<string, { task?: string; why: string }>;
};

describe("the route gate (T-2136)", () => {
  it("reads every route of the router", () => {
    expect(paths.length, "src/router.tsx declares no path").toBeGreaterThan(20);
    expect(paths).toContain("/");
    expect(new Set(paths).size, "two routes declare the same path").toBe(paths.length);
  });

  it("opens every rendering route in a mocked spec, or says what covers it instead", () => {
    const opened = routesOpenedBy(paths, specs("e2e"));
    const { missing, stale } = verdict(
      rendering,
      new Set(opened.keys()),
      Object.keys(allow.mocked),
    );
    expect(missing, "no mocked spec under e2e/ opens these routes").toEqual([]);
    expect(stale, "these are opened now, or gone: remove them from tests/gate_routes.allow.json").toEqual([]);
  });

  it("walks every rendering route in a live journey, or says why it cannot", () => {
    const opened = routesOpenedBy(paths, specs("e2e/live"));
    const { missing, stale } = verdict(served, new Set(opened.keys()), Object.keys(allow.live));
    expect(missing, "no live journey under e2e/live/ walks these routes").toEqual([]);
    expect(stale, "these are walked now, or gone: remove them from tests/gate_routes.allow.json").toEqual([]);
  });

  it("names every redirect-only route in a test, because the redirect is the behaviour", () => {
    const named = new Set(
      Object.values(unitTests()).flatMap((source) =>
        addressesIn(source).flatMap((address) => {
          const route = routeOf(address, paths);
          return route ? [route] : [];
        }),
      ),
    );
    expect(redirecting.length, "the Portal has redirect-only routes (UI-28)").toBeGreaterThan(0);
    expect(redirecting.filter((path) => !named.has(path)), "these redirects are asserted nowhere").toEqual([]);
  });

  it("keeps both allow-lists shrinking, each entry saying what covers the route", () => {
    for (const side of ["mocked", "live"] as const) {
      const entries = Object.entries(allow[side]);
      expect(entries.length).toBeLessThanOrEqual(ALLOWED_ON_2026_09_20[side]);
      for (const [route, entry] of entries) {
        expect(paths, `${side}: ${route} is no route of the router`).toContain(route);
        expect(
          entry.why.length,
          `${side}: ${route} says nothing about what covers it`,
        ).toBeGreaterThan(20);
        if (entry.task !== undefined) expect(entry.task).toMatch(/^T-\d{4}$/);
      }
    }
  });
});

describe("the rule the route gate applies", () => {
  const paths = ["/", "/login", "/projects/$project/models", "/projects/$project/$plural"];

  it("gives an address to the most specific route that matches it", () => {
    expect(routeOf("/projects/helsinki/models", paths)).toBe("/projects/$project/models");
    expect(routeOf("/projects/helsinki/pipelines", paths)).toBe("/projects/$project/$plural");
    expect(routeOf("/", paths)).toBe("/");
    expect(routeOf("/nothing/here/at/all", paths)).toBeUndefined();
  });

  it("does not let the generic route stand in for a page that has its own", () => {
    const opened = routesOpenedBy(paths, {
      "models.spec.ts": 'await page.goto("/projects/helsinki/models?lang=en");',
    });
    expect(opened.get("/projects/$project/models")).toEqual(["models.spec.ts"]);
    expect(opened.get("/projects/$project/$plural")).toBeUndefined();
  });

  it("is red for a route no spec opens and green once one does", () => {
    const none = routesOpenedBy(paths, {});
    expect(verdict(paths, new Set(none.keys()), []).missing).toEqual([...paths].sort());
    const one = routesOpenedBy(paths, { "login.spec.ts": 'page.goto("/login")' });
    expect(verdict(paths, new Set(one.keys()), []).missing).not.toContain("/login");
    expect(
      verdict(paths, new Set(one.keys()), ["/login"]).stale,
      "an entry for a route that is opened now has to go",
    ).toEqual(["/login"]);
  });

  it("reads an address out of any literal, and none out of a comment", () => {
    expect(addressesIn('page.goto("/a?lang=en"); const b = `/b`;')).toEqual(["/a", "/b"]);
    expect(addressesIn('// page.goto("/a")')).toEqual([]);
  });

  it("reads the router's paths and nothing else", () => {
    expect(routerPaths('createRoute({ path: "/x" });\n// path: "/commented"')).toEqual(["/x"]);
    const nested = [
      'const top = createRoute({ getParentRoute: () => rootRoute, id: "protected" });',
      'const list = createRoute({ getParentRoute: () => top, path: "/p/$plural" });',
      'const index = createRoute({ getParentRoute: () => list, path: "/" });',
      'const edit = createRoute({ getParentRoute: () => list, path: "$name/edit" });',
    ].join("\n");
    expect(routerPaths(nested)).toEqual(["/p/$plural", "/p/$plural/$name/edit"]);
  });
});
