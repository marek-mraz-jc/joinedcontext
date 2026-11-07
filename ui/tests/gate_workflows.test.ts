/**
 * T-2728: every step of every workflow has a live journey and an assistant tool, or names the
 * task that owes it (TS-26, AG-87).
 *
 * `workflow_coverage.json` is the matrix of `Testing/07-workflow-coverage.md` in the docs,
 * machine-read. This gate holds it to the tree: a live journey it names has to exist with that
 * title, a tool it names has to be registered, every mutating route of `openapi.json` has to
 * belong to a step or say why it belongs to none, and the owed entries only ever shrink. A
 * refusal step (no permission, a red verdict, a secret typed in) is a step like any other, so it
 * is gated exactly as the success it refuses.
 */
import { readFileSync, readdirSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";
import { localImports, mutatingRoutes, parseCoverage, testTitles, titleIn, withoutComments } from "./gates";
import type { Coverage, CoverageStep } from "./gates";

const ui = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const portal = resolve(ui, "..");

/** What was owed when the gate was written; the counts may shrink, never grow. */
const OWED_ON_2026_09_25 = { live: 4, assistant: 1, unclaimed: 5 };

const coverage = JSON.parse(readFileSync(join(ui, "tests/workflow_coverage.json"), "utf8")) as Coverage;
const openapi = JSON.parse(readFileSync(join(ui, "openapi.json"), "utf8")) as {
  paths: Record<string, Record<string, unknown>>;
};

function liveSpecs(): Record<string, string> {
  const directory = join(ui, "e2e/live");
  return Object.fromEntries(
    readdirSync(directory)
      .filter((name) => name.endsWith(".ts"))
      .map((name) => [name, readFileSync(join(directory, name), "utf8")]),
  );
}

/** Every operation the registry holds and every tool a conversation reads with. */
function registeredTools(): Set<string> {
  const sources = [
    ...readdirSync(join(portal, "src/ops")).map((name) => join(portal, "src/ops", name)),
    join(portal, "src/agents/capabilities.rs"),
  ];
  const names = sources.flatMap((file) =>
    [...readFileSync(file, "utf8").matchAll(/"(jc_[a-z_]+|describe_tool|search_catalog)"/g)].map((match) => match[1]),
  );
  return new Set(names);
}

function steps(data: Coverage): [string, CoverageStep][] {
  return Object.entries(data.workflows).flatMap(([workflow, byStep]) =>
    Object.entries(byStep).map(([step, entry]) => [`${workflow}/${step}`, entry] as [string, CoverageStep]),
  );
}

/** Everything wrong with a matrix against a tree, one line each, so a red gate says what to fix. */
function problems(
  data: Coverage,
  specs: Record<string, string>,
  tools: ReadonlySet<string>,
  routes: readonly string[],
): string[] {
  const found: string[] = [];
  const claimed = new Set<string>();
  for (const [name, step] of steps(data)) {
    if ((step.live === undefined) === (step.liveOwed === undefined)) {
      found.push(`${name}: names a live journey or the task that owes one, exactly one of the two`);
    }
    if (step.live) {
      const spec = specs[step.live.spec];
      if (spec === undefined) {
        found.push(`${name}: e2e/live/${step.live.spec} does not exist`);
      } else {
        const helpers = localImports(spec).flatMap((file) => (specs[file] === undefined ? [] : [specs[file]]));
        if (!titleIn(step.live.test, spec, helpers)) {
          found.push(`${name}: e2e/live/${step.live.spec} has no test "${step.live.test}"`);
        }
      }
    }
    const ways = [step.assistant, step.assistantOwed, step.personOnly].filter((way) => way !== undefined);
    if (ways.length !== 1) {
      found.push(`${name}: names an assistant tool, the task that owes one, or why only a person may, exactly one`);
    }
    if (step.assistant !== undefined && !tools.has(step.assistant)) {
      found.push(`${name}: ${step.assistant} is no registered tool`);
    }
    for (const owed of [step.liveOwed, step.assistantOwed]) {
      if (owed !== undefined && !/^T-\d{4}$/.test(owed)) found.push(`${name}: "${owed}" is no task id`);
    }
    if (step.personOnly !== undefined && step.personOnly.length < 20) {
      found.push(`${name}: says too little about why only a person may take it`);
    }
    for (const route of step.routes ?? []) {
      if (!routes.includes(route)) found.push(`${name}: ${route} is no mutating route of openapi.json`);
      claimed.add(route);
    }
  }
  for (const route of routes) {
    if (!claimed.has(route) && data.unclaimed[route] === undefined) {
      found.push(`${route} belongs to no workflow step and is not in "unclaimed"`);
    }
  }
  for (const [route, entry] of Object.entries(data.unclaimed)) {
    if (!routes.includes(route)) found.push(`unclaimed: ${route} is no mutating route any more; remove it`);
    else if (claimed.has(route)) found.push(`unclaimed: ${route} belongs to a step now; remove it`);
    if (entry.why.length < 20) found.push(`unclaimed: ${route} says too little about why no step owns it`);
  }
  return found;
}

function owed(data: Coverage) {
  const all = steps(data).map(([, step]) => step);
  return {
    live: all.filter((step) => step.liveOwed !== undefined).length,
    assistant: all.filter((step) => step.assistantOwed !== undefined).length,
    unclaimed: Object.keys(data.unclaimed).length,
  };
}

describe("the workflow coverage gate (T-2728, TS-26)", () => {
  const routes = mutatingRoutes(openapi);

  it("reads the matrix and the tree it is held to", () => {
    expect(Object.keys(coverage.workflows).length, "the matrix names no workflow").toBeGreaterThan(10);
    expect(routes.length, "openapi.json has no mutating route").toBeGreaterThan(20);
    expect(registeredTools().has("jc_resource_propose"), "the registry was not read").toBe(true);
  });

  it("finds every live journey, every tool and every route the matrix names", () => {
    expect(problems(coverage, liveSpecs(), registeredTools(), routes)).toEqual([]);
  });

  it("keeps the owed entries shrinking", () => {
    const now = owed(coverage);
    for (const side of ["live", "assistant", "unclaimed"] as const) {
      expect(now[side], `${side}: more is owed than on 2026-09-25`).toBeLessThanOrEqual(OWED_ON_2026_09_25[side]);
    }
  });
});

describe("the rule the workflow gate applies", () => {
  const specs = {
    "kindJourney.ts": "export function kindJourney() { test(`${kind}: created and removed`, async () => {}); }",
    "kind-subscriptions.spec.ts": 'import { kindJourney } from "./kindJourney";\nkindJourney({ kind: "Subscription" });',
    "load.spec.ts": 'test("a data source, checked and proposed", async () => {});',
  };
  const tools = new Set(["jc_resource_propose"]);
  const routes = ["POST /api/v1/projects/{project}/{plural}", "POST /api/v1/auth/logout"];
  const good: Coverage = {
    workflows: {
      subscription: {
        create: {
          live: { spec: "kind-subscriptions.spec.ts", test: "Subscription: created and removed" },
          assistant: "jc_resource_propose",
          routes: ["POST /api/v1/projects/{project}/{plural}"],
        },
        approve: { liveOwed: "T-2729", personOnly: "a person approves a change, never an agent (AG-11)" },
      },
    },
    unclaimed: { "POST /api/v1/auth/logout": { why: "signing out is no workflow step, it ends the session" } },
  };
  const variant = (patch: (data: Coverage) => void): Coverage => {
    const copy = structuredClone(good);
    patch(copy);
    return copy;
  };

  it("is green on a matrix that holds, through a helper's template title", () => {
    expect(problems(good, specs, tools, routes)).toEqual([]);
  });

  it("is red on a spec that is gone and on a title no spec declares", () => {
    const gone = variant((d) => (d.workflows.subscription.create.live = { spec: "gone.spec.ts", test: "x" }));
    expect(problems(gone, specs, tools, routes)).toEqual(["subscription/create: e2e/live/gone.spec.ts does not exist"]);
    const renamed = variant((d) => (d.workflows.subscription.create.live = { spec: "kind-subscriptions.spec.ts", test: "Subscription: renamed" }));
    expect(problems(renamed, specs, tools, routes)[0]).toMatch(/has no test "Subscription: renamed"/);
  });

  it("does not let a template stand in for a value the spec never names", () => {
    const other = variant((d) => (d.workflows.subscription.create.live = { spec: "kind-subscriptions.spec.ts", test: "Dashboard: created and removed" }));
    expect(problems(other, specs, tools, routes)[0]).toMatch(/has no test "Dashboard/);
  });

  it("is red on a mutating route no step claims, and on an unclaimed entry a step claims", () => {
    const unmapped = variant((d) => delete d.workflows.subscription.create.routes);
    expect(problems(unmapped, specs, tools, routes)).toEqual([
      'POST /api/v1/projects/{project}/{plural} belongs to no workflow step and is not in "unclaimed"',
    ]);
    const both = variant((d) => d.workflows.subscription.create.routes?.push("POST /api/v1/auth/logout"));
    expect(problems(both, specs, tools, routes)).toEqual(["unclaimed: POST /api/v1/auth/logout belongs to a step now; remove it"]);
  });

  it("is red on a step with no tool and no owing task, and on a tool nobody registered", () => {
    const none = variant((d) => delete d.workflows.subscription.approve.personOnly);
    expect(problems(none, specs, tools, routes)[0]).toMatch(/subscription\/approve: names an assistant tool/);
    const unknown = variant((d) => (d.workflows.subscription.create.assistant = "jc_make_it_so"));
    expect(problems(unknown, specs, tools, routes)).toEqual(["subscription/create: jc_make_it_so is no registered tool"]);
  });

  it("is red on a step that claims a journey and owes it at once", () => {
    const both = variant((d) => (d.workflows.subscription.create.liveOwed = "T-2729"));
    expect(problems(both, specs, tools, routes)[0]).toMatch(/exactly one of the two/);
  });

  it("reads a title in each quoting, and skips one inside a comment", () => {
    const source = "test('single', f);\ntest(\"double\", f);\n// test(\"commented\", f);\ntest.fixme(`tpl ${x}`, f);";
    expect(testTitles(source).map(String)).toEqual(["/^single$/", "/^double$/", "/^tpl (.+?)$/"]);
    expect(withoutComments("a // b")).toBe("a ");
  });
});

describe("reading the matrix from the docs page (T-2728)", () => {
  const page = [
    "## 3. Workflows",
    "",
    "### 3.1 Subscription (`subscription`)",
    "",
    "| step | unit | API | mocked UI | live journey | assistant |",
    "|---|---|---|---|---|---|",
    '| `create` | — | `tests/x_tests.rs` | — | `ui/e2e/live/kind-subscriptions.spec.ts` › "Subscription: created and removed" | `jc_resource_propose` |',
    "| `approve` | — | — | — | owed: T-2729 | person only: a person approves a change, never an agent (AG-11) |",
    "",
    "## 4. Routes",
    "",
    "| route | steps |",
    "|---|---|",
    "| `POST /api/v1/projects/{project}/{plural}` | `subscription/create` |",
    "| `POST /api/v1/auth/logout` | none: signing out is no workflow step, it ends the session |",
  ].join("\n");

  it("reads steps, their journeys, their tools and the routes they own", () => {
    expect(parseCoverage(page)).toEqual({
      workflows: {
        subscription: {
          create: {
            live: { spec: "kind-subscriptions.spec.ts", test: "Subscription: created and removed" },
            assistant: "jc_resource_propose",
            routes: ["POST /api/v1/projects/{project}/{plural}"],
          },
          approve: { liveOwed: "T-2729", personOnly: "a person approves a change, never an agent (AG-11)" },
        },
      },
      unclaimed: { "POST /api/v1/auth/logout": { why: "signing out is no workflow step, it ends the session" } },
    });
  });

  it("refuses a row it cannot read, with its line, instead of dropping it", () => {
    expect(() => parseCoverage(page.replace("owed: T-2729", "later"))).toThrow(/line 8: the live cell/);
    expect(() => parseCoverage(page.replace("`subscription/create`", "`space/create`"))).toThrow(
      /line 14: space\/create is no step/,
    );
    expect(() => parseCoverage(page.replace("| `jc_resource_propose` |", "|"))).toThrow(/line 7: a step row has six cells/);
  });

  it("is the matrix the docs page holds, as recorded", () => {
    // `pnpm record:workflows <docs>/Testing/07-workflow-coverage.md` writes the file; this holds
    // its shape, and the gate above holds it to the tree.
    for (const byStep of Object.values(coverage.workflows)) {
      expect(Object.keys(byStep).length).toBeGreaterThan(0);
    }
  });
});
