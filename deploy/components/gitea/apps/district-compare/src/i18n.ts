/**
 * Localization and UI strings in Finnish, English and Swedish for district-compare.
 */
import type { Measure } from "./districts";

export type Lang = "fi" | "en" | "sv";
export const ZONE = "Europe/Helsinki";

const WORDS = {
  title: { fi: "Vertaa kaupunginosia", en: "Compare districts", sv: "Jämför stadsdelar" },
  page: { fi: "Vertaa kaupunginosia", en: "Compare districts", sv: "Jämför stadsdelar" },
  measure: { fi: "Mittari", en: "Measure", sv: "Mått" },
  districtsRanked: { fi: "Kaupunginosat järjestyksessä", en: "Districts ranked", sv: "Stadsdelar i ordning" },
  selectedDistrictsCompared: { fi: "Valitut kaupunginosat rinnakkain", en: "Selected districts compared", sv: "Valda stadsdelar sida vid sida" },
  measure_events: { fi: "Tapahtumat", en: "Events", sv: "Evenemang" },
  measure_bikes: { fi: "Kaupunkipyöräasemat", en: "City-bike stations", sv: "Stadscykelstationer" },
  measure_bikeSlots: { fi: "Pyörätelinepaikat", en: "Bike slots", sv: "Cykelplatser" },
  measure_alerts: { fi: "Liikennetiedotteet", en: "Traffic alerts", sv: "Trafikmeddelanden" },
  measure_pm25: { fi: "Pienhiukkaset (PM2.5)", en: "Fine particles (PM2.5)", sv: "Fina partiklar (PM2.5)" },
  measure_aqi: { fi: "Ilmanlaatuindeksi", en: "Air quality index", sv: "Luftkvalitetsindex" },
  area: { fi: "Pinta-ala", en: "Area", sv: "Areal" },
  km2: { fi: "km²", en: "km²", sv: "km²" },
  perKm2: { fi: "/ km²", en: "/ km²", sv: "/ km²" },
  ugm3: { fi: "µg/m³", en: "µg/m³", sv: "µg/m³" },
  noStation: { fi: "ei mittausasemaa", en: "no station", sv: "ingen mätstation" },
  unlocatedEvents: { fi: "{n} tapahtumaa ilman sijaintitietoa.", en: "{n} events without a place.", sv: "{n} evenemang utan plats." },
  unlocatedEvent: { fi: "1 tapahtuma ilman sijaintitietoa.", en: "1 event without a place.", sv: "1 evenemang utan plats." },
  emptyDistricts: { fi: "Kaupunginosien rajoja ei voitu lukea.", en: "No district boundaries readable.", sv: "Stadsdelarnas gränser kunde inte läsas." },
  loading: { fi: "Luetaan tietoja…", en: "Reading data…", sv: "Läser data…" },
  analysing: { fi: "Lasketaan vertailua…", en: "Calculating comparison…", sv: "Beräknar jämförelsen…" },
  unreadable: { fi: "Tietoja ei voitu lukea.", en: "Data could not be read.", sv: "Data kunde inte läsas." },
  unreadableStatus: {
    fi: "Tietoja ei voitu lukea (HTTP {status}). Yritä myöhemmin uudelleen.",
    en: "Data could not be read (HTTP {status}). Try again later.",
    sv: "Data kunde inte läsas (HTTP {status}). Försök igen senare.",
  },
  partialFailed: { fi: "Tietotyyppejä ei voitu lukea: {types}", en: "Some data could not be read: {types}", sv: "En del data kunde inte läsas: {types}" },
  analysisFailed: { fi: "Vertailu epäonnistui: {reason}", en: "Comparison failed: {reason}", sv: "Jämförelsen misslyckades: {reason}" },
  clear: { fi: "Tyhjennä valinnat", en: "Clear selection", sv: "Töm valet" },
  map: { fi: "Kartta: kaupunginosat", en: "Map: districts", sv: "Karta: stadsdelar" },
  mapLegend: {
    fi: "Väri sinisestä, mittarin pienin arvo, oranssiin, suurin; vaalea: ei arvoa. Paksu reuna: valittu kaupunginosa.",
    en: "Colour from blue, the lowest value of the measure, to orange, the highest; pale: no value. Thick border: a selected district.",
    sv: "Färg från blått, måttets lägsta värde, till orange, det högsta; blek: inget värde. Tjock kant: en vald stadsdel.",
  },
  chartTitle: { fi: "Valitut kaupunginosat: {measure}", en: "Selected districts: {measure}", sv: "Valda stadsdelar: {measure}" },
  history: { fi: "Valittujen kaupunginosien historia: {measure}", en: "History of the selected districts: {measure}", sv: "De valda stadsdelarnas historik: {measure}" },
  historyNote: {
    fi: "Sovelluksen palvelin laskee joka päivä kunkin kaupunginosan luvut ja säilyttää ne, joten kaupunginosalla on historia.",
    en: "The App's server computes every district's figures each day and keeps them, so a district has a history.",
    sv: "Appens server räknar varje stadsdels siffror varje dag och sparar dem, så att en stadsdel har en historik.",
  },
  historyPick: { fi: "Valitse kaupunginosia nähdäksesi niiden historian.", en: "Select districts to see their history.", sv: "Välj stadsdelar för att se deras historik." },
  historyReading: { fi: "Luetaan säilytettyjä päiviä…", en: "Reading the kept days…", sv: "Läser de sparade dagarna…" },
  historyEmpty: { fi: "Palvelin ei ole vielä säilyttänyt yhtään päivää näille kaupunginosille.", en: "The server has not kept any day of these districts yet.", sv: "Servern har ännu inte sparat någon dag för de här stadsdelarna." },
  historyFailed: { fi: "Historiaa ei voitu lukea: {reason}", en: "The history could not be read: {reason}", sv: "Historiken kunde inte läsas: {reason}" },
  historyStale: { fi: "Syötteitä ei voitu lukea juuri nyt: päivät ovat viimeksi säilytetyt.", en: "The feeds could not be read just now: these are the days as last kept.", sv: "Flödena kunde inte läsas just nu: det här är dagarna som de senast sparades." },
  day: { fi: "Päivä", en: "Day", sv: "Dag" },
  boundaries: { fi: "Rajat (GeoJSON)", en: "Boundaries (GeoJSON)", sv: "Gränser (GeoJSON)" },
  boundariesOf: { fi: "Lataa kaupunginosien rajat GeoJSON-tiedostona", en: "Download the district boundaries as GeoJSON", sv: "Ladda ner stadsdelarnas gränser som GeoJSON" },
  licence: { fi: "Lisenssi", en: "Licence", sv: "Licens" },
  licenceOf: { fi: "Lataa rajojen lisenssi", en: "Download the licence of the boundaries", sv: "Ladda ner gränsernas licens" },
  filesFailed: { fi: "Tiedostoa ei voitu ladata: {reason}", en: "The file could not be downloaded: {reason}", sv: "Filen kunde inte laddas ner: {reason}" },
  chartEmpty: { fi: "Ei valittuja kaupunginosia.", en: "No districts selected.", sv: "Inga valda stadsdelar." },
  noneSelected: { fi: "Valitse vähintään yksi kaupunginosa vertailuun.", en: "Select at least one district to compare.", sv: "Välj minst en stadsdel att jämföra." },
  details: { fi: "Tiedot: {name}", en: "Details: {name}", sv: "Detaljer: {name}" },
  deselectDistrict: { fi: "Poista valinta: {name}", en: "Deselect {name}", sv: "Avmarkera {name}" },
  selectDistrict: { fi: "Valitse {name}", en: "Select {name}", sv: "Välj {name}" },
  type_AdministrativeArea: { fi: "Kaupunginosat", en: "City districts", sv: "Stadsdelar" },
  type_Event: { fi: "Tapahtumat", en: "Events", sv: "Evenemang" },
  type_BikeHireDockingStation: { fi: "Kaupunkipyöräasemat", en: "Bike stations", sv: "Cykelstationer" },
  type_Alert: { fi: "Liikennetiedotteet", en: "Alerts", sv: "Trafikmeddelanden" },
  type_AirQualityObserved: { fi: "Ilmanlaatu", en: "Air quality", sv: "Luftkvalitet" },
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
  saved: { fi: "Tallennetut vertailut", en: "Saved comparisons", sv: "Sparade jämförelser" },
  savedNote: { fi: "Tallenna näkymän vertailu nimellä ja jaa sen linkki: projektin kirjautuneet lukijat avaavat sen vain luettavaksi.", en: "Keep the comparison on screen under a name and share its link: the project's signed-in readers open it read-only.", sv: "Spara jämförelsen på skärmen under ett namn och dela dess länk: projektets inloggade läsare öppnar den skrivskyddad." },
  savedSignIn: { fi: "Kirjaudu sisään tallentaaksesi vertailuja.", en: "Sign in to keep comparisons.", sv: "Logga in för att spara jämförelser." },
  savedNone: { fi: "Et ole vielä tallentanut vertailua.", en: "You have not kept a comparison yet.", sv: "Du har inte sparat någon jämförelse ännu." },
  savedReading: { fi: "Luetaan vertailuja…", en: "Reading the comparisons…", sv: "Läser jämförelserna…" },
  savedFailed: { fi: "Vertailuja ei voitu lukea: {reason}", en: "The comparisons could not be read: {reason}", sv: "Jämförelserna kunde inte läsas: {reason}" },
  comparisonName: { fi: "Vertailun nimi", en: "Name of the comparison", sv: "Jämförelsens namn" },
  keep: { fi: "Tallenna vertailu", en: "Keep this comparison", sv: "Spara jämförelsen" },
  keepNeedsDistricts: { fi: "Valitse ensin kaupunginosia.", en: "Select districts first.", sv: "Välj stadsdelar först." },
  keepNeedsName: { fi: "Anna vertailulle nimi (enintään 80 merkkiä).", en: "Give the comparison a name (at most 80 characters).", sv: "Ge jämförelsen ett namn (högst 80 tecken)." },
  kept: { fi: "Tallennettu: {name}", en: "Kept: {name}", sv: "Sparad: {name}" },
  openComparison: { fi: "Avaa {name}", en: "Open {name}", sv: "Öppna {name}" },
  updateComparison: { fi: "Päivitä {name} näkymän mukaiseksi", en: "Update {name} to what is on screen", sv: "Uppdatera {name} till det som visas" },
  forgetComparison: { fi: "Poista {name}", en: "Delete {name}", sv: "Radera {name}" },
  shareComparison: { fi: "Jaa linkki: {name}", en: "Link to share: {name}", sv: "Länk att dela: {name}" },
  updated: { fi: "Päivitetty: {name}", en: "Updated: {name}", sv: "Uppdaterad: {name}" },
  forgotten: { fi: "Poistettu: {name}", en: "Deleted: {name}", sv: "Raderad: {name}" },
  refused: { fi: "Ei tallennettu: {reason}", en: "Not saved: {reason}", sv: "Inte sparat: {reason}" },
  comparisonLine: { fi: "{measure}: {districts}", en: "{measure}: {districts}", sv: "{measure}: {districts}" },
  sharedBy: { fi: "Jaettu vertailu {name}, tekijä {owner}: vain luettavissa.", en: "Shared comparison {name}, by {owner}: read-only.", sv: "Delad jämförelse {name}, av {owner}: skrivskyddad." },
  sharedSignIn: { fi: "Kirjaudu sisään avataksesi jaetun vertailun.", en: "Sign in to open a shared comparison.", sv: "Logga in för att öppna en delad jämförelse." },
  sharedFailed: { fi: "Jaettua vertailua ei voitu avata: {reason}", en: "The shared comparison could not be opened: {reason}", sv: "Den delade jämförelsen kunde inte öppnas: {reason}" },
  pinnedTitle: { fi: "Kaikille kiinnitetty", en: "Pinned for everyone", sv: "Fäst för alla" },
  pinnedBy: { fi: "{name}, kiinnittänyt {owner}. Sivu avautuu tähän.", en: "{name}, pinned by {owner}. The page opens on it.", sv: "{name}, fäst av {owner}. Sidan öppnas på den." },
  pinnedNone: { fi: "Mitään ei ole kiinnitetty: sivu avautuu suurimpiin kaupunginosiin.", en: "Nothing is pinned: the page opens on the largest districts.", sv: "Inget är fäst: sidan öppnas på de största stadsdelarna." },
  pin: { fi: "Kiinnitä tämä vertailu kaikille", en: "Pin this comparison for everyone", sv: "Fäst den här jämförelsen för alla" },
  unpin: { fi: "Poista kiinnitys", en: "Take the pin back", sv: "Ta bort fästningen" },
  openPinned: { fi: "Avaa kiinnitetty", en: "Open the pinned one", sv: "Öppna den fästa" },
  pinned: { fi: "Kiinnitetty: {name}", en: "Pinned: {name}", sv: "Fäst: {name}" },
  unpinned: { fi: "Kiinnitys poistettu.", en: "The pin is taken back.", sv: "Fästningen är borttagen." },
  pinStewardOnly: { fi: "Vain ylläpitäjä kiinnittää vertailun kaikille.", en: "Only a steward pins a comparison for everyone.", sv: "Bara en förvaltare fäster en jämförelse för alla." },
} as const;

