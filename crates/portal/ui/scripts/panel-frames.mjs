// One frame fixture per panel of the deployment's dashboards (tests/fixtures/dashboards), in the
// shape daiya's datasources answer (crates/daiya/query: Prometheus range and instant-table
// frames, NGSI-LD temporal series and entity tables), for one fixed absolute range per dashboard
// (its saved range ending at TO). The Portal's dashboard e2e serves them from its panel route and
// the reference Grafana 12.4.14 reads the same frames through TestData raw_frame, so both sides
// draw the same numbers. Display names are the ones the real source sets: a Prometheus series is
// its legendFormat filled from its labels, an NGSI-LD series its entity's name.
//
//   node scripts/panel-frames.mjs        writes tests/fixtures/frames/<dashboard>-<panel>.json
//
// A frame's `served` member, when present, is what daiya answers after the panel's
// transformations (crates/daiya/api/tests/panel_fixtures.rs writes and checks it).
import { readdirSync, readFileSync, writeFileSync, existsSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const DASHBOARDS = join(here, "../tests/fixtures/dashboards");
const OUT = join(here, "../tests/fixtures/frames");

/** The end of every range: 2026-11-08 07:00 in Bratislava. */
export const TO = Date.UTC(2026, 10, 8, 6, 0, 0);
const UNIT = { m: 60_000, h: 3_600_000, d: 86_400_000 };

/** The saved range of `dashboard` as absolute epoch milliseconds ending at TO. */
export function rangeOf(dashboard) {
  const match = /^now-(\d+)([mhd])$/.exec(dashboard.time?.from ?? "now-6h");
  const span = match ? Number(match[1]) * UNIT[match[2]] : 6 * UNIT.h;
  return { from: TO - span, to: TO };
}

/** A small seeded generator, so every run writes the same numbers. */
function random(seed) {
  let s = seed >>> 0 || 1;
  return () => {
    s = (s * 1664525 + 1013904223) >>> 0;
    return s / 2 ** 32;
  };
}
const hash = (text) => [...text].reduce((h, c) => (h * 31 + c.charCodeAt(0)) >>> 0, 7);

/** The label values a placeholder of a legendFormat takes. */
const LABELS = {
  status: ["2xx", "4xx", "5xx"],
  code: ["200", "404", "503"],
  method: ["GET", "POST", "PATCH"],
  route: ["/ngsi-ld/v1/entities", "/ngsi-ld/v1/temporal/entities", "/api/v1/projects"],
  kind: ["entities", "temporal", "geo"],
  standard: ["ngsi-ld", "ogc", "mcp"],
  pod: ["antares-0", "antares-1"],
  verdict: ["permit", "deny"],
  operation: ["retrieveEntity", "queryEntity"],
  lane: ["manual", "assistant"],
  type: ["Endpoint", "Pipeline"],
  part: ["prompt", "completion"],
  tool: ["jc_data_query", "jc_change_plan"],
  state: ["active", "idle"],
  reached: ["yes", "no"],
  scheme: ["http", "https"],
  target: ["portal", "gateway"],
  surface: ["ui", "api"],
  host: ["dev.joinedcontext.com", "api.dev.joinedcontext.com"],
  domain: ["joinedcontext.com"],
};

/** A level and a swing that look like the unit. */
function scale(unit) {
  switch (unit) {
    case "s":
      return [0.08, 0.05];
    case "ms":
      return [40, 25];
    case "bytes":
      return [3.2e8, 4e7];
    case "Bps":
      return [2.4e5, 1e5];
    case "percentunit":
      return [0.6, 0.3];
    case "d":
      return [45, 30];
    case "locale":
      return [1800, 700];
    case "none":
      return [9, 5];
    case "short":
      return [120, 80];
    default:
      return [14, 9];
  }
}

/** Every label set a legendFormat names, as the product of its placeholders' values. */
function labelSets(legend) {
  const names = [...(legend ?? "").matchAll(/\{\{\s*(\w+)\s*\}\}/g)].map((m) => m[1]);
  let sets = [{}];
  for (const name of names) {
    const values = LABELS[name] ?? ["a", "b"];
    sets = sets.flatMap((set) => values.map((value) => ({ ...set, [name]: value })));
  }
  return sets.slice(0, 6);
}

const fill = (legend, labels) => (legend ?? "").replace(/\{\{\s*(\w+)\s*\}\}/g, (_, name) => labels[name] ?? "");

/** One reading series: a wave with noise, rounded to what the source keeps, with a gap now and then. */
function readings(n, unit, seed, gap) {
  const rnd = random(seed);
  const [level, swing] = scale(unit);
  const phase = rnd() * 6;
  return Array.from({ length: n }, (_, i) => {
    if (gap && i % 17 === 11) return null;
    const v = level + swing * Math.sin(i / 5 + phase) + (swing / 3) * (rnd() - 0.5);
    const clamped = unit === "percentunit" ? Math.min(1, Math.max(0, v)) : Math.max(0, v);
    return Number(clamped.toPrecision(5));
  });
}

/** Prometheus range-query frames of one target, as daiya's build_prom_frames writes them. */
function promSeries(target, unit, range, step, seed) {
  const times = [];
  for (let t = range.from; t <= range.to; t += step) times.push(t);
  return labelSets(target.legendFormat).map((labels, index) => {
    const name = fill(target.legendFormat, labels) || target.expr;
    return {
      name,
      refId: target.refId,
      fields: [
        { name: "Time", type: "time", labels: {}, values: times },
        { name, type: "number", displayName: name, labels, values: readings(times.length, unit, seed + index * 101, index === 1) },
      ],
    };
  });
}

/** A Prometheus instant query with `format: table`, as daiya's build_prom_table_frame writes it. */
function promTable(target, unit, range, seed) {
  // An instant table has a column per label the query keeps: its `by (...)`, else its legend's.
  const by = /\bby\s*\(([^)]*)\)/.exec(target.expr ?? "")?.[1];
  const sets = labelSets(by ? by.split(",").map((name) => `{{${name.trim()}}}`).join(" ") : target.legendFormat);
  const keys = [...new Set(sets.flatMap((set) => Object.keys(set)))].sort();
  const values = readings(sets.length, unit, seed, false);
  return {
    name: target.refId,
    refId: target.refId,
    fields: [
      { name: "Time", type: "time", labels: {}, values: sets.map(() => range.to) },
      ...keys.map((key) => ({ name: key, type: "string", labels: {}, values: sets.map((set) => set[key] ?? null) })),
      { name: `Value #${target.refId}`, type: "number", labels: {}, values },
    ],
  };
}

