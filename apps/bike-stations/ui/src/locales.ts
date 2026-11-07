/** What the station map says, in English and Finnish (UI-30): the reader's language, else English. */
import type { Standing } from "./stations";

export interface Strings {
  locale: string;
  title: string;
  subtitle: string;
  totals: (stations: number, bikes: number, docks: number) => string;
  search: string;
  searchHelp: string;
  results: (count: number) => string;
  noResults: string;
  loading: string;
  noEndpoint: string;
  refused: (reason: string) => string;
  truncated: (count: number) => string;
  mapLabel: string;
  noBasemap: string;
  notOnMap: string;
  standing: Record<Standing, string>;
  bikesAndDocks: (bikes: number | null, docks: number | null) => string;
  unnamed: string;
  status: string;
  attribution: string;
}

const count = (value: number | null, missing: string) => (value === null ? missing : String(value));

const en: Strings = {
  locale: "en",
  title: "City bike stations",
  subtitle: "Free bikes and free docks at every HSL city bike station, read from the helsinki project's shared stations.",
  totals: (stations, bikes, docks) => `${stations} stations · ${bikes} free bikes · ${docks} free docks`,
  search: "Search by station name",
  searchHelp: "Finds names without diacritics too, for example “toolo”.",
  results: (n) => (n === 1 ? "1 station" : `${n} stations`),
  noResults: "No station matches the search.",
  loading: "Loading the stations…",
  noEndpoint:
    "This application has no endpoint for the shared city bike stations, so there is nothing to show. A steward of the project helsinki-mobility can check the shared space city-bikes.",
  refused: (reason) => `The stations could not be read: ${reason}`,
  truncated: (n) => `Only the first ${n} stations are shown.`,
  mapLabel: "Map of the city bike stations; the same stations are listed beside it",
  noBasemap: "The platform has no basemap configured, so the stations are drawn without a map under them.",
  notOnMap: "no position",
  standing: { empty: "no free bikes", full: "no free docks", available: "bikes and docks free", unknown: "counts not reported" },
  bikesAndDocks: (bikes, docks) => `${count(bikes, "–")} bikes · ${count(docks, "–")} docks`,
  unnamed: "Unnamed station",
  status: "Status",
  attribution: "Data: HSL city bikes (Inurba), ODbL 1.0, through the helsinki project of the joinedcontext platform.",
};

const fi: Strings = {
  locale: "fi",
  title: "Kaupunkipyöräasemat",
  subtitle: "Vapaat pyörät ja vapaat telineet jokaisella HSL:n kaupunkipyöräasemalla helsinki-projektin jakamista asemista.",
  totals: (stations, bikes, docks) => `${stations} asemaa · ${bikes} vapaata pyörää · ${docks} vapaata telinettä`,
  search: "Hae aseman nimellä",
  searchHelp: "Löytää nimet myös ilman ääkkösiä, esimerkiksi ”toolo”.",
  results: (n) => (n === 1 ? "1 asema" : `${n} asemaa`),
  noResults: "Haulla ei löydy asemaa.",
  loading: "Ladataan asemia…",
  noEndpoint:
    "Sovelluksella ei ole päätepistettä jaetuille kaupunkipyöräasemille, joten näytettävää ei ole. Projektin helsinki-mobility ylläpitäjä voi tarkistaa jaetun tilan city-bikes.",
  refused: (reason) => `Asemia ei voitu lukea: ${reason}`,
  truncated: (n) => `Vain ensimmäiset ${n} asemaa näytetään.`,
  mapLabel: "Kartta kaupunkipyöräasemista; samat asemat on lueteltu sen vieressä",
  noBasemap: "Alustalle ei ole määritetty taustakarttaa, joten asemat piirretään ilman karttaa.",
  notOnMap: "ei sijaintia",
  standing: { empty: "ei vapaita pyöriä", full: "ei vapaita telineitä", available: "pyöriä ja telineitä vapaana", unknown: "määriä ei ilmoitettu" },
  bikesAndDocks: (bikes, docks) => `${count(bikes, "–")} pyörää · ${count(docks, "–")} telinettä`,
  unnamed: "Nimetön asema",
  status: "Tila",
  attribution: "Tiedot: HSL:n kaupunkipyörät (Inurba), ODbL 1.0, joinedcontext-alustan helsinki-projektin kautta.",
};

export const LOCALES = { en, fi } as const;

/** The strings of the reader's language: `fi-FI` reads Finnish, anything else English. */
export function stringsFor(language: string | undefined): Strings {
  return language?.toLowerCase().startsWith("fi") ? fi : en;
}
