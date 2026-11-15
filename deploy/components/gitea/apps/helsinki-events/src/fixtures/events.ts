import type { Row } from "@joinedcontext/sdk";

const event = (local: string, fields: Record<string, unknown>): Row =>
  ({ id: `urn:ngsi-ld:Event:hel.fi:helsinki:${local}`, type: "Event", ...fields }) as Row;

const names = (en: string, fi: string) => ({ languageMap: { en, fi, sv: en } });

/**
 * Six events as the city's feed carries them (T-2923): three registers, one without a place on
 * the map, one cancelled, one already over, one whose source is not an https link.
 */
export const EVENTS: Row[] = [
  event("helsinki-agf1", {
    name: names("Workshop for Families", "Perhepaja"),
    description: names("No need to register in advance.", "Toimintaan ei tarvitse ilmoittautua."),
    startDate: "2030-10-20T14:00:00Z",
    endDate: "2030-10-20T16:00:00Z",
    eventStatus: "EventScheduled",
    address: "Siltakatu 11, Helsinki",
    location: { type: "Point", coordinates: [24.9384, 60.1699] },
    source: "https://api.hel.fi/linkedevents/v1/",
  }),
  event("helsinki-agf2", {
    name: names("Organ concert", "Urkukonsertti"),
    description: names("Music in the cathedral.", "Musiikkia tuomiokirkossa."),
    startDate: "2030-10-20T17:00:00Z",
    endDate: "2030-10-20T18:30:00Z",
    eventStatus: "EventScheduled",
    address: "Unioninkatu 29, Helsinki",
    location: { type: "Point", coordinates: [24.9522, 60.1703] },
    source: "https://api.hel.fi/linkedevents/v1/",
  }),
  event("espoo_le-agn5", {
    name: names("Story hour", "Satutunti"),
    description: names("For children aged 3 to 6.", "3–6-vuotiaille."),
    startDate: "2030-10-22T09:00:00Z",
    endDate: "2030-10-22T10:00:00Z",
    eventStatus: "EventScheduled",
    address: "Kirjastokatu 1, Espoo",
    location: { type: "Point", coordinates: [24.6559, 60.2055] },
    source: "http://insecure.example/events",
  }),
  event("kulke-7711", {
    name: names("Jazz at Stoa", "Jazzia Stoassa"),
    description: names("An evening of jazz.", "Jazzilta."),
    startDate: "2030-11-05T17:00:00Z",
    endDate: "2030-11-05T19:00:00Z",
    eventStatus: "EventCancelled",
    address: "Turunlinnantie 1, Helsinki",
    source: "https://api.hel.fi/linkedevents/v1/",
  }),
  event("kulke-7712", {
    name: names("Dance workshop", "Tanssityöpaja"),
    startDate: "2030-10-23T15:00:00Z",
    endDate: "2030-10-23T17:00:00Z",
    eventStatus: "EventScheduled",
    address: "Turunlinnantie 1, Helsinki",
    location: { type: "Point", coordinates: [25.0785, 60.2107] },
  }),
  event("helsinki-past", {
    name: names("Summer market", "Kesätori"),
    startDate: "2020-06-01T08:00:00Z",
    endDate: "2020-06-01T14:00:00Z",
    eventStatus: "EventScheduled",
    location: { type: "Point", coordinates: [24.95, 60.167] },
  }),
];
