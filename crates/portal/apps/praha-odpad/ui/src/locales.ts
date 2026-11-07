/** Every word on screen, Czech first (UI-30); the two locales have the same keys. */
import type { SortKey } from "./containers";

export interface Strings {
  locale: string;
  title: string;
  /** The one page's name in the shell. */
  page: string;
  subtitle: string;
  loading: string;
  failed: (reason: string) => string;
  noEndpoint: string;
  empty: string;
  tiles: { containers: string; meanFill: string; unread: string };
  search: string;
  kind: string;
  any: string;
  fullestOnly: string;
  column: Record<SortKey | "kind" | "isle", string>;
  sortBy: (column: string) => string;
  fullest: string;
  longestUnread: string;
  tenthNote: string;
  tenthUnavailable: string;
  ago: (hours: number) => string;
  isleUnknown: string;
  noValue: string;
  values: Record<string, string>;
  download: string;
  csvHeader: string[];
  csvName: string;
  truncated: (count: number) => string;
  source: string;
}

const cs: Strings = {
  locale: "cs",
  title: "Tříděný odpad Prahy",
  page: "Kontejnery",
  subtitle: "Pracoviště svozu: zaplněnost kontejnerů se senzorem, stáří posledního měření a stanoviště.",
  loading: "Načítám kontejnery…",
  failed: (reason) => `Kontejnery se nepodařilo načíst: ${reason}`,
  noEndpoint: "Aplikace nemá přístup k datům o kontejnerech, a nemá tedy co zobrazit.",
  empty: "Žádný kontejner neodpovídá výběru.",
  tiles: { containers: "Kontejnery se senzorem", meanFill: "Průměrná zaplněnost", unread: "Bez platného měření" },
  search: "Hledat podle kódu kontejneru nebo stanoviště",
  kind: "Druh odpadu",
  any: "všechny",
  fullestOnly: "Jen nejplnější desetina",
  column: { code: "Kontejner", kind: "Druh odpadu", fill: "Zaplněnost", ageHours: "Měřeno", isle: "Stanoviště" },
  sortBy: (column) => `Seřadit podle: ${column}`,
  fullest: "nejplnější desetina ve městě",
  longestUnread: "nejdéle neměřená desetina",
  tenthNote: "Označení porovnává kontejner s ostatními kontejnery ve městě, ne s normou.",
  tenthUnavailable: "Pro porovnání s desetinou města je třeba alespoň deset kontejnerů s měřením.",
  ago: (hours) => (hours < 1 ? "před méně než hodinou" : hours < 48 ? `před ${Math.floor(hours)} h` : `před ${Math.floor(hours / 24)} dny`),
  isleUnknown: "stanoviště neuvedeno",
  noValue: "neuvedeno",
  values: {
    colouredGlass: "barevné sklo",
    clearGlass: "čiré sklo",
    paper: "papír",
    plastic: "plast",
    metal: "kovy",
    beverageCartons: "nápojové kartony",
    electronics: "elektro",
    edibleOil: "jedlé oleje",
    mixedRecyclables: "směs",
  },
  download: "Stáhnout tabulku (CSV)",
  csvHeader: ["Kontejner", "Druh odpadu", "Zaplněnost (%)", "Měřeno", "Stanoviště"],
  csvName: "kontejnery-praha.csv",
  truncated: (count) => `Zobrazeno je prvních ${count} kontejnerů.`,
  source: "Zdroj: Golemio, zaplněnost kontejnerů na tříděný odpad; IPR Praha a MHMP, stanoviště; přes veřejný prostor praha-mesto.",
};

const en: Strings = {
  locale: "en",
  title: "Prague recycling",
  page: "Containers",
  subtitle: "A collection desk: how full the sensor containers are, how old their last reading is, and where they stand.",
  loading: "Loading containers…",
  failed: (reason) => `The containers could not be loaded: ${reason}`,
  noEndpoint: "This app has no access to the container data, so it has nothing to show.",
  empty: "No container matches the selection.",
  tiles: { containers: "Containers with a sensor", meanFill: "Mean fill level", unread: "Without a valid reading" },
  search: "Search by container or point code",
  kind: "Kind of waste",
  any: "all",
  fullestOnly: "Only the fullest tenth",
  column: { code: "Container", kind: "Kind of waste", fill: "Fill level", ageHours: "Measured", isle: "Recycling point" },
  sortBy: (column) => `Sort by: ${column}`,
  fullest: "fullest tenth in the city",
  longestUnread: "longest-unread tenth",
  tenthNote: "The mark compares a container with the other containers of the city, not with a norm.",
  tenthUnavailable: "Comparing with the city's tenth needs at least ten containers with a reading.",
  ago: (hours) => (hours < 1 ? "less than an hour ago" : hours < 48 ? `${Math.floor(hours)} h ago` : `${Math.floor(hours / 24)} days ago`),
  isleUnknown: "point not given",
  noValue: "not given",
  values: {
    colouredGlass: "coloured glass",
    clearGlass: "clear glass",
    paper: "paper",
    plastic: "plastic",
    metal: "metal",
    beverageCartons: "beverage cartons",
    electronics: "electronics",
    edibleOil: "edible oil",
    mixedRecyclables: "mixed recyclables",
  },
  download: "Download the table (CSV)",
  csvHeader: ["Container", "Kind of waste", "Fill level (%)", "Measured", "Recycling point"],
  csvName: "containers-prague.csv",
  truncated: (count) => `The first ${count} containers are shown.`,
  source: "Source: Golemio, fill levels of sorted-waste containers; IPR Praha and MHMP, recycling points; through the public space praha-mesto.",
};

export const LOCALES: Record<string, Strings> = { cs, en };

export function stringsFor(language: string | undefined): Strings {
  return LOCALES[(language ?? "cs").slice(0, 2).toLowerCase()] ?? cs;
}
