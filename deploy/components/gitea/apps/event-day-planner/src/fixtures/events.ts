import type { Row } from "@joinedcontext/sdk";

const event = (local: string, fields: Record<string, unknown>): Row =>
  ({ id: `urn:ngsi-ld:Event:hel.fi:helsinki:${local}`, type: "Event", source: "https://api.hel.fi/linkedevents/v1/", eventStatus: "EventScheduled", ...fields }) as Row;

const names = (en: string, fi: string) => ({ languageMap: { en, fi, sv: en } });
const at = (lon: number, lat: number) => ({ type: "Point", coordinates: [lon, lat] });

/**
 * Seven events as the city's feed carries them (T-3329), on Sunday 20 October 2030 in Helsinki
 * (UTC+3): three that follow one another in the centre, one at the same time as another, one far
 * out in Itäkeskus, an exhibition open all day, and a cancelled one; plus one on the next day.
 */
export const EVENTS: Row[] = [
  event("helsinki-agf1", {
    name: names("Workshop for Families", "Perhepaja"),
    description: names("No need to register in advance.", "Toimintaan ei tarvitse ilmoittautua."),
    startDate: "2030-10-20T07:00:00Z",
    endDate: "2030-10-20T08:00:00Z",
    address: "Siltakatu 11, Helsinki",
    location: at(24.9384, 60.1699),
  }),
  event("helsinki-agf2", {
    name: names("Organ concert", "Urkukonsertti"),
    startDate: "2030-10-20T09:00:00Z",
    endDate: "2030-10-20T10:00:00Z",
    address: "Unioninkatu 29, Helsinki",
    location: at(24.9522, 60.1703),
  }),
  event("helsinki-agf3", {
    name: names("Poetry reading", "Runoilta"),
    startDate: "2030-10-20T09:30:00Z",
    endDate: "2030-10-20T10:30:00Z",
    address: "Kalevankatu 2, Helsinki",
    location: at(24.9391, 60.1663),
  }),
  event("kulke-4", {
    name: names("Jazz at Stoa", "Jazzia Stoassa"),
    startDate: "2030-10-20T15:00:00Z",
    endDate: "2030-10-20T16:30:00Z",
    address: "Turunlinnantie 1, Helsinki",
    location: at(25.0814, 60.2108),
  }),
  event("hkm-5", {
    name: names("City Museum exhibition", "Kaupunginmuseon näyttely"),
    startDate: "2030-10-01T08:00:00Z",
    endDate: "2030-12-31T16:00:00Z",
    address: "Aleksanterinkatu 16, Helsinki",
    location: at(24.9517, 60.1693),
  }),
  event("helsinki-agf6", {
    name: names("Cancelled lecture", "Peruttu luento"),
    startDate: "2030-10-20T12:00:00Z",
    endDate: "2030-10-20T13:00:00Z",
    eventStatus: "EventCancelled",
    address: "Oodi, Helsinki",
    location: at(24.9381, 60.1737),
  }),
  event("helsinki-agf7", {
    name: names("Monday matinee", "Maanantain matinea"),
    startDate: "2030-10-21T10:00:00Z",
    endDate: "2030-10-21T12:00:00Z",
    address: "Mannerheimintie 13, Helsinki",
    location: at(24.9372, 60.1716),
  }),
];

/** The fixtures as the page holds them: read through the SDK's stub, each language map resolved to English. */
export async function eventRows(): Promise<Row[]> {
  const { stubClient } = await import("@joinedcontext/sdk/testing");
  const access = { permissions: [{ resource: { type: "Event" }, actions: ["queryEntity", "retrieveEntity"], attributes: "*" as const }], prohibitions: [] };
  return stubClient({ entities: EVENTS, access }, { language: "en" }).entities.all("Event");
}
