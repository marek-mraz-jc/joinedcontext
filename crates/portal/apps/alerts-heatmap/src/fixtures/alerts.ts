import type { Row } from "@joinedcontext/sdk";

const alert = (local: string, fields: Record<string, unknown>): Row =>
  ({ id: `urn:ngsi-ld:Alert:hel.fi:helsinki:${local}`, type: "Alert", category: "traffic", ...fields }) as Row;

const point = (lon: number, lat: number) => ({ type: "Point", coordinates: [lon, lat] });

/**
 * Eight alerts as the city's feed carries them (T-3333), the texts in Finnish as published: four
 * road works around one junction of Mannerheimintie (a repeat place), a road work along a street
 * as a MultiLineString, two traffic announcements, and one announcement with no start time.
 * Monday 2030-10-07 05:00 UTC is 08:00 in Helsinki.
 */
export const ALERTS: Row[] = [
  alert("GUID1", { name: "Mannerheimintie, Helsinki. Tietyö.", subCategory: "roadWorks", address: "Helsinki", validFrom: "2030-10-07T05:00:00Z", dateIssued: "2030-10-01T08:00:00Z", location: point(24.9361, 60.1712) }),
  alert("GUID2", { name: "Mannerheimintie, Helsinki. Tietyö.", subCategory: "roadWorks", address: "Helsinki", validFrom: "2030-10-07T05:30:00Z", dateIssued: "2030-10-01T08:10:00Z", location: point(24.9364, 60.1714) }),
  alert("GUID3", { name: "Mannerheimintie, Helsinki. Tietyö.", subCategory: "roadWorks", address: "Helsinki", validFrom: "2030-10-14T05:15:00Z", dateIssued: "2030-10-08T08:00:00Z", location: point(24.9366, 60.1711) }),
  alert("GUID4", { name: "Mannerheimintie, Helsinki. Tietyö.", subCategory: "roadWorks", address: "Helsinki", validFrom: "2030-10-21T05:45:00Z", dateIssued: "2030-10-15T08:00:00Z", location: point(24.9362, 60.1716) }),
  alert("GUID5", {
    name: "Tie 40927, Espoo. Tietyö.",
    subCategory: "roadWorks",
    address: "Espoo",
    validFrom: "2030-10-08T21:00:00Z",
    dateIssued: "2030-10-02T07:53:43Z",
    location: { type: "MultiLineString", coordinates: [[[24.7568, 60.2274], [24.7579, 60.2263]], [[24.7581, 60.2261], [24.7585, 60.2258]]] },
  }),
  alert("GUID6", { name: "Kehä I, Vantaa. Liikennetiedote.", subCategory: "carAccident", address: "Vantaa", validFrom: "2030-10-09T14:20:00Z", dateIssued: "2030-10-09T14:20:00Z", location: point(25.0105, 60.2501) }),
  alert("GUID7", { name: "Länsiväylä, Espoo. Liikennetiedote.", subCategory: "carAccident", address: "Espoo", validFrom: "2030-10-10T06:05:00Z", dateIssued: "2030-10-10T06:05:00Z", location: point(24.8302, 60.1612) }),
  alert("GUID8", { name: "Itäväylä, Helsinki. Liikennetiedote.", subCategory: "carAccident", address: "Helsinki", location: point(25.0802, 60.2101) }),
];
