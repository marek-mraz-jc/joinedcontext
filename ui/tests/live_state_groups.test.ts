/**
 * T-6404: the live sweep runs specs on several workers, so two specs that hold one fixed object, or a
 * spec that grants a demo person a right, must not run side by side. e2e/live/stateGroups.ts names
 * the groups; this reads every spec and refuses a map that lets such specs overlap.
 */
// covers: e2e/live/stateGroups.ts
import { readFileSync, readdirSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it } from "vitest";
import { EXCLUSIVE, GROUPS, NARROW_GRANTS, SHARED_SEEDS, projects } from "../e2e/live/stateGroups";

const LIVE = join(__dirname, "../e2e/live");
const SPECS = readdirSync(LIVE).filter((file) => file.endsWith(".spec.ts"));
const text = (spec: string) => readFileSync(join(LIVE, spec), "utf8");

/** The group a spec runs in: "exclusive", a group's name, or the spec alone. */
function groupOf(spec: string): string {
  if (EXCLUSIVE.specs.includes(spec)) return "exclusive";
  return Object.entries(GROUPS).find(([, group]) => group.specs.includes(spec))?.[0] ?? `free:${spec}`;
}

/**
 * The fixed object names a spec holds: a top-level constant set to a literal resource name or
 * NGSI-LD URN (`const APP = "helsinki-alerts"`), and every task prefix it names (`t1585-`).
 */
function fixedNames(source: string): string[] {
  const names = new Set<string>();
  for (const [, value] of source.matchAll(/^const [A-Z_]+ = ["'`]([^"'`$]+)["'`];/gm)) {
    if (/^[a-z][a-z0-9-]{2,62}$/.test(value) || /^urn:ngsi-ld:[A-Za-z]+:\S+$/.test(value)) names.add(value);
  }
  for (const [prefix] of source.matchAll(/\bt[0-9]{4}[a-z]?-/g)) names.add(prefix);
  return [...names];
}

/** Whether a spec writes a demo person into a grant: a binding's subject or a group's member. */
function grantsDemoPerson(source: string): boolean {
  return /\b(subjects|members): \[\{ user: [A-Z]+\.user/.test(source);
}

describe("fixedNames and grantsDemoPerson", () => {
  it("find a literal name, a URN and a task prefix, and leave templates and paths out", () => {
    const source = [
      'const APP = "helsinki-alerts";',
      'const TARGET = "urn:ngsi-ld:Endpoint:hel.fi:helsinki:helsinki-all";',
      "const NAME = `t1585-${Date.now()}`;",
      'const P = "/projects/helsinki";',
      'const TYPE = "BikeHireDockingStation";',
    ].join("\n");
    expect(fixedNames(source).sort()).toEqual(["helsinki-alerts", "t1585-", "urn:ngsi-ld:Endpoint:hel.fi:helsinki:helsinki-all"]);
    expect(grantsDemoPerson("spec: { members: [{ user: VIEWER.user }] }")).toBe(true);
    expect(grantsDemoPerson("spec: { members: [{ user: PERSON.email }] }")).toBe(false);
  });
});

describe("the live state groups", () => {
  it("name only specs that exist, each in one group", () => {
    const listed = [...EXCLUSIVE.specs, ...Object.values(GROUPS).flatMap((group) => group.specs)];
    expect(listed.filter((spec) => !SPECS.includes(spec)), "listed specs that do not exist").toEqual([]);
    expect(listed.filter((spec, i) => listed.indexOf(spec) !== i), "specs in two groups").toEqual([]);
    expect(Object.keys(NARROW_GRANTS).filter((spec) => !SPECS.includes(spec))).toEqual([]);
  });

  it("keep two specs that hold one fixed name in one group, or one of them exclusive", () => {
    const holders = new Map<string, string[]>();
    for (const spec of SPECS) for (const name of fixedNames(text(spec))) holders.set(name, [...(holders.get(name) ?? []), spec]);
    const clashes = [...holders]
      .filter(([name]) => !(name in SHARED_SEEDS))
      .map(([name, specs]) => [name, specs.filter((spec) => groupOf(spec) !== "exclusive")] as const)
      .filter(([, specs]) => new Set(specs.map(groupOf)).size > 1)
      .map(([name, specs]) => `${name}: ${specs.join(", ")}`);
    expect(clashes, "put these specs in one group of e2e/live/stateGroups.ts").toEqual([]);
  });

  it("run a spec that grants a demo person a right alone, unless the grant reaches nothing shared", () => {
    const loose = SPECS.filter((spec) => grantsDemoPerson(text(spec)) && groupOf(spec) !== "exclusive" && !(spec in NARROW_GRANTS));
    expect(loose, "make these exclusive or name why in NARROW_GRANTS").toEqual([]);
  });

  it("give every group one worker and run the exclusive specs after all others", () => {
    const built = projects({});
    expect(built.at(-1)).toMatchObject({ name: "exclusive", workers: 1 });
    expect(built.slice(0, -1).every((project) => project.teardown === "exclusive")).toBe(true);
    expect(built.filter((project) => project.name in GROUPS).every((project) => project.workers === 1)).toBe(true);
  });
});
