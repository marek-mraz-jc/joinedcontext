/** What the dashboard says, in Slovak and English (UI-30): the reader's language, else Slovak. */
import type { Key } from "./indicators";

export interface Strings {
  locale: string;
  title: string;
  page: string;
  subtitle: string;
  label: Record<Key, string>;
  question: Record<Key, string>;
  /** The unit as a person reads it, for the code `Development/13` fixes. */
  unit: Record<Key, string>;
  notMeasured: string;
  window: string;
  quarter: (label: string) => string;
  year: (label: string) => string;
  formula: string;
  computed: (when: string) => string;
  loading: string;
  noEndpoint: string;
  refused: (reason: string) => string;
  none: string;
  noThreshold: string;
  attribution: string;
}

const sk: Strings = {
  locale: "sk",
  title: "Ukazovatele mesta Žilina",
  page: "Ukazovatele",
  subtitle: "Šesť čísel o meste z otvorených údajov Štatistického úradu SR, každé so svojím obdobím a výpočtom.",
  label: {
    "obyvatelstvo-stav": "Obyvatelia",
    "celkovy-prirastok": "Celkový prírastok obyvateľov",
    "priemerny-vek": "Priemerný vek",
    "index-starnutia": "Index starnutia",
    uchadzaci: "Uchádzači o zamestnanie",
    "navstevnici-rok": "Návštevníci ubytovacích zariadení",
  },
  question: {
    "obyvatelstvo-stav": "Koľko ľudí má v meste trvalý pobyt na konci štvrťroka.",
    "celkovy-prirastok": "Narodení mínus zomrelí plus prisťahovaní mínus vysťahovaní; strata je záporná.",
    "priemerny-vek": "Priemerný vek obyvateľa mesta.",
    "index-starnutia": "Počet ľudí v poproduktívnom veku na 100 detí.",
    uchadzaci: "Počet evidovaných uchádzačov o zamestnanie s bydliskom v meste.",
    "navstevnici-rok": "Hostia ubytovaní v meste za celý posledný kalendárny rok.",
  },
  unit: {
    "obyvatelstvo-stav": "osôb",
    "celkovy-prirastok": "osôb",
    "priemerny-vek": "rokov",
    "index-starnutia": "%",
    uchadzaci: "osôb",
    "navstevnici-rok": "návštevníkov",
  },
  notMeasured: "nemerané",
  window: "Obdobie",
  quarter: (label) => `${label.slice(0, 2).replace("Q", "")}. štvrťrok ${label.slice(3)}`,
  year: (label) => `rok ${label}`,
  formula: "Ako sa počíta",
  computed: (when) => `Vypočítané ${when}`,
  loading: "Načítavam ukazovatele…",
  noEndpoint: "Aplikácia nemá zverejnený prístup k ukazovateľom mesta, preto nemá čo zobraziť.",
  refused: (reason) => `Ukazovatele sa nepodarilo načítať: ${reason}`,
  none: "Mesto zatiaľ nezverejnilo žiadny ukazovateľ.",
  noThreshold:
    "Pre žiadne z týchto čísel neexistuje zverejnená hraničná hodnota, preto žiadne nie je označené ako dobré či zlé.",
  attribution: "Zdroj: Štatistický úrad SR, DATAcube (om7101qr, om7052rr, om7105rr, pr5001rr, cr3803mr), CC BY-SA 4.0.",
};

const en: Strings = {
  locale: "en",
  title: "Indicators of the city of Žilina",
  page: "Indicators",
  subtitle: "Six numbers about the city from the Statistical Office's open data, each with its period and how it is computed.",
  label: {
    "obyvatelstvo-stav": "Residents",
    "celkovy-prirastok": "Total population change",
    "priemerny-vek": "Mean age",
    "index-starnutia": "Ageing index",
    uchadzaci: "Registered jobseekers",
    "navstevnici-rok": "Visitors in accommodation",
  },
  question: {
    "obyvatelstvo-stav": "How many people are permanent residents of the city at the end of the quarter.",
    "celkovy-prirastok": "Births less deaths plus people moving in less people moving out; a loss is negative.",
    "priemerny-vek": "The mean age of a resident of the city.",
    "index-starnutia": "People of post-productive age per 100 children.",
    uchadzaci: "Registered jobseekers living in the city.",
    "navstevnici-rok": "Guests staying in the city's accommodation over the last whole calendar year.",
  },
  unit: {
    "obyvatelstvo-stav": "people",
    "celkovy-prirastok": "people",
    "priemerny-vek": "years",
    "index-starnutia": "%",
    uchadzaci: "people",
    "navstevnici-rok": "visitors",
  },
  notMeasured: "not measured",
  window: "Period",
  quarter: (label) => `${label}`,
  year: (label) => `year ${label}`,
  formula: "How it is computed",
  computed: (when) => `Computed ${when}`,
  loading: "Loading the indicators…",
  noEndpoint: "This app has no published access to the city's indicators, so it has nothing to show.",
  refused: (reason) => `The indicators could not be loaded: ${reason}`,
  none: "The city has not published an indicator yet.",
  noThreshold: "None of these numbers has a published limit, so none is marked good or bad.",
  attribution: "Source: Statistical Office of the SR, DATAcube (om7101qr, om7052rr, om7105rr, pr5001rr, cr3803mr), CC BY-SA 4.0.",
};

export const LOCALES: Record<string, Strings> = { sk, en };

export function stringsFor(language: string | undefined): Strings {
  const short = (language ?? "sk").slice(0, 2).toLowerCase();
  return LOCALES[short] ?? sk;
}
