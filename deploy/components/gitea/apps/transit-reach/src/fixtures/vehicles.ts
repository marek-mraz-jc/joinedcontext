/**
 * Five vehicles' last hour as the temporal read answers it (shaped like dev's HFP vehicles, times
 * in 2030): three buses of route 550 running east from Rautatientori through two stops, and two of
 * route 4570 running north from the last of them. Every vehicle stands still a minute at each stop
 * and reports every 30 seconds.
 */
const START = Date.UTC(2030, 9, 7, 6, 0);
const STEP = 30_000;

/** The stops, as longitude and latitude: Rautatientori, then 2 km and 4 km east, then 3 km north. */
export const STOPS = {
  centre: [24.9414, 60.171],
  east2: [24.9773, 60.171],
  east4: [25.0132, 60.171],
  north: [25.0132, 60.198],
} as const;

type Position = readonly [number, number];

/** One run: a minute at each stop, four minutes' ride between, a reading every 30 s. */
function run(path: Position[], start: number): { t: number; at: Position; speed: number }[] {
  const out: { t: number; at: Position; speed: number }[] = [];
  let t = start;
  path.forEach((stop, i) => {
    for (let k = 0; k < 3; k++, t += STEP) out.push({ t, at: [stop[0] + k * 1e-5, stop[1]], speed: 0 });
    const next = path[i + 1];
    if (!next) return;
    for (let k = 1; k < 8; k++, t += STEP) {
      const share = k / 8;
      out.push({ t, at: [stop[0] + (next[0] - stop[0]) * share, stop[1] + (next[1] - stop[1]) * share], speed: 8 });
    }
  });
  return out;
}

function vehicle(id: string, route: string, readings: { t: number; at: Position; speed: number }[]) {
  const iso = (t: number) => new Date(t).toISOString();
  return {
    id: `urn:ngsi-ld:Vehicle:hel.fi:helsinki:${id}`,
    type: "Vehicle",
    location: { type: "GeoProperty", values: readings.map((r) => [{ type: "Point", coordinates: [r.at[0], r.at[1]] }, iso(r.t)]) },
    speed: { type: "Property", values: readings.map((r) => [r.speed, iso(r.t)]) },
    route: { type: "Property", values: readings.map((r) => [route, iso(r.t)]) },
  };
}

const EAST: Position[] = [STOPS.centre, STOPS.east2, STOPS.east4];
const NORTH: Position[] = [STOPS.east4, STOPS.north];

export const HISTORY = [
  vehicle("12-1001", "550", run(EAST, START)),
  vehicle("12-1002", "550", run(EAST, START + 15 * 60_000)),
  vehicle("12-1003", "550", run(EAST, START + 30 * 60_000)),
  vehicle("12-2001", "4570", run(NORTH, START + 5 * 60_000)),
  vehicle("12-2002", "4570", run(NORTH, START + 25 * 60_000)),
];
