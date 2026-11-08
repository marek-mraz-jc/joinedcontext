// The reference copies of the deployment's dashboards for upstream Grafana 12.4.14: every
// panel's datasource rewritten to the built-in TestData source, scenario raw_frame, holding the
// panel's frame fixture (tests/fixtures/frames, panel-frames.mjs) in Grafana's own frame shape;
// the k6 run variable becomes a custom variable with the two runs the Portal's stub offers.
// Transformations, field config, options, time and refresh stay as saved.
//
//   node scripts/grafana-copies.mjs <dir>    writes <dir>/<dashboard>.json
import { mkdirSync, readdirSync, readFileSync, writeFileSync, existsSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const DASHBOARDS = join(here, "../tests/fixtures/dashboards");
const FRAMES = join(here, "../tests/fixtures/frames");
export const TESTDATA = { type: "grafana-testdata-datasource", uid: "testdata" };
/** The runs the k6 dashboard's `testid` variable offers on both sides. */
export const K6_RUNS = ["k6-2026-11-07T21", "k6-2026-11-08T05"];

/** A daiya frame as Grafana's DataFrame JSON: the display name as `displayNameFromDS`. */
export function grafanaFrame(frame) {
  return {
    name: frame.name,
    refId: frame.refId,
    fields: frame.fields.map((field) => ({
      name: field.name,
      type: field.type === "geo" || field.type === "json" ? "other" : field.type,
      labels: field.labels ?? {},
      config: {
        ...(field.displayName ? { displayNameFromDS: field.displayName } : {}),
        ...(field.unit ? { unit: field.unit } : {}),
      },
      values: field.values,
    })),
  };
}

/** The fixture a panel of dashboard file `stem` reads; a parity-* copy reads context-broker's. */
export function fixtureOf(stem, panel) {
  const own = join(FRAMES, `${stem}-${panel.id}.json`);
  if (existsSync(own)) return JSON.parse(readFileSync(own, "utf8"));
  const broker = join(FRAMES, `context-broker-${panel.id}.json`);
  return JSON.parse(readFileSync(broker, "utf8"));
}

export function copyOf(stem, dashboard) {
  const copy = structuredClone(dashboard);
  delete copy.id;
  copy.panels = (copy.panels ?? []).map((panel) => {
    if (!panel.targets?.length) return panel;
    const frames = fixtureOf(stem, panel).frames.map(grafanaFrame);
    return {
      ...panel,
      datasource: TESTDATA,
      targets: [{ refId: "A", datasource: TESTDATA, scenarioId: "raw_frame", rawFrameContent: JSON.stringify(frames) }],
    };
  });
  for (const variable of copy.templating?.list ?? []) {
    if (variable.type !== "query") continue;
    Object.assign(variable, {
      type: "custom",
      query: K6_RUNS.join(","),
      options: [],
      datasource: undefined,
      definition: undefined,
      refresh: undefined,
    });
  }
  return copy;
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  const out = process.argv[2];
  if (!out) throw new Error("usage: node scripts/grafana-copies.mjs <dir>");
  mkdirSync(out, { recursive: true });
  for (const file of readdirSync(DASHBOARDS).filter((f) => f.endsWith(".json"))) {
    const dashboard = JSON.parse(readFileSync(join(DASHBOARDS, file), "utf8"));
    writeFileSync(join(out, file), JSON.stringify(copyOf(file.replace(/\.json$/, ""), dashboard), null, 1));
  }
}
