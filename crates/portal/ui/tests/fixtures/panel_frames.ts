/**
 * Panel frames in the shape daiya's panel render route answers (`frameSets[].frames`, flattened by
 * the Portal with each frame's `refId`): a Prometheus range query of the context-broker dashboard,
 * the NGSI-LD history and table reads of the helsinki dashboard, and one frame per other panel
 * type daiya lists in SUPPORTED_PANELS.
 */
import type { Frame } from "../../src/components/panels/frames";

const T0 = Date.UTC(2026, 10, 8, 6, 0, 0);
const MIN = 60_000;
export const FROM = T0;
export const TO = T0 + 60 * MIN;
const times = (n: number, step = 5 * MIN) => Array.from({ length: n }, (_, i) => T0 + i * step);

/** context-broker: `sum by (status) (rate(http_requests_total[5m]))`, one frame per label set. */
export const requestRate: Frame[] = [
  {
    name: "status=200",
    refId: "A",
    fields: [
      { name: "Time", type: "time", values: times(12) },
      { name: "Value", type: "number", labels: { status: "200" }, values: [12.1, 12.4, 13.0, 12.8, 14.2, 15.9, 15.1, 14.7, 13.9, 13.3, 12.9, 12.6] },
    ],
  },
  {
    name: "status=500",
    refId: "A",
    fields: [
      { name: "Time", type: "time", values: times(12) },
      { name: "Value", type: "number", labels: { status: "500" }, values: [0, 0, 0.1, 0.4, 0.9, 0.2, 0, 0, null, 0, 0, 0] },
    ],
  },
];

/** helsinki: pm25 history of two stations (NGSI-LD temporal read). */
export const pm25History: Frame[] = [
  {
    name: "Kallio",
    refId: "A",
    fields: [
      { name: "observedAt", type: "time", values: times(6, 10 * MIN) },
      { name: "pm25", type: "number", unit: "µg/m³", displayName: "Kallio", values: [4.2, 5.1, 6.8, 7.4, 5.9, 5.0] },
    ],
  },
  {
    name: "Mannerheimintie",
    refId: "A",
    fields: [
      { name: "observedAt", type: "time", values: times(6, 10 * MIN) },
      { name: "pm25", type: "number", unit: "µg/m³", displayName: "Mannerheimintie", values: [9.8, 11.2, 14.5, 16.1, 12.0, 10.4] },
    ],
  },
];

/** helsinki: the current readings as a table (NGSI-LD entities read). */
export const stations: Frame[] = [
  {
    name: "AirQualityObserved",
    refId: "A",
    fields: [
      { name: "name", type: "string", values: ["Kallio", "Mannerheimintie", "Vartiokylä"] },
      { name: "pm25", type: "number", values: [5.0, 10.4, 26.3] },
      { name: "airQualityLevel", type: "string", values: ["good", "moderate", "poor"] },
      { name: "dateObserved", type: "time", values: [TO - 4 * MIN, TO - 2 * MIN, TO - 9 * MIN] },
    ],
  },
];

/** context-broker: `up` per instance, for stat, gauge and bargauge. */
export const up: Frame[] = [
  {
    name: "",
    refId: "A",
    fields: [
      { name: "Time", type: "time", values: times(4) },
      { name: "Value", type: "number", labels: { instance: "antares-0" }, values: [1, 1, 1, 1] },
    ],
  },
  {
    name: "",
    refId: "A",
    fields: [
      { name: "Time", type: "time", values: times(4) },
      { name: "Value", type: "number", labels: { instance: "antares-1" }, values: [1, 1, 0, 0] },
    ],
  },
];

/** Memory in bytes per pod, for a bargauge and a gauge with thresholds. */
export const memory: Frame[] = [
  {
    name: "",
    refId: "A",
    fields: [
      { name: "Time", type: "time", values: times(3) },
      { name: "Value", type: "number", labels: { pod: "broker" }, values: [310e6, 320e6, 334e6] },
      { name: "Value", type: "number", labels: { pod: "portal" }, values: [700e6, 760e6, 812e6] },
    ],
  },
];

/** Requests per route as categories, for a barchart and a piechart. */
export const routes: Frame[] = [
  {
    name: "",
    refId: "A",
    fields: [
      { name: "route", type: "string", values: ["/ngsi-ld/v1/entities", "/ngsi-ld/v1/temporal", "/q/health"] },
      { name: "requests", type: "number", values: [5120, 830, 2400] },
    ],
  },
];

/** `histogram_quantile` buckets of request duration, one field per `le`. */
export const durationBuckets: Frame[] = ["0.05", "0.1", "0.5", "1", "+Inf"].map((le, index) => ({
  name: `le=${le}`,
  refId: "A",
  fields: [
    { name: "Time", type: "time", values: times(4, 15 * MIN) },
    { name: "Value", type: "number", labels: { le }, values: [10, 20, 15, 5].map((v) => v * (5 - index)) },
  ],
}));

/** A pipeline's run state over the hour, for a state-timeline. */
export const runState: Frame[] = [
  {
    name: "",
    refId: "A",
    fields: [
      { name: "Time", type: "time", values: [T0, T0 + 20 * MIN, T0 + 35 * MIN, T0 + 50 * MIN] },
      { name: "air-quality", type: "string", values: ["ok", "failing", "ok", "running"] },
    ],
  },
];

/** A range with no readings: frames with fields and no values. */
export const empty: Frame[] = [
  {
    name: "",
    refId: "A",
    fields: [
      { name: "Time", type: "time", values: [] },
      { name: "Value", type: "number", values: [] },
    ],
  },
];
