/** Every word on screen, Slovak first (UI-30); the two locales have the same keys. */
import type { SortKey } from "./bridges";

export interface Strings {
  locale: string;
  title: string;
  subtitle: string;
  loading: string;
  failed: (reason: string) => string;
  noEndpoint: string;
  empty: string;
  tiles: { bridges: string; length: string; medianAge: string; listed: string; incomplete: string };
  search: string;
  district: string;
  roadClass: string;
  any: string;
  listedOnly: string;
  column: Record<SortKey | "road" | "yearBuilt" | "material" | "heritage" | "manager" | "district", string>;
  sortBy: (column: string) => string;
  oldest: string;
  longest: string;
  tenthNote: string;
  tenthUnavailable: string;
  years: (count: number) => string;
  metres: string;
  noValue: string;
  values: Record<string, string>;
  download: string;
  csvHeader: string[];
  csvName: string;
  truncated: (count: number) => string;
  source: string;
}

const sk: Strings = {
  locale: "sk",
  title: "Mosty Banskobystrického kraja",
  subtitle: "Pracovisko pre správu ciest: vek, dĺžka a pamiatková ochrana mostov z krajského registra.",
  loading: "Načítavam mosty…",
  failed: (reason) => `Mosty sa nepodarilo načítať: ${reason}`,
  noEndpoint: "Aplikácia nemá prístup k registru mostov, preto nemá čo zobraziť.",
  empty: "Žiadny most nezodpovedá výberu.",
  tiles: { bridges: "Mosty", length: "Premostená dĺžka spolu (m)", medianAge: "Medián veku (roky)", listed: "Pamiatky", incomplete: "Bez roku alebo dĺžky" },
  search: "Hľadať most podľa názvu, kódu alebo správcu",
  district: "Okres",
  roadClass: "Trieda cesty",
  any: "všetky",
  listedOnly: "Iba pamiatkovo chránené",
  column: {
    name: "Most",
    road: "Cesta",
    yearBuilt: "Rok výstavby",
    age: "Vek",
    spans: "Polia",
    length: "Dĺžka (m)",
    material: "Materiál",
    heritage: "Pamiatka",
    manager: "Správca",
    district: "Okres",
  },
  sortBy: (column) => `Zoradiť podľa: ${column}`,
  oldest: "najstaršia desatina v kraji",
  longest: "najdlhšia desatina v kraji",
  tenthNote: "Označenie porovnáva most s ostatnými mostmi v kraji, nie s normou.",
  tenthUnavailable: "Na porovnanie s desatinou kraja treba aspoň desať mostov s údajom.",
  years: (count) => `${count} ${count === 1 ? "rok" : count >= 2 && count <= 4 ? "roky" : "rokov"}`,
  metres: "m",
  noValue: "neuvedené",
  values: {
    motorway: "diaľnica",
    firstClass: "I. trieda",
    secondClass: "II. trieda",
    thirdClass: "III. trieda",
    local: "miestna",
    service: "účelová",
    notListed: "nie",
    cultural: "kultúrna pamiatka",
    culturalAndTechnical: "kultúrna a technická pamiatka",
  },
  download: "Stiahnuť tabuľku (CSV)",
  csvHeader: ["Most", "Kód", "Trieda cesty", "Číslo cesty", "Rok výstavby", "Vek (roky)", "Polia", "Dĺžka (m)", "Materiál", "Pamiatka", "Správca", "Okres"],
  csvName: "mosty-bbsk.csv",
  truncated: (count) => `Zobrazených je prvých ${count} mostov.`,
  source: "Zdroj: Banskobystrický samosprávny kraj, register mostov, cez verejný priestor bbsk-registre.",
};

const en: Strings = {
  locale: "en",
  title: "Bridges of the Banská Bystrica region",
  subtitle: "A desk for road management: age, length and heritage status of the bridges from the region's register.",
  loading: "Loading bridges…",
  failed: (reason) => `The bridges could not be loaded: ${reason}`,
  noEndpoint: "This app has no access to the bridge register, so it has nothing to show.",
  empty: "No bridge matches the selection.",
  tiles: { bridges: "Bridges", length: "Total bridged length (m)", medianAge: "Median age (years)", listed: "Listed monuments", incomplete: "Without year or length" },
  search: "Search a bridge by name, code or manager",
  district: "District",
  roadClass: "Road class",
  any: "all",
  listedOnly: "Only listed monuments",
  column: {
    name: "Bridge",
    road: "Road",
    yearBuilt: "Year built",
    age: "Age",
    spans: "Spans",
    length: "Length (m)",
    material: "Material",
    heritage: "Monument",
    manager: "Manager",
    district: "District",
  },
  sortBy: (column) => `Sort by: ${column}`,
  oldest: "oldest tenth in the region",
  longest: "longest tenth in the region",
  tenthNote: "The mark compares a bridge with the other bridges of the region, not with a norm.",
  tenthUnavailable: "Comparing with the region's tenth needs at least ten bridges with the figure.",
  years: (count) => `${count} ${count === 1 ? "year" : "years"}`,
  metres: "m",
  noValue: "not given",
  values: {
    motorway: "motorway",
    firstClass: "first class",
    secondClass: "second class",
    thirdClass: "third class",
    local: "local",
    service: "service road",
    notListed: "no",
    cultural: "cultural monument",
    culturalAndTechnical: "cultural and technical monument",
  },
  download: "Download the table (CSV)",
  csvHeader: ["Bridge", "Code", "Road class", "Road number", "Year built", "Age (years)", "Spans", "Length (m)", "Material", "Monument", "Manager", "District"],
  csvName: "bridges-bbsk.csv",
  truncated: (count) => `The first ${count} bridges are shown.`,
  source: "Source: Banská Bystrica Self-Governing Region, bridge register, through the public space bbsk-registre.",
};

export const LOCALES: Record<string, Strings> = { sk, en };

export function stringsFor(language: string | undefined): Strings {
  return LOCALES[(language ?? "sk").slice(0, 2).toLowerCase()] ?? sk;
}
