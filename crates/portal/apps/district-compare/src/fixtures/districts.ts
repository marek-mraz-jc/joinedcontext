import type { Row } from "@joinedcontext/sdk";
import type { RowsByType } from "../districts";

const point = (lon: number, lat: number) => ({
  type: "Point" as const,
  coordinates: [lon, lat],
});

/**
 * Three AdministrativeAreas in Helsinki (Kamppi with a hole, Kallio with a MultiPolygon, Töölö simple polygon),
 * plus one subdistrict (Etu-Töölö) that must be filtered out by divisionLevel === "district".
 */
const DISTRICT_ROWS: Row[] = [
  {
    id: "urn:ngsi-ld:AdministrativeArea:hel.fi:helsinki:101",
    type: "AdministrativeArea",
    districtCode: "101",
    divisionLevel: "district",
    name: "Kamppi",
    location: {
      type: "Polygon",
      coordinates: [
        // Outer ring: [24.920, 60.160] to [24.940, 60.170]
        [
          [24.920, 60.160],
          [24.940, 60.160],
          [24.940, 60.170],
          [24.920, 60.170],
          [24.920, 60.160],
        ],
        // Interior hole: [24.926, 60.163] to [24.934, 60.167]
        [
          [24.926, 60.163],
          [24.934, 60.163],
          [24.934, 60.167],
          [24.926, 60.167],
          [24.926, 60.163],
        ],
      ],
    },
  },
  {
    id: "urn:ngsi-ld:AdministrativeArea:hel.fi:helsinki:102",
    type: "AdministrativeArea",
    districtCode: "102",
    divisionLevel: "district",
    name: "Kallio",
    location: {
      type: "MultiPolygon",
      coordinates: [
        // Part 1: [24.945, 60.180] to [24.955, 60.190]
        [
          [
            [24.945, 60.180],
            [24.955, 60.180],
            [24.955, 60.190],
            [24.945, 60.190],
            [24.945, 60.180],
          ],
        ],
        // Part 2: [24.960, 60.180] to [24.970, 60.190]
        [
          [
            [24.960, 60.180],
            [24.970, 60.180],
            [24.970, 60.190],
            [24.960, 60.190],
            [24.960, 60.180],
          ],
        ],
      ],
    },
  },
  {
    id: "urn:ngsi-ld:AdministrativeArea:hel.fi:helsinki:103",
    type: "AdministrativeArea",
    districtCode: "103",
    divisionLevel: "district",
    name: "Töölö",
    location: {
      type: "Polygon",
      coordinates: [
        // [24.910, 60.175] to [24.930, 60.185]
        [
          [24.910, 60.175],
          [24.930, 60.175],
          [24.930, 60.185],
          [24.910, 60.185],
          [24.910, 60.175],
        ],
      ],
    },
  },
  {
    id: "urn:ngsi-ld:AdministrativeArea:hel.fi:helsinki:1031",
    type: "AdministrativeArea",
    districtCode: "1031",
    divisionLevel: "subdistrict",
    name: "Etu-Töölö",
    location: {
      type: "Polygon",
      coordinates: [
        [
          [24.910, 60.168],
          [24.918, 60.168],
          [24.918, 60.174],
          [24.910, 60.174],
          [24.910, 60.168],
        ],
      ],
    },
  },
];

/**
 * 10 Events:
 * - 3 in Kamppi (101)
 * - 2 in Kallio (102: 1 in Part 1, 1 in Part 2)
 * - 1 in Töölö (103)
 * - 1 in the hole of Kamppi (outside)
 * - 1 in the subdistrict Etu-Töölö (outside)
 * - 1 outside all districts (outside)
 * - 1 with no location (outside)
 */
const EVENT_ROWS: Row[] = [
  {
    id: "urn:ngsi-ld:Event:hel.fi:helsinki:ev-1",
    type: "Event",
    name: "Kamppi tapahtuma 1",
    startDate: "2030-06-01T10:00:00Z",
    location: point(24.922, 60.162),
  },
  {
    id: "urn:ngsi-ld:Event:hel.fi:helsinki:ev-2",
    type: "Event",
    name: "Kamppi tapahtuma 2",
    startDate: "2030-06-01T12:00:00Z",
    location: point(24.924, 60.168),
  },
  {
    id: "urn:ngsi-ld:Event:hel.fi:helsinki:ev-3",
    type: "Event",
    name: "Kamppi tapahtuma 3",
    startDate: "2030-06-01T14:00:00Z",
    location: point(24.938, 60.165),
  },
  {
    id: "urn:ngsi-ld:Event:hel.fi:helsinki:ev-4",
    type: "Event",
    name: "Kallio tapahtuma 1",
    startDate: "2030-06-02T10:00:00Z",
    location: point(24.950, 60.185),
  },
  {
    id: "urn:ngsi-ld:Event:hel.fi:helsinki:ev-5",
    type: "Event",
    name: "Kallio tapahtuma 2",
    startDate: "2030-06-02T12:00:00Z",
    location: point(24.965, 60.185),
  },
  {
    id: "urn:ngsi-ld:Event:hel.fi:helsinki:ev-6",
    type: "Event",
    name: "Töölö tapahtuma 1",
    startDate: "2030-06-03T10:00:00Z",
    location: point(24.920, 60.180),
  },
  {
    id: "urn:ngsi-ld:Event:hel.fi:helsinki:ev-7",
    type: "Event",
    name: "Kamppi reiässä tapahtuma",
    startDate: "2030-06-04T10:00:00Z",
    location: point(24.930, 60.165),
  },
  {
    id: "urn:ngsi-ld:Event:hel.fi:helsinki:ev-8",
    type: "Event",
    name: "Osa-alueen tapahtuma",
    startDate: "2030-06-05T10:00:00Z",
    location: point(24.914, 60.171),
  },
  {
    id: "urn:ngsi-ld:Event:hel.fi:helsinki:ev-9",
    type: "Event",
    name: "Ulkopuolinen tapahtuma",
    startDate: "2030-06-06T10:00:00Z",
    location: point(25.000, 60.250),
  },
  {
    id: "urn:ngsi-ld:Event:hel.fi:helsinki:ev-10",
    type: "Event",
    name: "Sijainniton tapahtuma",
    startDate: "2030-06-07T10:00:00Z",
    location: null,
  },
];

