import type { Row } from "@joinedcontext/sdk";

/**
 * Four indicators shaped like the city's (helsinki-kpi on dev), with a history each would have:
 * bikes available with a daily rhythm and one reading far off it, docking stations rising day by
 * day, a counter falling hour by hour, and virtual stations with no history at all. Times in 2030.
 */
const PREFIX = "urn:ngsi-ld:KeyPerformanceIndicator:hel.fi:helsinki-kpi:";
const HOUR = 3_600_000;
const START = Date.UTC(2030, 9, 1);

const kpi = (local: string, name: string, currentValue: number, calculationFormula: string): Row =>
  ({
    id: `${PREFIX}${local}`,
    type: "KeyPerformanceIndicator",
    name,
    currentValue,
    calculationFormula,
    source: "https://gbfs.example.test/helsinki/station_status.json",
  }) as Row;

export const KPIS: Row[] = [
  kpi("bikes-available-sum", "Bikes available in the city bike network", 3848, "sum(num_bikes_available) over station_status"),
  kpi("bike-network-stations", "Docking stations in the Helsinki city bike network", 476, "count(station_id) over station_information"),
  kpi("demo-counter-1", "Demo counter", 71, "a counter the demo pipeline counts down"),
  kpi("bike-network-virtual", "Virtual stations in the city bike network", 0, "count(station_id) where is_virtual_station"),
];

/** A small, repeatable wobble, so no series is unnaturally exact. */
const wobble = (i: number) => (((i * 37) % 11) - 5) / 10;

function series(local: string, points: [number, number][]) {
  return {
    id: `${PREFIX}${local}`,
    type: "KeyPerformanceIndicator",
    currentValue: { type: "Property", values: points.map(([t, v]) => [v, new Date(t).toISOString()]) },
  };
}

/** The bikes reading at 2030-10-03 12:00 UTC is far off the rhythm: the one point that looks wrong. */
export const SPIKE = START + 2 * 24 * HOUR + 12 * HOUR;

export const HISTORY = [
  series(
    "bikes-available-sum",
    Array.from({ length: 24 * 5 }, (_, i) => {
      const t = START + i * HOUR;
      const v = (i % 24 >= 7 && i % 24 < 19 ? 3000 : 3800) + wobble(i) * 20;
      return [t, t === SPIKE ? 1200 : v] as [number, number];
    }),
  ),
  series(
    "bike-network-stations",
    Array.from({ length: 20 }, (_, i) => [START - 15 * 24 * HOUR + i * 24 * HOUR, 456 + i + wobble(i) * 0.4] as [number, number]),
  ),
  series(
    "demo-counter-1",
    Array.from({ length: 30 }, (_, i) => [START + i * HOUR, 100 - i + wobble(i) * 0.2] as [number, number]),
  ),
];
