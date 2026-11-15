/**
 * Localization and strings in Finnish, English and Swedish for data-quality-inspector.
 */

export type Lang = "fi" | "en" | "sv";

const WORDS = {
  title: { fi: "Helsingin datan laatu", en: "Helsinki data quality", sv: "Helsingfors datakvalitet" },
  page: { fi: "Helsingin datan laatu", en: "Helsinki data quality", sv: "Helsingfors datakvalitet" },
  typesList: { fi: "Tietotyypit", en: "Entity types", sv: "Datatyper" },
  attributesTable: { fi: "Tyypin {type} ominaisuudet", en: "Attributes of {type}", sv: "Egenskaper för {type}" },
  failingTable: { fi: "Virheelliset kohteet", en: "Failing entities", sv: "Felaktiga objekt" },
  trend: { fi: "Laadun kehitys: palvelimen säilyttämät ajot", en: "Quality over time: the runs the server keeps", sv: "Kvaliteten över tid: körningarna servern sparar" },
  trendNote: {
    fi: "Sovelluksen palvelin tarkistaa datan kerran päivässä klo 02.40 UTC (enintään 2 000 kohdetta tyyppiä kohden) ja säilyttää jokaisen ajon tulokset. Täysi raportti listaa jokaisen kohteen jokaisen havainnon. Datan nykytila näkyy yllä.",
    en: "The App's server inspects the data once a day at 02:40 UTC (at most 2,000 entities per type) and keeps every run's scores. A full report lists every finding of every entity. The data as it is now is shown above.",
    sv: "Appens server granskar datan en gång om dagen kl. 02.40 UTC (högst 2 000 objekt per typ) och sparar varje körnings resultat. En fullständig rapport listar varje fynd för varje objekt. Datan som den är nu visas ovan.",
  },
  trendReading: { fi: "Luetaan ajoja…", en: "Reading the runs…", sv: "Läser körningarna…" },
  trendEmpty: { fi: "Palvelin ei ole vielä säilyttänyt yhtään ajoa. Ensimmäinen ajo tehdään seuraavan kerran klo 02.40 UTC; datan nykytila näkyy yllä.", en: "The server has not kept any run yet. The first one runs at the next 02:40 UTC; the data as it is now is shown above.", sv: "Servern har inte sparat någon körning ännu. Den första görs nästa gång klockan är 02.40 UTC; datan som den är nu visas ovan." },
  trendFailed: { fi: "Ajoja ei voitu lukea: {reason}", en: "The runs could not be read: {reason}", sv: "Körningarna kunde inte läsas: {reason}" },
  trendStale: { fi: "Viimeisin ajo on yli vuorokauden vanha: päivittäinen ajo ei onnistunut. Sovelluksen omistaja näkee syyn portaalissa.", en: "The newest run is over a day old: the daily run did not succeed. The App's owner sees why in the Portal.", sv: "Den senaste körningen är över ett dygn gammal: den dagliga körningen lyckades inte. Appens ägare ser varför i portalen." },
  ranAt: { fi: "Ajettu", en: "Run at", sv: "Körd" },
  entitiesColumn: { fi: "Kohteita", en: "Entities", sv: "Objekt" },
  completeColumn: { fi: "Täydellisyys", en: "Complete", sv: "Fullständighet" },
  validColumn: { fi: "Kelvollisia", en: "Valid", sv: "Giltiga" },
  findingsColumn: { fi: "Havaintoja", en: "Findings", sv: "Fynd" },
  fullReport: { fi: "Täysi raportti", en: "Full report", sv: "Fullständig rapport" },
  fullReportOf: { fi: "Lataa ajon {when} täysi raportti", en: "Download the full report of the run at {when}", sv: "Ladda ner den fullständiga rapporten för körningen {when}" },
  reportFailed: { fi: "Raporttia ei voitu ladata: {reason}", en: "The report could not be downloaded: {reason}", sv: "Rapporten kunde inte laddas ner: {reason}" },
  loading: { fi: "Luetaan tietoja…", en: "Reading data…", sv: "Läser data…" },
  loadingProgress: { fi: "Luetaan {done} / {total} tyypistä…", en: "Reading {done} of {total} types…", sv: "Läser {done} av {total} typer…" },
  analysing: { fi: "Tarkastetaan…", en: "Inspecting…", sv: "Granskar…" },
  schemaUnreadable: {
    fi: "Julkaistua skeemaa ei voitu lukea; kelvollisuutta ei tarkisteta.",
    en: "The published schema could not be read; validity is not checked.",
    sv: "Det publicerade schemat kunde inte läsas; giltigheten kontrolleras inte.",
  },
  retry: { fi: "Yritä uudelleen", en: "Retry", sv: "Försök igen" },
  typeReadError: { fi: "Tietoja ei voitu lukea.", en: "Data could not be read.", sv: "Datan kunde inte läsas." },
  allTypesFailed: { fi: "Minkään tietotyypin kohteita ei voitu lukea.", en: "Could not read entities for any type.", sv: "Objekten kunde inte läsas för någon typ." },
  noSchema: { fi: "ei julkaistua skeemaa", en: "no published schema", sv: "inget publicerat schema" },
  noTimestamp: { fi: "ei aikaleimaa", en: "no timestamp", sv: "ingen tidsstämpel" },
  entities: { fi: "Kohteet", en: "Entities", sv: "Objekt" },
  completeness: { fi: "Kattavuus", en: "Completeness", sv: "Fullständighet" },
  valid: { fi: "Kelvollinen", en: "Valid", sv: "Giltig" },
  medianAge: { fi: "Mediaani-ikä", en: "Median age", sv: "Medianålder" },
  oldest: { fi: "Vanhin", en: "Oldest", sv: "Äldsta" },
  freshness: { fi: "Aikaleiman tuoreus", en: "Freshness", sv: "Färskhet" },
  freshnessField: { fi: "Tuoreuden ominaisuus", en: "Freshness attribute", sv: "Färskhetens egenskap" },
  olderThan24hTitle: { fi: "Yli 24 h vanhat", en: "Older than 24 h", sv: "Äldre än 24 h" },
  olderThan24h: { fi: "{pct} yli 24 h vanhoja", en: "{pct} older than 24 h", sv: "{pct} äldre än 24 h" },
  chartTitle: { fi: "Tietotyyppien kattavuus", en: "Completeness by type", sv: "Fullständighet per typ" },
  chartEmpty: { fi: "Ei kattavuustietoja kaavioon.", en: "No completeness data to chart.", sv: "Inga fullständighetsuppgifter att visa." },
  overview: { fi: "Yleiskatsaus", en: "Overview", sv: "Översikt" },
  allTypes: { fi: "Kaikki tyypit", en: "All types", sv: "Alla typer" },
  attribute: { fi: "Ominaisuus", en: "Attribute", sv: "Egenskap" },
  required: { fi: "Pakollinen", en: "Required", sv: "Obligatorisk" },
  notChecked: { fi: "Ei tarkistettu", en: "Not checked", sv: "Inte kontrollerad" },
  yes: { fi: "kyllä", en: "yes", sv: "ja" },
  no: { fi: "ei", en: "no", sv: "nej" },
  id: { fi: "Tunniste", en: "ID", sv: "ID" },
  failedRule: { fi: "Hylkäyssyy", en: "Failed rule", sv: "Underkänd regel" },
  failingCapped: {
    fi: "Näytetään ensimmäiset {shown} / {total} virheellisestä kohteesta.",
    en: "Showing first {shown} of {total} failing entities.",
    sv: "Visar de första {shown} av {total} felaktiga objekt.",
  },
  allValid: { fi: "Kaikki kohteet ovat kelvollisia.", en: "All entities pass validation.", sv: "Alla objekt är giltiga." },
  noEntities: { fi: "Ei kohteita.", en: "No entities.", sv: "Inga objekt." },
  noSchemaDetails: {
    fi: "Ei julkaistua skeemaa; kelvollisuutta ei voitu tarkistaa.",
    en: "No published schema; validity could not be checked.",
    sv: "Inget publicerat schema; giltigheten kunde inte kontrolleras.",
  },
  inspectFailed: { fi: "Tarkastus epäonnistui: {reason}", en: "Inspection failed: {reason}", sv: "Granskningen misslyckades: {reason}" },

  // Rule wording
  rule_datetime: { fi: "ei kelvollinen aikaleima", en: "not a date-time", sv: "inte en tidpunkt" },
  rule_date: { fi: "ei kelvollinen päivämäärä", en: "not a date", sv: "inte ett datum" },
  rule_uri: { fi: "ei kelvollinen URI", en: "not a URI", sv: "inte en URI" },
  rule_minimum: { fi: "alle minimin {detail}", en: "below the minimum {detail}", sv: "under minimum {detail}" },
  rule_maximum: { fi: "yli maksimin {detail}", en: "above the maximum {detail}", sv: "över maximum {detail}" },
  rule_enum: { fi: "ei mikään näistä: {detail}", en: "not one of: {detail}", sv: "inte något av: {detail}" },
  rule_required: { fi: "puuttuu, mutta pakollinen", en: "missing, but required", sv: "saknas, men obligatorisk" },
  rule_unknown: { fi: "ei mallissa", en: "not in the model", sv: "inte i modellen" },
  rule_pattern: { fi: "ei vastaa säännöllistä lauseketta {detail}", en: "does not match pattern {detail}", sv: "matchar inte mönstret {detail}" },
  rule_type: { fi: "ei tyyppiä {detail}", en: "not of type {detail}", sv: "inte av typen {detail}" },
  rule_format_other: { fi: "ei kelvollinen {detail}", en: "not a valid {detail}", sv: "inte en giltig {detail}" },
  roleLabel: { fi: "Roolisi", en: "Your role", sv: "Din roll" },
  roleViewer: { fi: "Katselija", en: "Viewer", sv: "Läsare" },
  roleSteward: { fi: "Ylläpitäjä", en: "Steward", sv: "Förvaltare" },
  signedOut: { fi: "Et ole kirjautunut", en: "Not signed in", sv: "Inte inloggad" },
  historyPage: { fi: "Historia", en: "History", sv: "Historik" },
  state: { fi: "Tila", en: "State", sv: "Läge" },
  open: { fi: "Avoin", en: "Open", sv: "Öppen" },
  resolvedBy: { fi: "Ratkaissut {by}: {note}", en: "Resolved by {by}: {note}", sv: "Löst av {by}: {note}" },
  correctedBy: { fi: "Korjannut {by}: {note}", en: "Corrected by {by}: {note}", sv: "Rättat av {by}: {note}" },
  resolve: { fi: "Ratkaise", en: "Resolve", sv: "Lös" },
  stewardOnly: { fi: "Vain ylläpitäjä ratkaisee ja korjaa havaintoja.", en: "Only a steward resolves or corrects findings.", sv: "Bara en förvaltare löser och rättar fynd." },
  actionTitle: { fi: "Ratkaise: {attribute} kohteessa {id}", en: "Resolve: {attribute} of {id}", sv: "Lös: {attribute} i {id}" },
  markResolved: { fi: "Merkitse ratkaistuksi", en: "Mark resolved", sv: "Markera som löst" },
  note: { fi: "Huomautus", en: "Note", sv: "Anteckning" },
  correctLead: { fi: "Tai korjaa arvo: sovellus kirjoittaa sen päätepisteensä kautta, ja havainto ratkeaa.", en: "Or correct the value: the App writes it through its Endpoint, and the finding is resolved.", sv: "Eller rätta värdet: appen skriver det genom sin slutpunkt, och fyndet är löst." },
  value: { fi: "Oikea arvo", en: "Correct value", sv: "Rätt värde" },
  valueKind: { fi: "Arvon laji", en: "Kind of value", sv: "Typ av värde" },
  kindText: { fi: "Teksti", en: "Text", sv: "Text" },
  kindNumber: { fi: "Luku", en: "Number", sv: "Tal" },
  kindBoolean: { fi: "Kyllä tai ei", en: "Yes or no", sv: "Ja eller nej" },
  correct: { fi: "Korjaa arvo", en: "Correct the value", sv: "Rätta värdet" },
  resolvedDone: { fi: "Havainto on ratkaistu.", en: "The finding is resolved.", sv: "Fyndet är löst." },
  correctedDone: { fi: "Arvo on korjattu ja havainto ratkaistu.", en: "The value is corrected and the finding resolved.", sv: "Värdet är rättat och fyndet löst." },
  reopen: { fi: "Avaa uudelleen", en: "Open again", sv: "Öppna igen" },
  reopened: { fi: "Havainto on taas avoin.", en: "The finding is open again.", sv: "Fyndet är öppet igen." },
  cancel: { fi: "Peruuta", en: "Cancel", sv: "Avbryt" },
  gone: { fi: "Sitä ei enää ole, tai se ei ole sinun muutettavissasi.", en: "It is no longer there, or it is not yours to change.", sv: "Den finns inte längre, eller så är den inte din att ändra." },
  noRight: { fi: "Tunnuksellasi ei ole oikeutta tähän.", en: "Your account may not do this.", sv: "Ditt konto får inte göra detta." },
  signInAgain: { fi: "Kirjaudu uudelleen sisään ja yritä uudelleen.", en: "Sign in again, then try again.", sv: "Logga in igen och försök sedan på nytt." },
  serverOffline: { fi: "Palvelimeen ei saatu yhteyttä. Tarkista yhteys ja yritä uudelleen.", en: "The server could not be reached. Check the connection and try again.", sv: "Servern kunde inte nås. Kontrollera anslutningen och försök igen." },
  serverDown: { fi: "Palvelin ei vastannut. Yritä hetken päästä uudelleen.", en: "The server did not answer. Try again in a moment.", sv: "Servern svarade inte. Försök igen om en stund." },
  serverRefused: { fi: "Palvelin ei hyväksynyt pyyntöä: {why}", en: "The server refused: {why}", sv: "Servern godtog inte begäran: {why}" },
  historyTitle: { fi: "Muutokset", en: "Changes", sv: "Ändringar" },
  historyMine: { fi: "Omat muutoksesi; ylläpitäjä näkee kaikkien.", en: "Your own changes; a steward sees everyone's.", sv: "Dina egna ändringar; en förvaltare ser allas." },
  historyAll: { fi: "Kaikkien muutokset, uusin ensin.", en: "Everyone's changes, newest first.", sv: "Allas ändringar, nyaste först." },
  historyEmpty: { fi: "Muutoksia ei ole vielä.", en: "No change yet.", sv: "Inga ändringar ännu." },
  historyLoading: { fi: "Luetaan historiaa…", en: "Reading the history…", sv: "Läser historiken…" },
  historySignIn: { fi: "Kirjaudu sisään nähdäksesi historian.", en: "Sign in to see the history.", sv: "Logga in för att se historiken." },
  refresh: { fi: "Päivitä", en: "Refresh", sv: "Uppdatera" },
  when: { fi: "Milloin", en: "When", sv: "När" },
  who: { fi: "Kuka", en: "Who", sv: "Vem" },
  what: { fi: "Mitä", en: "What", sv: "Vad" },
  insert: { fi: "lisäsi", en: "added", sv: "lade till" },
  update: { fi: "muutti", en: "changed", sv: "ändrade" },
  delete: { fi: "poisti", en: "removed", sv: "tog bort" },
  edit: { fi: "muokkasi", en: "edited", sv: "redigerade" },
} as const;