/** The bar gauges' `sum(increase(...[24h])) by (x)`: one series per label value, whole counts. */
function promCounts(target, range, step, seed) {
  return promSeries(target, "short", range, step, seed).map((frame) => ({
    ...frame,
    fields: [frame.fields[0], { ...frame.fields[1], values: frame.fields[1].values.map((v) => (v === null ? null : Math.round(v))) }],
  }));
}

const STATIONS = {
  TrafficFlowObserved: ["Baana", "Eläintarhanlahti", "Kulosaaren silta", "Lauttasaaren silta", "Huopalahdentie"],
  AirQualityObserved: ["Mäkelänkatu", "Kallio 2", "Vartiokylä Huivipolku", "Länsisatama 4"],
  BikeHireDockingStation: ["Kaivopuisto", "Laivasillankatu", "Kapteeninpuistikko", "Viiskulma", "Sepänkatu", "Hietalahdentori"],
};

/** NGSI-LD temporal series, one frame per entity, named by its `name` (namePattern entity_name). */
function ngsiSeries(target, unit, range, seed) {
  const names = STATIONS[target.entityType] ?? ["A", "B"];
  const aggregated = target.aggrPeriodDuration === "PT6H";
  const step = aggregated ? 6 * UNIT.h : UNIT.d;
  const attr = target.attributes?.[0] ?? "value";
  return names.map((label, index) => {
    const times = [];
    for (let t = range.from + step; t <= range.to; t += step) times.push(t);
    const values = readings(times.length, unit, seed + index * 37, index === 2).map((v) => (v === null || unit !== "locale" ? v : Math.round(v)));
    const id = `urn:ngsi-ld:${target.entityType}:${label.toLowerCase().replace(/[^a-z0-9]+/g, "-")}`;
    const suffix = aggregated ? ` (${target.aggrMethod})` : "";
    return {
      name: label,
      refId: target.refId,
      fields: [
        { name: "Time", type: "time", labels: {}, values: times },
        {
          name: attr,
          type: "number",
          displayName: `${label}${suffix}`,
          labels: { entity: id, type: target.entityType, ...(aggregated ? { aggregation: target.aggrMethod } : {}) },
          values,
        },
      ],
    };
  });
}

const OKRESY = ["Banskobystrický kraj, okres Banská Bystrica", "Banskobystrický kraj, okres Brezno", "Banskobystrický kraj, okres Detva", "Banskobystrický kraj, okres Lučenec", "Banskobystrický kraj, okres Zvolen"];

