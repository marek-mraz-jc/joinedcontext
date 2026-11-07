/** What the dashboard says, in Slovak and English (UI-30): the reader's language, else Slovak. */

export interface Strings {
  locale: string;
  title: string;
  subtitle: string;
  views: string;
  view: { map: string; list: string; calendar: string };
  filters: string;
  town: string;
  anyTown: string;
  category: string;
  anyCategory: string;
  from: string;
  to: string;
  search: string;
  searchHelp: string;
  clear: string;
  rangeInvalid: string;
  results: (count: number) => string;
  noResults: string;
  noEvents: string;
  loading: string;
  noEndpoint: string;
  refused: (reason: string) => string;
  truncated: (count: number) => string;
  mapLabel: string;
  noBasemap: string;
  notOnMap: (count: number) => string;
  previousMonth: string;
  nextMonth: string;
  thisMonth: string;
  weekdays: string[];
  more: (count: number) => string;
  dayEvents: (day: string, count: number) => string;
  pickDay: string;
  allDay: string;
  sourcePage: string;
  details: string;
  subscribe: string;
  subscribeIntro: string;
  wholeRegion: string;
  wholeRegionHelp: string;
  dateRange: string;
  dateRangeHelp: (from: string, to: string) => string;
  onlyDates: string;
  copy: string;
  copied: string;
  copyFailed: string;
  openCalendar: string;
  calendarLink: string;
  attribution: string;
}

const sk: Strings = {
  locale: "sk",
  title: "Podujatia v Banskobystrickom kraji",
  subtitle: "Nadchádzajúce podujatia miest, obcí a inštitúcií kraja z ich webových stránok, na mape, v zozname a v kalendári.",
  views: "Zobrazenie podujatí",
  view: { map: "Mapa", list: "Zoznam", calendar: "Kalendár" },
  filters: "Filtre",
  town: "Mesto alebo obec",
  anyTown: "Všetky",
  category: "Kategória",
  anyCategory: "Všetky",
  from: "Od",
  to: "Do",
  search: "Hľadať",
  searchHelp: "Názov, miesto alebo usporiadateľ; aj bez diakritiky.",
  clear: "Zrušiť filtre",
  rangeInvalid: "Dátum „Od“ je po dátume „Do“, preto nič nevyhovuje. Opravte jeden z nich.",
  results: (count) => `${count} ${count === 1 ? "podujatie" : count >= 2 && count <= 4 ? "podujatia" : "podujatí"}`,
  noResults: "Filtrom nevyhovuje žiadne podujatie. Zmeňte mesto, kategóriu alebo dátumy, alebo filtre zrušte.",
  noEvents: "Zatiaľ tu nie sú žiadne nadchádzajúce podujatia. Zdroje sa načítavajú raz denne v noci.",
  loading: "Načítavam podujatia…",
  noEndpoint: "Aplikácia nemá zverejnený prístup k podujatiam kraja, preto nemá čo zobraziť.",
  refused: (reason) => `Podujatia sa nepodarilo načítať: ${reason}`,
  truncated: (count) => `Zobrazených je prvých ${count} podujatí; spresnite filtre.`,
  mapLabel: "Mapa podujatí; rovnaké podujatia sú v zozname",
  noBasemap: "Podkladová mapa nie je nastavená, podujatia sú zobrazené na prázdnom pozadí.",
  notOnMap: (count) => `${count} z vybraných podujatí nemá polohu a na mape nie je.`,
  previousMonth: "Predchádzajúci mesiac",
  nextMonth: "Nasledujúci mesiac",
  thisMonth: "Tento mesiac",
  weekdays: ["Po", "Ut", "St", "Št", "Pi", "So", "Ne"],
  more: (count) => `+${count} ďalšie`,
  dayEvents: (day, count) => `${day}: ${count === 0 ? "žiadne podujatie" : sk.results(count)}`,
  pickDay: "Vyberte deň v kalendári a jeho podujatia sa zobrazia tu.",
  allDay: "celý deň",
  sourcePage: "Stránka podujatia",
  details: "Podrobnosti",
  subscribe: "Pridať do môjho kalendára",
  subscribeIntro:
    "Odkaz vložte do kalendára ako odber (Google Kalendár: Iné kalendáre, Z adresy URL; Outlook: Pridať kalendár, Prihlásiť sa na odber z webu). Kalendár sa potom aktualizuje sám.",
  wholeRegion: "Celý kraj",
  wholeRegionHelp: "Všetky nadchádzajúce podujatia kraja.",
  dateRange: "Vybrané dátumy",
  dateRangeHelp: (from, to) => `Podujatia ${from ? `od ${from}` : ""}${from && to ? " " : ""}${to ? `do ${to}` : ""}.`,
  onlyDates: "Kalendár zatiaľ filtruje len podľa dátumu; mesto, kategória a hľadanie sa do odkazu nedostanú.",
  copy: "Kopírovať odkaz",
  copied: "Odkaz je skopírovaný.",
  copyFailed: "Odkaz sa nepodarilo skopírovať; označte ho v poli a skopírujte ručne.",
  openCalendar: "Otvoriť v kalendári",
  calendarLink: "Odkaz na kalendár",
  attribution: "Zdroj: webové stránky miest, obcí a inštitúcií, ktoré podujatie zverejnili. Licencia CC BY 4.0.",
};

