import type { Row } from "@joinedcontext/sdk";

const FINTRAFFIC = "https://tie.digitraffic.fi/api/traffic-message/v1/messages";
const id = (local: string) => `urn:ngsi-ld:Alert:hel.fi:helsinki:${local}`;

/** Five alerts in the shape the `helsinki` endpoint serves them: four from Fintraffic, one a steward added. */
export const ALERTS: Row[] = [
  {
    id: id("GUID50001"),
    type: "Alert",
    // A LanguageProperty in its keyValues shape; a row reads the application's language of it.
    name: { languageMap: { fi: "Mannerheimintien päällystys", en: "Mannerheimintie resurfacing" } } as unknown as string,
    category: "traffic",
    subCategory: "roadWorks",
    address: "Mannerheimintie 12, Helsinki",
    dateIssued: "2026-09-20T06:00:00Z",
    validFrom: "2026-09-21T06:00:00Z",
    validTo: "2026-09-30T18:00:00Z",
    location: { type: "Point", coordinates: [24.9384, 60.1699] },
    source: FINTRAFFIC,
  },
  {
    id: id("GUID50002"),
    type: "Alert",
    name: "Ring I lane closure",
    category: "traffic",
    subCategory: "roadWorks",
    address: "Kehä I, Espoo",
    dateIssued: "2026-09-18T08:00:00Z",
    validFrom: "2026-09-19T20:00:00Z",
    validTo: "2026-09-22T12:00:00Z",
    location: { type: "Point", coordinates: [24.8321, 60.2211] },
    source: FINTRAFFIC,
  },
  {
    id: id("GUID50003"),
    type: "Alert",
    name: "Accident on Itäväylä",
    category: "traffic",
    subCategory: "carAccident",
    address: "Itäväylä, Helsinki",
    dateIssued: "2026-09-22T07:40:00Z",
    validFrom: "2026-09-22T07:30:00Z",
    validTo: null,
    location: { type: "Point", coordinates: [25.0781, 60.2105] },
    source: FINTRAFFIC,
  },
  {
    id: id("GUID50004"),
    type: "Alert",
    name: "Länsiväylä bridge inspection",
    category: "traffic",
    subCategory: "roadWorks",
    address: "Länsiväylä, Espoo",
    dateIssued: "2026-09-10T09:00:00Z",
    validFrom: "2026-09-12T21:00:00Z",
    validTo: "2026-09-14T05:00:00Z",
    location: { type: "Point", coordinates: [24.8543, 60.1623] },
    source: FINTRAFFIC,
  },
  {
    id: id("steward-market-day"),
    type: "Alert",
    name: null,
    category: "event",
    subCategory: "MARKET",
    address: "Kauppatori, Helsinki",
    dateIssued: "2026-09-21T10:00:00Z",
    validFrom: "2026-09-23T06:00:00Z",
    validTo: "2026-09-23T16:00:00Z",
    location: { type: "Point", coordinates: [24.9526, 60.1675] },
    source: null,
  },
];
