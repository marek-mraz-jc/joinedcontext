/**
 * The App's words in Finnish, English and Swedish, and Helsinki's way of writing numbers, days and hours.
 * The language comes from the address (`?lang=fi`), else the reader's browser's first of the three,
 * English otherwise. The alerts' own texts are Finnish only, as the feed publishes them.
 */

export type Lang = "fi" | "en" | "sv";
export const ZONE = "Europe/Helsinki";

const WORDS = {
  title: { fi: "Missä ja milloin häiriöitä on", en: "Where and when alerts happen", sv: "Var och när störningar sker" },
  page: { fi: "Häiriöt", en: "Alerts", sv: "Störningar" },
  loading: { fi: "Luetaan tiedotteita…", en: "Reading the alerts…", sv: "Läser meddelandena…" },
  analysing: { fi: "Lasketaan…", en: "Working it out…", sv: "Räknar ut…" },
  unreadable: { fi: "Tiedotteita ei voitu lukea.", en: "The alerts could not be read.", sv: "Meddelandena kunde inte läsas." },
  unreadableStatus: { fi: "Tiedotteita ei voitu lukea (HTTP {status}). Yritä myöhemmin uudelleen.", en: "The alerts could not be read (HTTP {status}). Try again later.", sv: "Meddelandena kunde inte läsas (HTTP {status}). Försök igen senare." },
  analysisFailed: { fi: "Laskenta epäonnistui: {reason}", en: "The analysis failed: {reason}", sv: "Analysen misslyckades: {reason}" },
  retry: { fi: "Yritä uudelleen", en: "Retry", sv: "Försök igen" },
  none: { fi: "Ei tiedotteita.", en: "No alerts.", sv: "Inga meddelanden." },
  noneMatch: { fi: "Mikään tiedote ei vastaa valintoja.", en: "No alert matches the filters.", sv: "Inget meddelande motsvarar filtren." },
  summary: {
    fi: "{kept} {kept|tiedote|tiedotetta} {first}–{last}. Eniten kuusikulmiossa: {hot} {hot|tiedote|tiedotetta}. Vilkkain alkamistunti: {busiest}.",
    en: "{kept} {kept|alert|alerts} from {first} to {last}. Most in one hexagon: {hot}. Busiest starting hour: {busiest}.",
    sv: "{kept} {kept|meddelande|meddelanden} från {first} till {last}. Flest i en sexhörning: {hot}. Livligaste starttimme: {busiest}.",
  },
  summaryNoTime: { fi: "{kept} {kept|tiedote|tiedotetta}. Eniten kuusikulmiossa: {hot} {hot|tiedote|tiedotetta}.", en: "{kept} {kept|alert|alerts}. Most in one hexagon: {hot}.", sv: "{kept} {kept|meddelande|meddelanden}. Flest i en sexhörning: {hot}." },
  unlocated: { fi: "{n} tiedotteella ei ole sijaintia, eikä {n|sitä|niitä} näy kartalla.", en: "{n} {n|alert has|alerts have} no location and {n|is|are} not on the map.", sv: "{n} {n|meddelande saknar|meddelanden saknar} plats och {n|syns|syns} inte på kartan." },
  filters: { fi: "Rajaa tiedotteita", en: "Filter the alerts", sv: "Filtrera meddelandena" },
  from: { fi: "Alkaen", en: "From", sv: "Från" },
  to: { fi: "Päättyen", en: "To", sv: "Till" },
  kinds: { fi: "Tyyppi", en: "Kind", sv: "Typ" },
  clear: { fi: "Tyhjennä valinnat", en: "Clear filters", sv: "Töm filtren" },
  open: { fi: "Tiedot: {name}", en: "Details of {name}", sv: "Detaljer: {name}" },
  map: { fi: "Kartta: tiedotteet kuusikulmioittain", en: "Map: alerts per hexagon", sv: "Karta: meddelanden per sexhörning" },
  mapLegend: { fi: "Värin tummuus: tiedotteiden määrä kuusikulmiossa. Ympyrä: toistuva paikka.", en: "Darker colour: more alerts in the hexagon. Circle: a place alerts keep coming back to.", sv: "Mörkare färg: fler meddelanden i sexhörningen. Cirkel: en plats där meddelanden återkommer." },
  onMap: { fi: "{n} kuusikulmiota kartalla", en: "{n} hexagons on the map", sv: "{n} sexhörningar på kartan" },
  week: { fi: "Viikon tunnit: milloin tiedotteet alkavat (Helsingin aikaa)", en: "Hours of the week: when the alerts start (Helsinki time)", sv: "Veckans timmar: när meddelandena börjar (Helsingforstid)" },
  weekEmpty: { fi: "Valituilla tiedotteilla ei ole alkamisaikaa.", en: "The alerts chosen have no start time.", sv: "De valda meddelandena har ingen starttid." },
  picked: { fi: "Vain {when}", en: "Only {when}", sv: "Bara {when}" },
  unpick: { fi: "Näytä kaikki tunnit, ei vain {when}", en: "Show every hour, not only {when}", sv: "Visa alla timmar, inte bara {when}" },
  places: { fi: "Toistuvat paikat", en: "Places alerts keep coming back to", sv: "Platser där meddelanden återkommer" },
  placesEmpty: { fi: "Mikään paikka ei toistu: ei vähintään kolmea tiedotetta 120 metrin säteellä.", en: "No place repeats: nowhere holds three alerts within 120 metres.", sv: "Ingen plats återkommer: ingenstans finns tre meddelanden inom 120 meter." },
  hexLine: { fi: "{count} {count|tiedote|tiedotetta} tässä kuusikulmiossa", en: "{count} {count|alert|alerts} in this hexagon", sv: "{count} {count|meddelande|meddelanden} i den här sexhörningen" },
  placeLine: { fi: "{count} {count|tiedote|tiedotetta}, {radius} m säteellä", en: "{count} {count|alert|alerts} within {radius} m", sv: "{count} {count|meddelande|meddelanden} inom {radius} m" },
  weeks: { fi: "Toistuvat paikat viikoittain", en: "Repeat places week by week", sv: "Återkommande platser vecka för vecka" },
  weeksNote: {
    fi: "Sovelluksen palvelin säilyttää jokaisen viikon tuloksen myös sen jälkeen, kun syöte on pudottanut viikon tiedotteet.",
    en: "The App's server keeps each week's result, also after the feed has dropped the week's alerts.",
    sv: "Appens server sparar varje veckas resultat, också efter att flödet har tappat veckans meddelanden.",
  },
  weeksLoading: { fi: "Luetaan viikkoja…", en: "Reading the weeks…", sv: "Läser veckorna…" },
  weeksEmpty: { fi: "Palvelin ei ole vielä tallentanut yhtään viikkoa.", en: "The server has not kept any week yet.", sv: "Servern har ännu inte sparat någon vecka." },
  weeksFailed: { fi: "Viikkoja ei voitu lukea: {reason}", en: "The weeks could not be read: {reason}", sv: "Veckorna kunde inte läsas: {reason}" },
  weeksStale: { fi: "Syötettä ei voitu lukea juuri nyt: viikot ovat viimeksi tallennetut.", en: "The feed could not be read just now: these are the weeks as last kept.", sv: "Flödet kunde inte läsas just nu: det här är veckorna som de senast sparades." },
  weekOf: { fi: "Viikko alkaen", en: "Week of", sv: "Veckan från" },
  alertsColumn: { fi: "Tiedotteita", en: "Alerts", sv: "Meddelanden" },
  spotsColumn: { fi: "Toistuvia paikkoja", en: "Repeat places", sv: "Återkommande platser" },
  topColumn: { fi: "Eniten tiedotteita", en: "Most alerts at", sv: "Flest meddelanden vid" },
  reports: { fi: "Tallennetut raportit", en: "Saved reports", sv: "Sparade rapporter" },
  reportName: { fi: "Raportin nimi", en: "Report name", sv: "Rapportens namn" },
  save: { fi: "Tallenna näkymä", en: "Save this view", sv: "Spara vyn" },
  saving: { fi: "Tallennetaan…", en: "Saving…", sv: "Sparar…" },
  saved: { fi: "Raportti tallennettu: {title}.", en: "Report saved: {title}.", sv: "Rapporten sparad: {title}." },
  savedNoPicture: { fi: "Raportti tallennettu: {title}. Karttakuvaa ei voitu ottaa.", en: "Report saved: {title}. The map picture could not be taken.", sv: "Rapporten sparad: {title}. Kartbilden kunde inte tas." },
  saveFailed: { fi: "Raporttia ei voitu tallentaa: {reason}", en: "The report could not be saved: {reason}", sv: "Rapporten kunde inte sparas: {reason}" },
  nameNeeded: { fi: "Anna raportille nimi.", en: "Give the report a name.", sv: "Ge rapporten ett namn." },
  reportsEmpty: { fi: "Ei vielä raportteja.", en: "No reports yet.", sv: "Inga rapporter ännu." },
  reportsFailed: { fi: "Raportteja ei voitu lukea: {reason}", en: "The reports could not be read: {reason}", sv: "Rapporterna kunde inte läsas: {reason}" },
  openReport: { fi: "Näytä raportti {title}", en: "Show the report {title}", sv: "Visa rapporten {title}" },
  reportLine: { fi: "{kept} {kept|tiedote|tiedotetta}, tallennettu {when}", en: "{kept} {kept|alert|alerts}, saved {when}", sv: "{kept} {kept|meddelande|meddelanden}, sparad {when}" },
  picture: { fi: "Karttakuva", en: "Map picture", sv: "Kartbild" },
  pictureOf: { fi: "Avaa raportin {title} karttakuva", en: "Open the map picture of {title}", sv: "Öppna kartbilden av {title}" },
  pictureFailed: { fi: "Karttakuvaa ei voitu avata: {reason}", en: "The map picture could not be opened: {reason}", sv: "Kartbilden kunde inte öppnas: {reason}" },
  showing: { fi: "Näytetään raportti {title}.", en: "Showing the report {title}.", sv: "Visar rapporten {title}." },
  changesPage: { fi: "Muutokset", en: "Changes", sv: "Ändringar" },
  changesTitle: { fi: "Muutokset", en: "Changes", sv: "Ändringar" },
  changesSignIn: { fi: "Kirjaudu sisään nähdäksesi muutokset.", en: "Sign in to see the changes.", sv: "Logga in för att se ändringarna." },
  changesAll: { fi: "Kaikkien muutokset, uusin ensin.", en: "Everyone's changes, newest first.", sv: "Allas ändringar, nyaste först." },
  changesMine: { fi: "Omat muutoksesi, uusin ensin.", en: "Your own changes, newest first.", sv: "Dina egna ändringar, nyaste först." },
  changesLoading: { fi: "Luetaan muutoksia…", en: "Reading the changes…", sv: "Läser ändringarna…" },
  changesEmpty: { fi: "Ei muutoksia vielä.", en: "No changes yet.", sv: "Inga ändringar ännu." },
  changesFailed: { fi: "Muutoksia ei voitu lukea: {reason}", en: "The changes could not be read: {reason}", sv: "Ändringarna kunde inte läsas: {reason}" },
  refresh: { fi: "Päivitä", en: "Refresh", sv: "Uppdatera" },
  when: { fi: "Milloin", en: "When", sv: "När" },
  who: { fi: "Kuka", en: "Who", sv: "Vem" },
  what: { fi: "Mitä", en: "What", sv: "Vad" },
  insert: { fi: "Lisätty", en: "Added", sv: "Tillagd" },
  update: { fi: "Muutettu", en: "Changed", sv: "Ändrad" },
  delete: { fi: "Poistettu", en: "Removed", sv: "Borttagen" },
  edit: { fi: "Muokattu", en: "Edited", sv: "Redigerad" },
  signedOut: { fi: "Ei kirjautunut", en: "Signed out", sv: "Inte inloggad" },
  roleViewer: { fi: "Katselija", en: "Viewer", sv: "Läsare" },
  roleSteward: { fi: "Ylläpitäjä", en: "Steward", sv: "Förvaltare" },
  roleLabel: { fi: "Rooli", en: "Role", sv: "Roll" },
  myFilters: { fi: "Omat rajaukset", en: "My filters", sv: "Mina filter" },
  myFiltersNote: { fi: "Tallenna näkymän rajaukset nimellä ja ota ne myöhemmin käyttöön yhdellä napsautuksella.", en: "Keep the filters on screen under a name and apply them again with one click.", sv: "Spara filtren på skärmen under ett namn och använd dem igen med ett klick." },
  filtersSignIn: { fi: "Kirjaudu sisään tallentaaksesi rajauksia.", en: "Sign in to keep filters.", sv: "Logga in för att spara filter." },
  filtersNone: { fi: "Et ole vielä tallentanut rajausta.", en: "You have not kept a filter yet.", sv: "Du har inte sparat något filter ännu." },
  filtersReading: { fi: "Luetaan rajauksia…", en: "Reading the filters…", sv: "Läser filtren…" },
  filtersFailed: { fi: "Rajauksia ei voitu lukea: {reason}", en: "The filters could not be read: {reason}", sv: "Filtren kunde inte läsas: {reason}" },
  filterName: { fi: "Rajauksen nimi", en: "Name of the filter", sv: "Filtrets namn" },
  keepFilter: { fi: "Tallenna rajaukset", en: "Keep these filters", sv: "Spara filtren" },
  filterNeedsName: { fi: "Anna rajaukselle nimi (enintään 80 merkkiä).", en: "Give the filter a name (at most 80 characters).", sv: "Ge filtret ett namn (högst 80 tecken)." },
  filterKept: { fi: "Tallennettu: {name}", en: "Kept: {name}", sv: "Sparat: {name}" },
  applyFilter: { fi: "Käytä rajausta {name}", en: "Apply {name}", sv: "Använd {name}" },
  forgetFilter: { fi: "Poista rajaus {name}", en: "Delete {name}", sv: "Radera {name}" },
  filterForgotten: { fi: "Poistettu: {name}", en: "Deleted: {name}", sv: "Raderat: {name}" },
  refused: { fi: "Ei tallennettu: {reason}", en: "Not saved: {reason}", sv: "Inte sparat: {reason}" },
  acks: { fi: "Uusimmat tiedotteet ja kuittaukset", en: "Newest alerts and acknowledgements", sv: "Nyaste meddelanden och kvitteringar" },
  acksNote: { fi: "Ylläpitäjä kuittaa tiedotteen kommentilla; kuittaus näkyy kaikille.", en: "A steward acknowledges an alert with a comment; everyone sees it.", sv: "En förvaltare kvitterar ett meddelande med en kommentar; alla ser den." },
  acksFailed: { fi: "Kuittauksia ei voitu lukea: {reason}", en: "The acknowledgements could not be read: {reason}", sv: "Kvitteringarna kunde inte läsas: {reason}" },
  ackedBy: { fi: "Kuitattu: {comment} ({who}, {at})", en: "Acknowledged: {comment} ({who}, {at})", sv: "Kvitterat: {comment} ({who}, {at})" },
  notAcked: { fi: "Ei kuitattu.", en: "Not acknowledged.", sv: "Inte kvitterat." },
  comment: { fi: "Kommentti: {name}", en: "Comment on {name}", sv: "Kommentar om {name}" },
  acknowledge: { fi: "Kuittaa {name}", en: "Acknowledge {name}", sv: "Kvittera {name}" },
  unacknowledge: { fi: "Peru kuittaus: {name}", en: "Take back the acknowledgement of {name}", sv: "Ångra kvitteringen av {name}" },
  ackButton: { fi: "Kuittaa", en: "Acknowledge", sv: "Kvittera" },
  unackButton: { fi: "Peru kuittaus", en: "Take back", sv: "Ångra" },
  acknowledged: { fi: "Kuitattu: {name}", en: "Acknowledged: {name}", sv: "Kvitterat: {name}" },
  unacknowledged: { fi: "Kuittaus peruttu: {name}", en: "Acknowledgement taken back: {name}", sv: "Kvitteringen ångrad: {name}" },
  commentNeeded: { fi: "Kirjoita kommentti (enintään 300 merkkiä).", en: "Write a comment (at most 300 characters).", sv: "Skriv en kommentar (högst 300 tecken)." },
  ackStewardOnly: { fi: "Vain ylläpitäjä kuittaa tiedotteita.", en: "Only a steward acknowledges alerts.", sv: "Bara en förvaltare kvitterar meddelanden." },
  ackSignIn: { fi: "Kirjaudu sisään ylläpitäjänä kuitataksesi tiedotteita.", en: "Sign in as a steward to acknowledge alerts.", sv: "Logga in som förvaltare för att kvittera meddelanden." },
} as const;

