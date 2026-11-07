/** What the map says, in Slovak and English (UI-30): the reader's language, else Slovak. */
import type { Kind, Pollutant } from "./places";

export interface Strings {
  locale: string;
  title: string;
  page: string;
  subtitle: string;
  search: string;
  searchHelp: string;
  show: string;
  all: string;
  kind: Record<Kind, string>;
  results: (count: number) => string;
  noResults: string;
  loading: string;
  noEndpoint: string;
  refused: (kind: string, reason: string) => string;
  truncated: (kind: string, count: number) => string;
  mapLabel: string;
  noBasemap: string;
  notOnMap: string;
  unnamed: string;
  departuresToday: (count: number) => string;
  pollutant: Record<Pollutant, string>;
  attribution: string;
}

const sk: Strings = {
  locale: "sk",
  title: "Mapa mesta Žilina",
  page: "Mapa",
  subtitle: "Kultúrne pamiatky, železničné stanice a ovzdušie z otvorených dát o meste, na jednej mape.",
  search: "Hľadať podľa názvu, adresy alebo slohu",
  searchHelp: "Hľadá aj bez diakritiky, napríklad „kastiel“.",
  show: "Zobraziť",
  all: "Všetko",
  kind: { monument: "Pamiatky", station: "Železničné stanice", air: "Ovzdušie" },
  results: (count) => `${count} ${count === 1 ? "miesto" : count >= 2 && count <= 4 ? "miesta" : "miest"}`,
  noResults: "Nič sa nenašlo. Skúste iné slovo alebo zobrazte všetko.",
  loading: "Načítavam miesta…",
  noEndpoint: "Aplikácia nemá zverejnený prístup k verejným dátam o meste, preto nemá čo zobraziť.",
  refused: (kind, reason) => `${kind} sa nepodarilo načítať: ${reason}`,
  truncated: (kind, count) => `${kind}: zobrazených je prvých ${count}; spresnite hľadanie.`,
  mapLabel: "Mapa miest; rovnaké miesta sú v zozname",
  noBasemap: "Podkladová mapa nie je nastavená, miesta sú zobrazené na prázdnom pozadí.",
  notOnMap: "bez adresy, iba v zozname",
  unnamed: "Bez názvu",
  departuresToday: (count) => `${count} ${count === 1 ? "odchod" : count >= 2 && count <= 4 ? "odchody" : "odchodov"} dnes`,
  pollutant: { pm10: "PM10", pm25: "PM2,5", no2: "NO₂", o3: "O₃", co: "CO" },
  attribution:
    "Zdroje: Pamiatkový úrad SR (register NKP) a MV SR (register adries), CC BY 4.0; ŽSR, grafikon vlakovej dopravy (GTFS); Európska environmentálna agentúra, údaje SHMÚ (stanica SK0020A), CC BY 4.0.",
};

const en: Strings = {
  locale: "en",
  title: "Map of Žilina",
  page: "Map",
  subtitle: "Cultural monuments, railway stations and air quality from the open data about the city, on one map.",
  search: "Search by name, address or style",
  searchHelp: "Finds words without diacritics too, for example “kastiel”.",
  show: "Show",
  all: "Everything",
  kind: { monument: "Monuments", station: "Railway stations", air: "Air quality" },
  results: (count) => `${count} ${count === 1 ? "place" : "places"}`,
  noResults: "Nothing found. Try another word or show everything.",
  loading: "Loading places…",
  noEndpoint: "This app has no published access to the public data about the city, so it has nothing to show.",
  refused: (kind, reason) => `${kind} could not be loaded: ${reason}`,
  truncated: (kind, count) => `${kind}: the first ${count} are shown; narrow the search.`,
  mapLabel: "Map of the places; the same places are in the list",
  noBasemap: "No base map is configured, so the places are drawn on a plain background.",
  notOnMap: "no address, in the list only",
  unnamed: "Unnamed",
  departuresToday: (count) => `${count} ${count === 1 ? "train" : "trains"} leaving today`,
  pollutant: { pm10: "PM10", pm25: "PM2.5", no2: "NO₂", o3: "O₃", co: "CO" },
  attribution:
    "Sources: Monuments Board of the SR (register of monuments) and Ministry of Interior of the SR (register of addresses), CC BY 4.0; ŽSR, train timetable (GTFS); European Environment Agency, SHMÚ's data (station SK0020A), CC BY 4.0.",
};

export const LOCALES: Record<string, Strings> = { sk, en };

export function stringsFor(language: string | undefined): Strings {
  const short = (language ?? "sk").slice(0, 2).toLowerCase();
  return LOCALES[short] ?? sk;
}
