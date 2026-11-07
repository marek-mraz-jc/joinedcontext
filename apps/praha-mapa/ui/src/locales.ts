/** What the map says, in Czech and English (UI-30): the reader's language, else Czech. */
import type { Kind, Type } from "./places";

export interface Strings {
  locale: string;
  title: string;
  /** The one page's name in the shell. */
  page: string;
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
  /** A type past the cap, read inside the map's view (T-5155). */
  inView: (what: string, count: number) => string;
  inViewTruncated: (what: string, count: number) => string;
  mapLabel: string;
  noBasemap: string;
  notOnMap: string;
  unnamed: string;
  attribution: string;
}

const cs: Strings = {
  locale: "cs",
  title: "Mapa Prahy",
  page: "Mapa",
  subtitle: "Školy, kultura, veřejné toalety, prodej jízdenek a tříděný odpad z otevřených dat Prahy, na jedné mapě.",
  search: "Hledat podle názvu, adresy nebo druhu",
  searchHelp: "Hledá i bez diakritiky, například „skola“.",
  show: "Zobrazit na mapě",
  kind: { school: "Školy", culture: "Kultura", publicToilet: "Veřejné toalety", ticketSale: "Prodej jízdenek", waste: "Tříděný odpad" },
  type: { PointOfInterest: "Místa", WasteContainerIsle: "Stanoviště tříděného odpadu" },
  results: (count) => `${count} ${count === 1 ? "místo" : count >= 2 && count <= 4 ? "místa" : "míst"}`,
  noResults: "Nic nenalezeno. Zkuste jiné slovo nebo zapněte další vrstvu.",
  loading: "Načítám místa…",
  noEndpoint: "Aplikace nemá zveřejněný přístup k otevřeným datům Prahy, a nemá tedy co zobrazit.",
  refused: (what, reason) => `${what} se nepodařilo načíst: ${reason}`,
  truncated: (what, count) => `${what}: zobrazeno je prvních ${count}; upřesněte hledání.`,
  inView: (what, count) => `${what}: ${count} v zobrazené části mapy; posuňte nebo přibližte mapu pro další.`,
  inViewTruncated: (what, count) => `${what}: prvních ${count} v zobrazené části mapy; přibližte mapu.`,
  mapLabel: "Mapa míst; stejná místa jsou v seznamu vedle mapy",
  noBasemap: "Podkladová mapa není nastavena, místa jsou zobrazena na prázdném pozadí.",
  notOnMap: "bez polohy, jen v seznamu",
  unnamed: "Bez názvu",
  attribution: "Zdroje: IPR Praha (školská a kulturní zařízení, veřejné toalety, stanoviště tříděného odpadu s MHMP); PID (prodejní místa jízdenek); přes veřejný prostor praha-mesto.",
};

const en: Strings = {
  locale: "en",
  title: "Map of Prague",
  page: "Map",
  subtitle: "Schools, culture, public toilets, ticket sale and recycling from Prague's open data, on one map.",
  search: "Search by name, address or kind",
  searchHelp: "Finds words without diacritics too, for example “skola”.",
  show: "Show on the map",
  kind: { school: "Schools", culture: "Culture", publicToilet: "Public toilets", ticketSale: "Ticket sale", waste: "Recycling" },
  type: { PointOfInterest: "Places", WasteContainerIsle: "Recycling points" },
  results: (count) => `${count} ${count === 1 ? "place" : "places"}`,
  noResults: "Nothing found. Try another word or turn on another layer.",
  loading: "Loading places…",
  noEndpoint: "This app has no published access to Prague's open data, so it has nothing to show.",
  refused: (what, reason) => `${what} could not be loaded: ${reason}`,
  truncated: (what, count) => `${what}: the first ${count} are shown; narrow the search.`,
  inView: (what, count) => `${what}: ${count} in the map's view; move or zoom the map for others.`,
  inViewTruncated: (what, count) => `${what}: the first ${count} in the map's view; zoom in.`,
  mapLabel: "Map of the places; the same places are in the list beside the map",
  noBasemap: "No base map is configured, so the places are drawn on a plain background.",
  notOnMap: "no position, in the list only",
  unnamed: "Unnamed",
  attribution: "Sources: IPR Praha (school and cultural facilities, public toilets, sorted-waste points with MHMP); PID (ticket points); through the public space praha-mesto.",
};

export const LOCALES: Record<string, Strings> = { cs, en };

export function stringsFor(language: string | undefined): Strings {
  const short = (language ?? "cs").slice(0, 2).toLowerCase();
  return LOCALES[short] ?? cs;
}
