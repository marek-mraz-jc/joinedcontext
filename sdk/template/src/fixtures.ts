import type { Row } from "@joinedcontext/sdk";

/**
 * Rows of every type the application reads, for its browser check (`e2e/`, T-2827): the built
 * bundle is served with the SDK's stub answering its endpoint from these rows, and every page is
 * checked at a phone, a tablet, a laptop and a wall. Keep the types and attribute names those of
 * `src/jc-types.ts` and the values invented: never rows copied from the endpoint, since the
 * repository is read by more people than the data's audience.
 */
export const ROWS: Row[] = [
  { id: "urn:ngsi-ld:Station:example.org:demo:1", type: "Station", name: "Kamppi", bikes: 4, status: "open", location: { type: "Point", coordinates: [24.93, 60.17] } },
  { id: "urn:ngsi-ld:Station:example.org:demo:2", type: "Station", name: "Kallio", bikes: 7, status: "closed", location: { type: "Point", coordinates: [24.95, 60.18] } },
  { id: "urn:ngsi-ld:Station:example.org:demo:3", type: "Station", name: "Töölö", bikes: 0, status: "open", location: { type: "Point", coordinates: [24.92, 60.18] } },
  { id: "urn:ngsi-ld:Note:example.org:demo:1", type: "Note", text: "Check the lock at Kallio" },
];
