/** What the map says, in Finnish and English (UI-30): the reader's language, else Finnish. */
import type { Kind, Type } from "./places";

export interface Strings {
  locale: string;
  title: string;
  subtitle: string;
  search: string;
  searchHelp: string;
  show: string;
  kind: Record<Kind, string>;
  type: Record<Type, string>;
  results: (count: number) => string;
  noResults: string;
  loading: string;
  noEndpoint: string;
  refused: (what: string, reason: string) => string;
  truncated: (what: string, count: number) => string;
  mapLabel: string;
  noBasemap: string;
  notOnMap: string;
  unnamed: string;
  attribution: string;
}

const fi: Strings = {
  locale: "fi",
  title: "Helsingin palvelukartta",
  subtitle: "Kirjastot, terveysasemat, uimahallit, uimarannat ja koulut sekä rantojen vedenlämpö kaupungin avoimesta datasta.",
  search: "Hae nimellä tai osoitteella",
  searchHelp: "Hakee myös ilman ääkkösiä, esimerkiksi ”kirjasto”.",
  show: "Näytä kartalla",
  kind: { library: "Kirjastot", healthStation: "Terveysasemat", swimmingHall: "Uimahallit", beach: "Uimarannat", school: "Koulut", water: "Vedenlämpö" },
  type: { PointOfInterest: "Palvelut", WaterQualityObserved: "Vedenlämpöanturit" },
  results: (count) => `${count} ${count === 1 ? "paikka" : "paikkaa"}`,
  noResults: "Ei tuloksia. Kokeile toista hakusanaa tai lisää karttataso.",
  loading: "Ladataan paikkoja…",
  noEndpoint: "Sovelluksella ei ole julkaistua pääsyä Helsingin avoimeen dataan, joten sillä ei ole mitään näytettävää.",
  refused: (what, reason) => `${what}: lataus epäonnistui: ${reason}`,
  truncated: (what, count) => `${what}: näytetään ensimmäiset ${count}; tarkenna hakua.`,
  mapLabel: "Paikkojen kartta; samat paikat ovat luettelossa kartan vieressä",
  noBasemap: "Taustakarttaa ei ole määritetty, joten paikat näytetään tyhjällä taustalla.",
  notOnMap: "ei sijaintia, vain luettelossa",
  unnamed: "Nimetön",
  attribution: "Lähteet: Helsingin kaupunki, palvelukartta; Helsingin kaupunki, uimarantojen vedenlämpöanturit; julkisen tilan helsinki kautta.",
};

const en: Strings = {
  locale: "en",
  title: "Helsinki service map",
  subtitle: "Libraries, health stations, swimming halls, beaches and schools, with the beaches' water temperature, from the city's open data.",
  search: "Search by name or address",
  searchHelp: "Finds words without diacritics too, for example “kirjasto”.",
  show: "Show on the map",
  kind: { library: "Libraries", healthStation: "Health stations", swimmingHall: "Swimming halls", beach: "Beaches", school: "Schools", water: "Water temperature" },
  type: { PointOfInterest: "Services", WaterQualityObserved: "Water-temperature sensors" },
  results: (count) => `${count} ${count === 1 ? "place" : "places"}`,
  noResults: "Nothing found. Try another word or turn on another layer.",
  loading: "Loading places…",
  noEndpoint: "This app has no published access to Helsinki's open data, so it has nothing to show.",
  refused: (what, reason) => `${what} could not be loaded: ${reason}`,
  truncated: (what, count) => `${what}: the first ${count} are shown; narrow the search.`,
  mapLabel: "Map of the places; the same places are in the list beside the map",
  noBasemap: "No base map is configured, so the places are drawn on a plain background.",
  notOnMap: "no position, in the list only",
  unnamed: "Unnamed",
  attribution: "Sources: City of Helsinki, service map; City of Helsinki, beach water-temperature sensors; through the public space helsinki.",
};

export const LOCALES: Record<string, Strings> = { fi, en };

export function stringsFor(language: string | undefined): Strings {
  const short = (language ?? "fi").slice(0, 2).toLowerCase();
  return LOCALES[short] ?? fi;
}
