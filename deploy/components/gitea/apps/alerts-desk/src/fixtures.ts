import type { Row } from "@joinedcontext/sdk";

/**
 * Alerts in the shape the `helsinki` endpoint serves them, for the tests and the browser check
 * (`e2e/`, T-2827). The values are invented: never rows copied from the endpoint.
 */
const FEED = "https://tie.digitraffic.fi/api/traffic-message/v1/messages";
const id = (local: string) => `urn:ngsi-ld:Alert:example.org:demo:${local}`;

export const ROWS: Row[] = [
  {
    id: id("1"),
    type: "Alert",
    // A LanguageProperty in its keyValues shape; a cell reads the application's language of it.
    name: { languageMap: { fi: "Mannerheimintien päällystys", en: "Mannerheimintie resurfacing" } } as unknown as string,
    description: { languageMap: { en: "One lane closed northbound." } } as unknown as string,
    category: "traffic",
    subCategory: "ROAD_WORK",
    address: "Mannerheimintie 12, Helsinki",
    dateIssued: "2026-09-20T06:00:00Z",
    validFrom: "2026-09-21T06:00:00Z",
    validTo: "2026-09-30T18:00:00Z",
    source: FEED,
  },
  {
    id: id("2"),
    type: "Alert",
    name: "Ring I lane closure",
    category: "traffic",
    subCategory: "ROAD_WORK",
    address: "Kehä I, Espoo",
    dateIssued: "2026-09-18T08:00:00Z",
    validFrom: "2026-09-19T20:00:00Z",
    validTo: "2026-10-12T05:00:00Z",
    source: FEED,
  },
  {
    id: id("3"),
    type: "Alert",
    name: "Bridge weight limit",
    category: "traffic",
    subCategory: "WEIGHT_RESTRICTION",
    address: "Lauttasaaren silta, Helsinki",
    dateIssued: "2026-09-02T10:00:00Z",
    validFrom: "2026-09-03T00:00:00Z",
    validTo: "2026-09-25T00:00:00Z",
    source: FEED,
  },
  {
    id: id("4"),
    type: "Alert",
    name: "Marathon closes the centre",
    category: "event",
    subCategory: "PUBLIC_EVENT",
    address: "Esplanadi, Helsinki",
    dateIssued: "2026-09-10T12:00:00Z",
    validFrom: "2026-10-04T07:00:00Z",
    validTo: "2026-10-04T16:00:00Z",
    source: FEED,
  },
];