export type Word = keyof typeof WORDS;

const DAYS = {
  fi: ["maanantai", "tiistai", "keskiviikko", "torstai", "perjantai", "lauantai", "sunnuntai"],
  en: ["Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday", "Sunday"],
  sv: ["måndag", "tisdag", "onsdag", "torsdag", "fredag", "lördag", "söndag"],
};
const SHORT_DAYS = {
  fi: ["ma", "ti", "ke", "to", "pe", "la", "su"],
  en: ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"],
  sv: ["mån", "tis", "ons", "tors", "fre", "lör", "sön"],
};

/** The Smart Data Models Alert codes the Helsinki feeds write (T-3950); an alert without one is "Other". */
const SUB_CATEGORIES: Record<string, Record<Lang, string>> = {
  roadWorks: { fi: "Tietyö", en: "Road work", sv: "Vägarbete" },
  roadClosed: { fi: "Tie suljettu", en: "Road closed", sv: "Vägen avstängd" },
  carAccident: { fi: "Onnettomuus", en: "Accident", sv: "Olycka" },
};

/**
 * One phrase in a language, its `{name}` slots filled. `{name|one|other}` is the word that agrees
 * with the count in slot `name`: `one` when the count is 1 (Finnish and English alike), else `other`.
 */
export function t(lang: Lang, word: Word, slots: Record<string, string | number> = {}): string {
  return WORDS[word][lang]
    .replace(/\{(\w+)\|([^|}]*)\|([^}]*)\}/g, (_, name: string, one: string, other: string) => (String(slots[name]) === "1" ? one : other))
    .replace(/\{(\w+)\}/g, (_, name: string) => String(slots[name] ?? `{${name}}`));
}