export type Word = keyof typeof WORDS;

/** Formats a translated string with `{slot}` replacement. */
export function t(lang: Lang, word: Word, slots: Record<string, string | number> = {}): string {
  const text = WORDS[word]?.[lang] ?? String(word);
  return text.replace(/\{(\w+)\}/g, (_, name: string) => String(slots[name] ?? `{${name}}`));
}

/** Determines active language from search parameters or navigator preferences. */
export function langOf(search: string, browser: readonly string[]): Lang {
  const asked = new URLSearchParams(search).get("lang");
  if (asked === "fi" || asked === "en" || asked === "sv") return asked;
  // The first of the person's languages the app speaks decides.
  const first = browser.map((tag) => /^(fi|sv|en)\b/i.exec(tag)?.[1].toLowerCase()).find(Boolean);
  return (first as Lang | undefined) ?? "en";
}

export const locale = (lang: Lang) => ({ fi: "fi-FI", en: "en-GB", sv: "sv-FI" })[lang];

/** Formats numbers with localized thousands and decimal separators. */
export function number(lang: Lang, value: number, decimals = 1): string {
  return new Intl.NumberFormat(locale(lang), {
    maximumFractionDigits: decimals,
    minimumFractionDigits: decimals > 0 && !Number.isInteger(value) ? 1 : 0,
  }).format(value);
}

/** Translated name for a measure. */
export function measureTitle(lang: Lang, measure: Measure): string {
  const key = `measure_${measure}` as Word;
  return t(lang, key);
}

/** Unit suffix for a measure. */
export function measureUnit(lang: Lang, measure: Measure): string {
  if (measure === "pm25") return t(lang, "ugm3");
  return "";
}

/** Localized entity type name for loading and error notices. */
export function translateType(type: string, lang: Lang): string {
  const key = `type_${type}` as Word;
  return WORDS[key] ? t(lang, key) : type;
}

/** A moment on Helsinki's clock, date and time, in the language's way. */
export function moment(lang: Lang, ms: number): string {
  return new Intl.DateTimeFormat(locale(lang), { timeZone: ZONE, dateStyle: "medium", timeStyle: "short" }).format(ms);
}