export type Word = keyof typeof WORDS;

/** Formats a localized string with `{slot}` replacements. */
export function t(lang: Lang, word: Word, slots: Record<string, string | number> = {}): string {
  const text = WORDS[word]?.[lang] ?? String(word);
  return text.replace(/\{(\w+)\}/g, (_, name: string) => String(slots[name] ?? `{${name}}`));
}

/** Determines active language from search parameters or navigator preferences. */
export function langOf(search: string, browser: readonly string[]): Lang {
  const asked = new URLSearchParams(search).get("lang");
  if (asked === "fi" || asked === "en" || asked === "sv") return asked;
  const first = browser.map((tag) => /^(fi|sv|en)\b/i.exec(tag)?.[1].toLowerCase()).find(Boolean);
  return (first as Lang | undefined) ?? "en";
}

export const locale = (lang: Lang) => ({ fi: "fi-FI", en: "en-GB", sv: "sv-FI" })[lang];

/** Formats whole or decimal numbers according to the active locale. */
export function number(lang: Lang, value: number, decimals = 0): string {
  return new Intl.NumberFormat(locale(lang), {
    maximumFractionDigits: decimals,
    minimumFractionDigits: decimals > 0 && !Number.isInteger(value) ? 1 : 0,
  }).format(value);
}

