/** What the map says, in Slovak and English (UI-30): the reader's language, else Slovak. */
import type { Kind } from "./places";

export interface Strings {
  locale: string;
  title: string;
  subtitle: string;
  search: string;
  searchHelp: string;
  show: string;
  kind: Record<Kind, string>;
  upcomingOnly: string;
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
  attribution: string;
}

const sk: Strings = {
  locale: "sk",
  title: "Mapa mesta Banská Bystrica",
  subtitle: "Podujatia, školy a ovzdušie z otvorených dát mesta, na jednej mape.",
  search: "Hľadať podľa názvu alebo adresy",
  searchHelp: "Hľadá aj bez diakritiky, napríklad „skola“.",
  show: "Zobraziť na mape",
  kind: { event: "Podujatia", school: "Školy", air: "Ovzdušie" },
  upcomingOnly: "Iba podujatia od dnes",
  results: (count) => `${count} ${count === 1 ? "miesto" : count >= 2 && count <= 4 ? "miesta" : "miest"}`,
  noResults: "Nič sa nenašlo. Skúste iné slovo alebo zapnite ďalšiu vrstvu.",
  loading: "Načítavam miesta…",
  noEndpoint: "Aplikácia nemá zverejnený prístup k verejným dátam mesta, preto nemá čo zobraziť.",
  refused: (kind, reason) => `${kind} sa nepodarilo načítať: ${reason}`,
  truncated: (kind, count) => `${kind}: zobrazených je prvých ${count}; spresnite hľadanie.`,
  mapLabel: "Mapa miest; rovnaké miesta sú v zozname vedľa mapy",
  noBasemap: "Podkladová mapa nie je nastavená, miesta sú zobrazené na prázdnom pozadí.",
  notOnMap: "bez polohy, iba v zozname",
  unnamed: "Bez názvu",
  attribution:
    "Zdroje: Mesto Banská Bystrica (podujatia, CC BY 4.0); MŠVVaM SR, Digitálna mapa škôl; Európska environmentálna agentúra (ovzdušie, stanica SK0263A).",
};

const en: Strings = {
  locale: "en",
  title: "Map of Banská Bystrica",
  subtitle: "Events, schools and air quality from the city's open data, on one map.",
  search: "Search by name or address",
  searchHelp: "Finds words without diacritics too, for example “skola”.",
  show: "Show on the map",
  kind: { event: "Events", school: "Schools", air: "Air quality" },
  upcomingOnly: "Only events from today",
  results: (count) => `${count} ${count === 1 ? "place" : "places"}`,
  noResults: "Nothing found. Try another word or turn on another layer.",
  loading: "Loading places…",
  noEndpoint: "This app has no published access to the city's public data, so it has nothing to show.",
  refused: (kind, reason) => `${kind} could not be loaded: ${reason}`,
  truncated: (kind, count) => `${kind}: the first ${count} are shown; narrow the search.`,
  mapLabel: "Map of the places; the same places are in the list beside the map",
  noBasemap: "No base map is configured, so the places are drawn on a plain background.",
  notOnMap: "no position, in the list only",
  unnamed: "Unnamed",
  attribution:
    "Sources: City of Banská Bystrica (events, CC BY 4.0); Slovak Ministry of Education, school map; European Environment Agency (air, station SK0263A).",
};

export const LOCALES: Record<string, Strings> = { sk, en };

export function stringsFor(language: string | undefined): Strings {
  const short = (language ?? "sk").slice(0, 2).toLowerCase();
  return LOCALES[short] ?? sk;
}