/**
 * 5 Bike stations:
 * - 1 in Kamppi (20 slots)
 * - 2 in Kallio (15 slots each = 30 slots total)
 * - 1 in Töölö (20 slots) -> ties with Kamppi on bikes (1) and bikeSlots (20)
 * - 1 outside (10 slots)
 */
const BIKE_ROWS: Row[] = [
  {
    id: "urn:ngsi-ld:BikeHireDockingStation:hel.fi:helsinki:bike-1",
    type: "BikeHireDockingStation",
    name: "Kamppi asema",
    totalSlotNumber: 20,
    location: point(24.922, 60.162),
  },
  {
    id: "urn:ngsi-ld:BikeHireDockingStation:hel.fi:helsinki:bike-2",
    type: "BikeHireDockingStation",
    name: "Kallio asema 1",
    totalSlotNumber: 15,
    location: point(24.950, 60.185),
  },
  {
    id: "urn:ngsi-ld:BikeHireDockingStation:hel.fi:helsinki:bike-3",
    type: "BikeHireDockingStation",
    name: "Kallio asema 2",
    totalSlotNumber: 15,
    location: point(24.965, 60.185),
  },
  {
    id: "urn:ngsi-ld:BikeHireDockingStation:hel.fi:helsinki:bike-4",
    type: "BikeHireDockingStation",
    name: "Töölö asema",
    totalSlotNumber: 20,
    location: point(24.920, 60.180),
  },
  {
    id: "urn:ngsi-ld:BikeHireDockingStation:hel.fi:helsinki:bike-5",
    type: "BikeHireDockingStation",
    name: "Ulkopuolinen asema",
    totalSlotNumber: 10,
    location: point(25.000, 60.250),
  },
];

/**
 * 5 Alerts:
 * - 1 in Kamppi
 * - 2 in Kallio
 * - 1 in Töölö -> ties with Kamppi on alerts (1)
 * - 1 outside
 */
const ALERT_ROWS: Row[] = [
  {
    id: "urn:ngsi-ld:Alert:hel.fi:helsinki:alert-1",
    type: "Alert",
    name: "Kamppi tietyö",
    validFrom: "2030-05-01T08:00:00Z",
    location: point(24.922, 60.162),
  },
  {
    id: "urn:ngsi-ld:Alert:hel.fi:helsinki:alert-2",
    type: "Alert",
    name: "Kallio tietyö 1",
    validFrom: "2030-05-02T08:00:00Z",
    location: point(24.950, 60.185),
  },
  {
    id: "urn:ngsi-ld:Alert:hel.fi:helsinki:alert-3",
    type: "Alert",
    name: "Kallio tietyö 2",
    validFrom: "2030-05-03T08:00:00Z",
    location: point(24.965, 60.185),
  },
  {
    id: "urn:ngsi-ld:Alert:hel.fi:helsinki:alert-4",
    type: "Alert",
    name: "Töölö tietyö",
    validFrom: "2030-05-04T08:00:00Z",
    location: point(24.920, 60.180),
  },
  {
    id: "urn:ngsi-ld:Alert:hel.fi:helsinki:alert-5",
    type: "Alert",
    name: "Ulkopuolinen tiedote",
    validFrom: "2030-05-05T08:00:00Z",
    location: point(25.000, 60.250),
  },
];

/**
 * 3 Air quality stations:
 * - 1 in Kamppi (pm25 = 12.0, aqi = 2)
 * - 1 in Kallio (pm25 = 18.0, aqi = 3)
 * - None in Töölö (pm25 = null, aqi = null, rank = null)
 * - 1 outside (pm25 = 10.0, aqi = 1)
 */
const AIR_ROWS: Row[] = [
  {
    id: "urn:ngsi-ld:AirQualityObserved:hel.fi:helsinki:air-1",
    type: "AirQualityObserved",
    name: "Kamppi ilmanlaatu",
    pm25: 12.0,
    airQualityIndex: 2,
    dateObserved: "2030-05-01T12:00:00Z",
    location: point(24.922, 60.162),
  },
  {
    id: "urn:ngsi-ld:AirQualityObserved:hel.fi:helsinki:air-2",
    type: "AirQualityObserved",
    name: "Kallio ilmanlaatu",
    pm25: 18.0,
    airQualityIndex: 3,
    dateObserved: "2030-05-02T12:00:00Z",
    location: point(24.950, 60.185),
  },
  {
    id: "urn:ngsi-ld:AirQualityObserved:hel.fi:helsinki:air-3",
    type: "AirQualityObserved",
    name: "Ulkopuolinen ilmanlaatu",
    pm25: 10.0,
    airQualityIndex: 1,
    dateObserved: "2030-05-03T12:00:00Z",
    location: point(25.000, 60.250),
  },
];

export const ROWS: RowsByType = {
  districts: DISTRICT_ROWS,
  events: EVENT_ROWS,
  bikes: BIKE_ROWS,
  alerts: ALERT_ROWS,
  air: AIR_ROWS,
};

export const ENTITIES: Row[] = [
  ...DISTRICT_ROWS,
  ...EVENT_ROWS,
  ...BIKE_ROWS,
  ...ALERT_ROWS,
  ...AIR_ROWS,
];