/** Formats duration in seconds into human-readable age words. */
export function ageMessage(lang: Lang, seconds: number): string {
  if (!Number.isFinite(seconds) || seconds < 0) return "0 min";
  if (seconds < 60) {
    return `${Math.round(seconds)} s`;
  }
  if (seconds < 3600) {
    return `${Math.round(seconds / 60)} min`;
  }
  if (seconds < 86400) {
    return `${Math.round(seconds / 3600)} h`;
  }
  const days = Math.round(seconds / 86400);
  if (lang === "fi") return `${days} pv`;
  if (lang === "sv") return `${days} ${days === 1 ? "dag" : "dagar"}`;
  return `${days} ${days === 1 ? "day" : "days"}`;
}

/** Words a validation rule failure in natural phrasing. */
export function ruleMessage(lang: Lang, rule: string, detail: string): string {
  if (rule === "format") {
    if (detail === "date-time") return t(lang, "rule_datetime");
    if (detail === "date") return t(lang, "rule_date");
    if (detail === "uri") return t(lang, "rule_uri");
    return t(lang, "rule_format_other", { detail });
  }
  if (rule === "minimum") return t(lang, "rule_minimum", { detail });
  if (rule === "maximum") return t(lang, "rule_maximum", { detail });
  if (rule === "enum") return t(lang, "rule_enum", { detail });
  if (rule === "required") return t(lang, "rule_required");
  if (rule === "unknown") return t(lang, "rule_unknown");
  if (rule === "pattern") return t(lang, "rule_pattern", { detail });
  if (rule === "type") return t(lang, "rule_type", { detail });
  return detail ? `${rule}: ${detail}` : rule;
}