/** The language of the page: `?lang=` when it names one, else the browser's. */
export function langOf(search: string, browser: readonly string[]): Lang {
  const asked = new URLSearchParams(search).get("lang");
  if (asked === "fi" || asked === "en" || asked === "sv") return asked;
  const first = browser.map((tag) => /^(fi|sv|en)\b/i.exec(tag)?.[1].toLowerCase()).find(Boolean);
  return (first as Lang | undefined) ?? "en";
}

export const locale = (lang: Lang) => ({ fi: "fi-FI", en: "en-GB", sv: "sv-FI" })[lang];

/** A whole number in the language's way: 1 234 in Finnish, 1,234 in English. */
export function number(lang: Lang, value: number): string {
  return new Intl.NumberFormat(locale(lang), { maximumFractionDigits: 0 }).format(value);
}

/** A day on Helsinki's calendar: 8.10.2026 in Finnish, 8 Oct 2026 in English. */
export function day(lang: Lang, ms: number): string {
  return new Intl.DateTimeFormat(locale(lang), { timeZone: ZONE, day: "numeric", month: lang === "fi" ? "numeric" : "short", year: "numeric" }).format(ms);
}

/** One hour of the week: "maanantai klo 7–8" or "Monday 07:00–08:00". */
export function hourOfWeek(lang: Lang, weekday: number, hour: number): string {
  const name = DAYS[lang][weekday] ?? "";
  const next = (hour + 1) % 24;
  if (lang === "fi") return `${name} klo ${hour}–${next}`;
  if (lang === "sv") return `${name} kl. ${hour}–${next}`;
  return `${name} ${String(hour).padStart(2, "0")}:00–${String(next).padStart(2, "0")}:00`;
}

export function shortDays(lang: Lang): string[] {
  return SHORT_DAYS[lang];
}

/** A sub-category in words, or the code itself for one the App does not know. */
export function subCategory(lang: Lang, code: string): string {
  return SUB_CATEGORIES[code]?.[lang] ?? (code || { fi: "Muu", en: "Other", sv: "Övrigt" }[lang]);
}

/** A moment on Helsinki's clock, date and time, in the language's way. */
export function moment(lang: Lang, ms: number): string {
  return new Intl.DateTimeFormat(locale(lang), { timeZone: ZONE, dateStyle: "medium", timeStyle: "short" }).format(ms);
}