/** An NGSI-LD entity table, as daiya's entities_to_table_frame writes it: id, type, observedAt, attributes. */
function ngsiTable(target, range, seed) {
  const rnd = random(seed);
  const type = target.entityType;
  const attributes = target.attributes ?? [];
  if (type === "KeyPerformanceIndicator") {
    const ids = OKRESY.map((_, i) => `urn:ngsi-ld:KeyPerformanceIndicator:${target.idPattern.replace(/[.*:]/g, "")}${i}`);
    return [
      {
        name: target.refId,
        refId: target.refId,
        fields: [
          { name: "id", type: "string", labels: {}, values: ids },
          { name: "type", type: "string", labels: {}, values: ids.map(() => type) },
          { name: "observedAt", type: "time", unit: "dateTimeAsIso", labels: {}, values: ids.map(() => range.to - 86_400_000) },
          { name: "title", type: "string", labels: {}, values: OKRESY },
          {
            name: "currentValue",
            type: "number",
            labels: {},
            values: target.idPattern.includes("emisie") ? OKRESY.map(() => Number((rnd() * 3).toFixed(4))) : OKRESY.map(() => Math.round(20000 + rnd() * 90000)),
          },
        ],
      },
    ];
  }
  const names = STATIONS[type] ?? ["A"];
  const ids = names.map((n) => `urn:ngsi-ld:${type}:${n.toLowerCase().replace(/[^a-z0-9]+/g, "-")}`);
  const column = (attr) => {
    switch (attr) {
      case "name":
        return { name: attr, type: "string", labels: {}, values: names };
      case "status":
        return { name: attr, type: "string", labels: {}, values: names.map((_, i) => ["working", "outOfService", "working", "withIncidence", "working", "working"][i % 6]) };
      case "pm25":
        return { name: attr, type: "number", labels: {}, values: names.map((_, i) => [27.4, 12.06, 6.3, 48.75][i % 4]) };
      case "pm10":
        return { name: attr, type: "number", labels: {}, values: names.map((_, i) => [41.0, 19.6, 11.24, null][i % 4]) };
      default:
        return { name: attr, type: "number", labels: {}, values: names.map(() => Math.round(rnd() * 20)) };
    }
  };
  return [
    {
      name: target.refId,
      refId: target.refId,
      fields: [
        { name: "id", type: "string", labels: {}, values: ids },
        { name: "type", type: "string", labels: {}, values: ids.map(() => type) },
        { name: "observedAt", type: "time", unit: "dateTimeAsIso", labels: {}, values: ids.map((_, i) => range.to - (i + 1) * 240_000) },
        ...attributes.map(column),
      ],
    },
  ];
}

const span = (text) => {
  const match = /^(\d+)([mhd])$/.exec(text ?? "");
  return match ? Number(match[1]) * UNIT[match[2]] : 0;
};

/** The range a panel reads: the dashboard's, or `now-timeFrom` to now, both ends `timeShift` back. */
// The shots open every dashboard on an absolute range (?from=&to=), and Grafana 12.4 applies a
// panel's timeFrom only under a relative dashboard range: here only timeShift moves the panel.
export function panelRange(dashboard, panel) {
  const range = rangeOf(dashboard);
  const from = range.from;
  const shift = span(panel.timeShift);
  return { from: from - shift, to: range.to - shift };
}

/** The frames one panel's datasource answers. */
export function framesOf(dashboard, panel) {
  const range = panelRange(dashboard, panel);
  const unit = panel.fieldConfig?.defaults?.unit;
  const span = range.to - range.from;
  const step = Math.max(60_000, Math.round(span / 60 / 60_000) * 60_000);
  const seed = hash(`${dashboard.uid}/${panel.id}`);
  return (panel.targets ?? []).flatMap((target, index) => {
    const s = seed + index * 7919;
    if (target.queryType === "temporal") return ngsiSeries(target, unit, range, s);
    if (target.queryType === "entity") return ngsiTable(target, range, s);
    if (target.format === "table") return [promTable(target, unit, range, s)];
    if (panel.type === "bargauge") return promCounts(target, range, step, s);
    if (panel.type === "stat") return promSeries({ ...target, legendFormat: target.legendFormat }, "none", range, step, s).map((f) => ({ ...f, fields: [f.fields[0], { ...f.fields[1], values: f.fields[1].values.map(() => 1) }] }));
    return promSeries(target, unit, range, step, s);
  });
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  for (const file of readdirSync(DASHBOARDS).filter((f) => f.endsWith(".json"))) {
    const dashboard = JSON.parse(readFileSync(join(DASHBOARDS, file), "utf8"));
    // The parity-* fixture dashboards are copies of context-broker and read its frames, but for
    // a panel with a time override of its own, which reads its own range.
    const copy = dashboard.uid.startsWith("parity-");
    const stem = file.replace(/\.json$/, "");
    for (const panel of dashboard.panels ?? []) {
      if (!panel.targets?.length || (copy && !panel.timeFrom && !panel.timeShift)) continue;
      const path = join(OUT, `${stem}-${panel.id}.json`);
      const previous = existsSync(path) ? JSON.parse(readFileSync(path, "utf8")) : {};
      const fixture = {
        dashboard: dashboard.uid,
        panel: panel.id,
        title: panel.title,
        ...panelRange(dashboard, panel),
        frames: framesOf(dashboard, panel),
        ...(previous.served ? { served: previous.served } : {}),
      };
      writeFileSync(path, `${JSON.stringify(fixture, null, 1)}\n`);
    }
  }
}
