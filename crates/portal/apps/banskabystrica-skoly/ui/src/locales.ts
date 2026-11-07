/** Every word on screen, Slovak first (UI-30); the two locales have the same keys. */
import type { SortKey } from "./coverage";

export interface Strings {
  locale: string;
  title: string;
  subtitle: string;
  loading: string;
  failed: (reason: string) => string;
  noEndpoint: string;
  empty: string;
  tiles: { schools: string; pupils: string; teachers: string; ratio: string; incomplete: string };
  search: string;
  completeOnly: string;
  column: Record<SortKey | "otherStaff" | "budget", string>;
  sortBy: (column: string) => string;
  highest: string;
  lowest: string;
  /** Under staff and budget several schools publish identically (one organization's). */
  shared: (schools: number) => string;
  tenthNote: string;
  tenthUnavailable: string;
  noValue: string;
  download: string;
  csvHeader: string[];
  csvName: string;
  truncated: (count: number) => string;
  source: string;
}

const sk: Strings = {
  locale: "sk",
  title: "Školy mesta Banská Bystrica",
  subtitle: "Pracovisko pre zamestnancov mesta: žiaci, učitelia a rozpočet škôl z Digitálnej mapy škôl.",
  loading: "Načítavam školy…",
  failed: (reason) => `Školy sa nepodarilo načítať: ${reason}`,
  noEndpoint: "Aplikácia nemá prístup k údajom o školách, preto nemá čo zobraziť.",
  empty: "Žiadna škola nezodpovedá hľadaniu.",
  tiles: { schools: "Školy", pupils: "Žiaci", teachers: "Pedagogickí zamestnanci", ratio: "Žiakov na učiteľa (mesto)", incomplete: "Školy bez úplných údajov" },
  search: "Hľadať školu podľa názvu alebo adresy",
  completeOnly: "Iba školy s úplnými údajmi",
  column: {
    name: "Škola",
    pupils: "Žiaci",
    teachers: "Učitelia",
    pupilsPerTeacher: "Žiakov na učiteľa",
    otherStaff: "Nepedagogickí",
    budget: "Rozpočet (€)",
    budgetPerPupil: "Rozpočet na žiaka (€)",
  },
  sortBy: (column) => `Zoradiť podľa: ${column}`,
  highest: "najvyššia desatina v meste",
  lowest: "najnižšia desatina v meste",
  shared: (schools) => `spoločné pre ${schools} škôl jednej organizácie`,
  tenthNote: "Označenie porovnáva školu s ostatnými školami v meste, nie s normou.",
  tenthUnavailable: "Na porovnanie s desatinou mesta treba aspoň desať škôl s údajmi.",
  noValue: "neuvedené",
  download: "Stiahnuť tabuľku (CSV)",
  csvHeader: ["Škola", "Adresa", "Žiaci", "Učitelia", "Žiakov na učiteľa", "Nepedagogickí", "Rozpočet (€)", "Rok rozpočtu", "Rozpočet na žiaka (€)"],
  csvName: "skoly-banska-bystrica.csv",
  truncated: (count) => `Zobrazených je prvých ${count} škôl.`,
  source: "Zdroj: MŠVVaM SR, Digitálna mapa škôl, cez verejný priestor mesta Banská Bystrica.",
};

const en: Strings = {
  locale: "en",
  title: "Schools of Banská Bystrica",
  subtitle: "A desk for city staff: pupils, teachers and budget of the schools from the national school map.",
  loading: "Loading schools…",
  failed: (reason) => `The schools could not be loaded: ${reason}`,
  noEndpoint: "This app has no access to the school data, so it has nothing to show.",
  empty: "No school matches the search.",
  tiles: { schools: "Schools", pupils: "Pupils", teachers: "Teaching staff", ratio: "Pupils per teacher (city)", incomplete: "Schools without complete figures" },
  search: "Search a school by name or address",
  completeOnly: "Only schools with complete figures",
  column: {
    name: "Organization",
    pupils: "Pupils",
    teachers: "Teachers",
    pupilsPerTeacher: "Pupils per teacher",
    otherStaff: "Non-teaching",
    budget: "Budget (€)",
    budgetPerPupil: "Budget per pupil (€)",
  },
  sortBy: (column) => `Sort by: ${column}`,
  highest: "highest tenth in the city",
  lowest: "lowest tenth in the city",
  shared: (schools) => `shared by ${schools} schools of one organization`,
  tenthNote: "The mark compares a school with the other schools of the city, not with a norm.",
  tenthUnavailable: "Comparing with the city's tenth needs at least ten schools with figures.",
  noValue: "not given",
  download: "Download the table (CSV)",
  csvHeader: ["Organization", "Address", "Pupils", "Teachers", "Pupils per teacher", "Non-teaching", "Budget (€)", "Budget year", "Budget per pupil (€)"],
  csvName: "schools-banska-bystrica.csv",
  truncated: (count) => `The first ${count} schools are shown.`,
  source: "Source: Slovak Ministry of Education, school map, through the city of Banská Bystrica's public space.",
};

export const LOCALES: Record<string, Strings> = { sk, en };

export function stringsFor(language: string | undefined): Strings {
  return LOCALES[(language ?? "sk").slice(0, 2).toLowerCase()] ?? sk;
}
