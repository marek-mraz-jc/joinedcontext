/** What the research screen says, in Slovak and English (UI-30): the reader's language, else Slovak. */
export interface Strings {
  locale: string;
  title: string;
  page: string;
  subtitle: string;
  works: (count: number) => string;
  span: (first: number, last: number) => string;
  series: (count: number) => string;
  perYear: string;
  perYearTable: string;
  year: string;
  count: string;
  kinds: string;
  kind: Record<string, string>;
  allKinds: string;
  topSeries: string;
  list: string;
  search: string;
  searchHelp: string;
  pickYear: string;
  allYears: string;
  shown: (shown: number, of: number) => string;
  more: string;
  none: string;
  open: string;
  noYear: string;
  noTitle: string;
  language: Record<string, string>;
  loading: string;
  noEndpoint: string;
  refused: (reason: string) => string;
  truncated: (count: number) => string;
  noAuthors: string;
  attribution: string;
}

const sk: Strings = {
  locale: "sk",
  title: "Otvorený výskum Žilinskej univerzity",
  page: "Výskum",
  subtitle: "Práce z Digitálneho repozitára UNIZA, ktoré majú vlastnú otvorenú licenciu: koľko ich je, odkiaľ sú a kde si ich prečítať.",
  works: (count) => `${count} ${count === 1 ? "práca" : count >= 2 && count <= 4 ? "práce" : "prác"}`,
  span: (first, last) => `z rokov ${first} – ${last}`,
  series: (count) => `v ${count} ${count === 1 ? "časopise či zborníku" : "časopisoch a zborníkoch"}`,
  perYear: "Práce podľa roku vydania",
  perYearTable: "Počty prác podľa roku",
  year: "Rok",
  count: "Počet",
  kinds: "Druh práce",
  kind: {
    Article: "Článok",
    "Conference paper": "Príspevok z konferencie",
    "Book of proceedings": "Zborník",
    "Working Paper": "Pracovný dokument",
    Journal: "Časopis",
    "journal article": "Článok v časopise",
    Book: "Kniha",
    Other: "Iné",
  },
  allKinds: "Všetky druhy",
  topSeries: "Časopisy a zborníky s najviac prácami",
  list: "Práce",
  search: "Hľadať v názve alebo zbierke",
  searchHelp: "Hľadá aj bez diakritiky, napríklad „krizovy manazment“.",
  pickYear: "Rok vydania",
  allYears: "Všetky roky",
  shown: (shown, of) => `Zobrazených ${shown} z ${of}`,
  more: "Zobraziť ďalšie",
  none: "Nič sa nenašlo. Skúste iné slovo alebo iný druh či rok.",
  open: "Otvoriť v DREPO",
  noYear: "rok neuvedený",
  noTitle: "Bez názvu",
  language: { sk: "slovensky", en: "anglicky", cs: "česky", de: "nemecky", fr: "francúzsky", pl: "poľsky", hu: "maďarsky" },
  loading: "Načítavam práce…",
  noEndpoint: "Aplikácia nemá zverejnený prístup k prácam univerzity, preto nemá čo zobraziť.",
  refused: (reason) => `Práce sa nepodarilo načítať: ${reason}`,
  truncated: (count) => `Zobrazených je prvých ${count} prác repozitára.`,
  noAuthors: "Mená autorov aplikácia nezobrazuje: sú pri každej práci v DREPO.",
  attribution: "Zdroj: Žilinská univerzita v Žiline, DREPO; každá práca pod licenciou, ktorú uvádza (CC BY 4.0).",
};

const en: Strings = {
  locale: "en",
  title: "Open research of the University of Žilina",
  page: "Research",
  subtitle: "The works of the UNIZA digital repository that carry an open licence of their own: how many, where from and where to read them.",
  works: (count) => `${count} ${count === 1 ? "work" : "works"}`,
  span: (first, last) => `from ${first} to ${last}`,
  series: (count) => `in ${count} ${count === 1 ? "journal or proceedings" : "journals and proceedings"}`,
  perYear: "Works by year of publication",
  perYearTable: "Number of works per year",
  year: "Year",
  count: "Works",
  kinds: "Kind of work",
  kind: {
    Article: "Article",
    "Conference paper": "Conference paper",
    "Book of proceedings": "Book of proceedings",
    "Working Paper": "Working paper",
    Journal: "Journal",
    "journal article": "Journal article",
    Book: "Book",
    Other: "Other",
  },
  allKinds: "Every kind",
  topSeries: "Journals and proceedings with the most works",
  list: "Works",
  search: "Search the title or the collection",
  searchHelp: "Finds words without diacritics too, for example “krizovy manazment”.",
  pickYear: "Year of publication",
  allYears: "Every year",
  shown: (shown, of) => `${shown} of ${of} shown`,
  more: "Show more",
  none: "Nothing found. Try another word, kind or year.",
  open: "Open in DREPO",
  noYear: "no year given",
  noTitle: "Untitled",
  language: { sk: "Slovak", en: "English", cs: "Czech", de: "German", fr: "French", pl: "Polish", hu: "Hungarian" },
  loading: "Loading the works…",
  noEndpoint: "This app has no published access to the university's works, so it has nothing to show.",
  refused: (reason) => `The works could not be loaded: ${reason}`,
  truncated: (count) => `The first ${count} works of the repository are shown.`,
  noAuthors: "The app shows no author names: they are with each work in DREPO.",
  attribution: "Source: University of Žilina, DREPO; each work under the licence it states (CC BY 4.0).",
};

export const LOCALES: Record<string, Strings> = { sk, en };

export function stringsFor(language: string | undefined): Strings {
  const short = (language ?? "sk").slice(0, 2).toLowerCase();
  return LOCALES[short] ?? sk;
}
