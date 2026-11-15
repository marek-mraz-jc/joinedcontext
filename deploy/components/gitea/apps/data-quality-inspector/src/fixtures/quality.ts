import type { Row } from "@joinedcontext/sdk";

/** Fixed Unix seconds (2026-10-08T12:01:40Z) so calculated ages in tests are exact. */
export const NOW = 1_791_460_900;

/**
 * Platform JSON schemas for three types:
 * - BikeHireDockingStation: LanguageProperty name, GeoProperty location with required [type, coordinates],
 *   integer availableBikeNumber min 0, status string, dateModified date-time.
 * - AdministrativeArea: districtCode pattern ^[0-9]{1,10}$, divisionLevel $ref.
 * - Event: startDate date-time, url format uri.
 * All three carry additionalProperties: false and required: ["id", "type"].
 */
export const SCHEMA: Record<string, unknown> = {
  BikeHireDockingStation: {
    properties: {
      name: {
        type: ["object", "null"],
        "x-ngsi-ld-kind": "LanguageProperty",
      },
      location: {
        type: "object",
        properties: {
          type: { type: "string", enum: ["Point", "Polygon"] },
          coordinates: {},
        },
        required: ["type", "coordinates"],
      },
      availableBikeNumber: {
        type: "integer",
        minimum: 0,
      },
      status: {
        type: "string",
      },
      dateModified: {
        type: "string",
        format: "date-time",
      },
    },
    additionalProperties: false,
    required: ["id", "type"],
  },
  AdministrativeArea: {
    properties: {
      districtCode: {
        type: "string",
        pattern: "^[0-9]{1,10}$",
      },
      divisionLevel: {
        $ref: "#/definitions/DivisionLevel",
      },
    },
    additionalProperties: false,
    required: ["id", "type"],
  },
  Event: {
    properties: {
      startDate: {
        type: "string",
        format: "date-time",
      },
      url: {
        type: "string",
        format: "uri",
      },
    },
    additionalProperties: false,
    required: ["id", "type"],
  },
};

/** Schema including Vehicle without any schema properties for full UI and e2e inspection. */
export const SCHEMA_WITH_VEHICLE: Record<string, unknown> = {
  ...SCHEMA,
  Vehicle: {},
};

const point = (lon: number, lat: number) => ({
  type: "Point" as const,
  coordinates: [lon, lat],
});

/**
 * Six BikeHireDockingStation entities:
 * - clean-1 (age 100 s at NOW)
 * - clean-2 (age 200 s at NOW)
 * - fail-neg: availableBikeNumber = -1 (minimum 0 violation, age 300 s)
 * - fail-float: availableBikeNumber = 1.5 (integer violation, age 400 s)
 * - fail-date: dateModified = "not-a-datetime" (date-time format violation)
 * - fail-loc: location = { type: "Point" } (missing coordinates in GeoProperty, age 700 s)
 */
const BIKE_ROWS: Row[] = [
  {
    id: "urn:ngsi-ld:BikeHireDockingStation:clean-1",
    type: "BikeHireDockingStation",
    name: "Rautatieasema",
    location: point(24.94, 60.17),
    availableBikeNumber: 10,
    status: "active",
    dateModified: "2026-10-08T12:00:00Z",
  },
  {
    id: "urn:ngsi-ld:BikeHireDockingStation:clean-2",
    type: "BikeHireDockingStation",
    name: "Kamppi",
    location: point(24.93, 60.16),
    availableBikeNumber: 5,
    status: "active",
    dateModified: "2026-10-08T11:58:20Z",
  },
  {
    id: "urn:ngsi-ld:BikeHireDockingStation:fail-neg",
    type: "BikeHireDockingStation",
    name: "Hakaniemi",
    location: point(24.95, 60.18),
    availableBikeNumber: -1,
    status: "active",
    dateModified: "2026-10-08T11:56:40Z",
  },
  {
    id: "urn:ngsi-ld:BikeHireDockingStation:fail-float",
    type: "BikeHireDockingStation",
    name: "Kallio",
    location: point(24.95, 60.19),
    availableBikeNumber: 1.5,
    status: "active",
    dateModified: "2026-10-08T11:55:00Z",
  },
  {
    id: "urn:ngsi-ld:BikeHireDockingStation:fail-date",
    type: "BikeHireDockingStation",
    name: "Töölö",
    location: point(24.92, 60.18),
    availableBikeNumber: 3,
    status: "active",
    dateModified: "not-a-datetime",
  },
  {
    id: "urn:ngsi-ld:BikeHireDockingStation:fail-loc",
    type: "BikeHireDockingStation",
    name: "Pasila",
    // A geometry without coordinates is the defect under test; the SDK's Row type cannot hold it.
    location: { type: "Point" } as unknown as Row["location"],
    availableBikeNumber: 7,
    status: "active",
    dateModified: "2026-10-08T11:50:00Z",
  },
];

/**
 * Two AdministrativeArea entities:
 * - dist-1: clean (districtCode "001", divisionLevel "district")
 * - dist-fail: districtCode "12a" (pattern ^[0-9]{1,10}$ violation)
 */
const DISTRICT_ROWS: Row[] = [
  {
    id: "urn:ngsi-ld:AdministrativeArea:dist-1",
    type: "AdministrativeArea",
    districtCode: "001",
    divisionLevel: "district",
  },
  {
    id: "urn:ngsi-ld:AdministrativeArea:dist-fail",
    type: "AdministrativeArea",
    districtCode: "12a",
    divisionLevel: "district",
  },
];

/**
 * Two Event entities:
 * - ev-1: clean (startDate "2026-10-08T10:00:00Z", url "https://hel.fi/events/1")
 * - ev-fail: extra attribute extraProp (unknown attribute with additionalProperties: false)
 */
const EVENT_ROWS: Row[] = [
  {
    id: "urn:ngsi-ld:Event:ev-1",
    type: "Event",
    startDate: "2026-10-08T10:00:00Z",
    url: "https://hel.fi/events/1",
  },
  {
    id: "urn:ngsi-ld:Event:ev-fail",
    type: "Event",
    startDate: "2026-10-08T08:00:00Z",
    url: "https://hel.fi/events/2",
    extraProp: "unexpected",
  },
];

/**
 * Two Vehicle entities (fourth type with no published schema).
 */
const VEHICLE_ROWS: Row[] = [
  {
    id: "urn:ngsi-ld:Vehicle:veh-1",
    type: "Vehicle",
    speed: 45,
    batteryLevel: 85,
  },
  {
    id: "urn:ngsi-ld:Vehicle:veh-2",
    type: "Vehicle",
    speed: 50,
    batteryLevel: 70,
  },
];

export const ROWS_BY_TYPE: Record<string, Row[]> = {
  BikeHireDockingStation: BIKE_ROWS,
  AdministrativeArea: DISTRICT_ROWS,
  Event: EVENT_ROWS,
  Vehicle: VEHICLE_ROWS,
};

export const ENTITIES: Row[] = [
  ...BIKE_ROWS,
  ...DISTRICT_ROWS,
  ...EVENT_ROWS,
  ...VEHICLE_ROWS,
];
