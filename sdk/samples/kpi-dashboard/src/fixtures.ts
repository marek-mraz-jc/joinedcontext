import type { Row } from "@joinedcontext/sdk";

const MONTHS = ["2025-10", "2025-11", "2025-12", "2026-01", "2026-02", "2026-03", "2026-04", "2026-05", "2026-06", "2026-07", "2026-08", "2026-09"];

function series(slug: string, name: string, category: string, unit: string, values: number[], target?: number, higherIsBetter = true): Row[] {
  return values.map((kpiValue, month) => ({
    id: `urn:ngsi-ld:KeyPerformanceIndicator:hel.fi:indicators:${slug}-${MONTHS[month]}`,
    type: "KeyPerformanceIndicator",
    name,
    category,
    unit,
    kpiValue,
    ...(target === undefined ? {} : { target }),
    higherIsBetter,
    dateObserved: `${MONTHS[month]}-01T00:00:00Z`,
  }));
}

/**
 * Twelve monthly observations of five indicators, as the endpoint answers them: two short of
 * target, one where lower is better, one ahead but slipping, and one with no target at all.
 */
export const ROWS: Row[] = [
  ...series("cycling", "Cycling share of trips", "Mobility", "%", [15, 15.4, 15.9, 16.3, 16.8, 17.1, 17.6, 18, 18.3, 18.7, 19, 19.2], 20),
  ...series("punctuality", "Bus punctuality", "Mobility", "%", [89, 90.2, 90.8, 91.4, 91, 90.6, 90.1, 89.9, 88.4, 88.9, 88.7, 89], 90),
  ...series("pm25", "PM2.5 mean", "Environment", "µg/m³", [12.1, 11.8, 11.9, 11.2, 10.9, 10.6, 10.4, 10.1, 9.9, 9.8, 9.6, 9.4], 10, false),
  ...series("recycling", "Recycling rate", "Environment", "%", [60, 59.6, 59.8, 59.1, 58.7, 58.9, 58.2, 57.9, 57.6, 57.4, 57.2, 57], 55),
  ...series("satisfaction", "Resident satisfaction", "Services", "/ 5", [3.6, 3.6, 3.7, 3.7, 3.6, 3.8, 3.8, 3.7, 3.9, 3.9, 3.8, 3.9]),
];