const en: Strings = {
  locale: "en",
  title: "Events in the Banská Bystrica Region",
  subtitle: "Upcoming events of the region's towns, municipalities and bodies from their web sites, on a map, in a list and in a calendar.",
  views: "Event views",
  view: { map: "Map", list: "List", calendar: "Calendar" },
  filters: "Filters",
  town: "Town or municipality",
  anyTown: "All",
  category: "Category",
  anyCategory: "All",
  from: "From",
  to: "To",
  search: "Search",
  searchHelp: "Name, place or organiser; diacritics optional.",
  clear: "Clear filters",
  rangeInvalid: "The From date is after the To date, so nothing matches. Correct one of them.",
  results: (count) => `${count} ${count === 1 ? "event" : "events"}`,
  noResults: "No event matches the filters. Change the town, category or dates, or clear the filters.",
  noEvents: "There are no upcoming events yet. The sources are read once a day, at night.",
  loading: "Loading events…",
  noEndpoint: "The application has no published access to the region's events, so there is nothing to show.",
  refused: (reason) => `The events could not be loaded: ${reason}`,
  truncated: (count) => `The first ${count} events are shown; narrow the filters.`,
  mapLabel: "Map of the events; the same events are in the list",
  noBasemap: "No base map is configured, the events are drawn on a blank background.",
  notOnMap: (count) => `${count} of the chosen events have no position and are not on the map.`,
  previousMonth: "Previous month",
  nextMonth: "Next month",
  thisMonth: "This month",
  weekdays: ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"],
  more: (count) => `+${count} more`,
  dayEvents: (day, count) => `${day}: ${count === 0 ? "no events" : en.results(count)}`,
  pickDay: "Pick a day in the calendar and its events are shown here.",
  allDay: "all day",
  sourcePage: "Event page",
  details: "Details",
  subscribe: "Add to my calendar",
  subscribeIntro:
    "Add the link to your calendar as a subscription (Google Calendar: Other calendars, From URL; Outlook: Add calendar, Subscribe from web). The calendar then keeps itself up to date.",
  wholeRegion: "Whole region",
  wholeRegionHelp: "Every upcoming event of the region.",
  dateRange: "Chosen dates",
  dateRangeHelp: (from, to) => `Events${from ? ` from ${from}` : ""}${to ? ` to ${to}` : ""}.`,
  onlyDates: "The calendar filters by date only for now; the town, category and search do not go into the link.",
  copy: "Copy link",
  copied: "The link is copied.",
  copyFailed: "The link could not be copied; select it in the field and copy it by hand.",
  openCalendar: "Open in calendar",
  calendarLink: "Calendar link",
  attribution: "Source: the web sites of the towns, municipalities and bodies that announced each event. Licence CC BY 4.0.",
};

export const LOCALES = { sk, en };

export function stringsFor(language: string | undefined): Strings {
  return language?.toLowerCase().startsWith("en") ? en : sk;
}
