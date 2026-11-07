/**
 * The App's words in Finnish, English and Swedish, and Helsinki's way of writing numbers and times.
 * The language comes from the address (`?lang=fi`), else the reader's browser's first of the three,
 * English otherwise. The indicators' names are English only, as the city's
 * pipelines write them.
 */

export type Lang = "fi" | "en" | "sv";
export const ZONE = "Europe/Helsinki";

const WORDS = {
  title: { fi: "Helsingin mittarit: trendi ja ennuste", en: "Helsinki KPIs: trend and forecast", sv: "Helsingfors mätare: trend och prognos" },
  page: { fi: "Mittarit", en: "Indicators", sv: "Mätare" },
  loading: { fi: "Luetaan mittareita…", en: "Reading the indicators…", sv: "Läser mätarna…" },
  analysing: { fi: "Lasketaan…", en: "Working it out…", sv: "Räknar…" },
  unreadable: { fi: "Mittareita ei voitu lukea.", en: "The indicators could not be read.", sv: "Mätarna kunde inte läsas." },
  unreadableStatus: { fi: "Mittareita ei voitu lukea (HTTP {status}). Yritä myöhemmin uudelleen.", en: "The indicators could not be read (HTTP {status}). Try again later.", sv: "Mätarna kunde inte läsas (HTTP {status}). Försök igen senare." },
  historyUnreadable: {
    fi: "Mittareiden historiaa ei voitu lukea (HTTP {status}): näkyvissä ovat vain nykyiset arvot.",
    en: "The indicators' history could not be read (HTTP {status}): only the current values are shown.",
    sv: "Mätarnas historik kunde inte läsas (HTTP {status}): bara de nuvarande värdena visas.",
  },
  analysisFailed: { fi: "Laskenta epäonnistui: {reason}", en: "The analysis failed: {reason}", sv: "Analysen misslyckades: {reason}" },
  retry: { fi: "Yritä uudelleen", en: "Retry", sv: "Försök igen" },
  earlier: { fi: "Aiemmat ennusteet ja toteutuma", en: "Earlier forecasts against what came", sv: "Tidigare prognoser mot utfallet" },
  earlierLoading: { fi: "Luetaan säilytettyjä ennusteita…", en: "Reading the kept forecasts…", sv: "Läser sparade prognoser…" },
  earlierNone: {
    fi: "Tämän mittarin aiempia ennusteita ei ole vielä erääntynyt. Palvelin säilyttää yhden ennusteen päivässä, joka yö klo 03.20 UTC.",
    en: "No earlier forecast of this indicator has come due yet. The server keeps one a day, made every night at 03:20 UTC.",
    sv: "Ingen tidigare prognos för den här mätaren har förfallit ännu. Servern sparar en om dagen, varje natt kl. 03.20 UTC.",
  },
  earlierHits: { fi: "{inside} / {all} mitatusta osui 95 %:n välille.", en: "{inside} of {all} measured fell within the 95 % band.", sv: "{inside} av {all} uppmätta låg inom 95 %-intervallet." },
  earlierFor: { fi: "Ajankohta", en: "For", sv: "Tidpunkt" },
  earlierMade: { fi: "Ennustettu", en: "Made on", sv: "Gjord" },
  earlierExpected: { fi: "Odotettu (95 %)", en: "Expected (95 %)", sv: "Väntat (95 %)" },
  earlierMeasured: { fi: "Mitattu", en: "Measured", sv: "Uppmätt" },
  earlierRange: { fi: "{v} ({lo}–{hi})", en: "{v} ({lo}–{hi})", sv: "{v} ({lo}–{hi})" },
  earlierNoReading: { fi: "ei mittausta", en: "no reading", sv: "ingen mätning" },
  earlierInside: { fi: "välillä", en: "within", sv: "inom" },
  earlierOutside: { fi: "välin ulkopuolella", en: "outside", sv: "utanför" },
  report: { fi: "Kuukausiraportti", en: "Monthly report", sv: "Månadsrapport" },
  reportMonth: { fi: "Kuukausi", en: "Month", sv: "Månad" },
  reportDownload: { fi: "Lataa ennusteet ja toteutuma (CSV)", en: "Download forecasts against readings (CSV)", sv: "Ladda ner prognoser mot utfall (CSV)" },
  reportBusy: { fi: "Kootaan…", en: "Putting it together…", sv: "Sammanställer…" },
  reportDownloading: { fi: "CSV-tiedosto ladataan.", en: "The CSV file is downloading.", sv: "CSV-filen laddas ner." },
  serverOffline: { fi: "Palvelimeen ei saatu yhteyttä. Tarkista yhteys ja yritä uudelleen.", en: "The server could not be reached. Check the connection and try again.", sv: "Servern kunde inte nås. Kontrollera anslutningen och försök igen." },
  serverDown: { fi: "Palvelin ei vastannut. Yritä hetken päästä uudelleen.", en: "The server did not answer. Try again in a moment.", sv: "Servern svarade inte. Försök igen om en stund." },
  serverRefused: { fi: "Palvelin ei hyväksynyt pyyntöä: {why}", en: "The server refused: {why}", sv: "Servern godtog inte begäran: {why}" },
  signInAgain: { fi: "Kirjaudu uudelleen sisään ja yritä uudelleen.", en: "Sign in again, then try again.", sv: "Logga in igen och försök sedan på nytt." },
  noRight: { fi: "Tunnuksellasi ei ole oikeutta tähän.", en: "Your account may not do this.", sv: "Ditt konto får inte göra detta." },
  storageFull: { fi: "Sovelluksen tallennustila on täynnä. Yritä huomenna uudelleen.", en: "The app's storage is full. Try again tomorrow.", sv: "Appens lagring är full. Försök igen i morgon." },
  none: { fi: "Ei mittareita.", en: "No indicators.", sv: "Inga mätare." },
  summary: {
    fi: "{total} mittaria, viimeiset {days} päivää: {rising} nousee, {falling} laskee, {flat} pysyy ennallaan. Poikkeavia pisteitä: {anomalies}.",
    en: "{total} indicators over the last {days} days: {rising} rising, {falling} falling, {flat} flat. Points that look wrong: {anomalies}.",
    sv: "{total} mätare under de senaste {days} dagarna: {rising} stiger, {falling} sjunker, {flat} oförändrade. Punkter som ser fel ut: {anomalies}.",
  },
  summaryShort: { fi: "{short} mittarilla on liian vähän historiaa ennusteeseen.", en: "{short} of them have too little history to forecast.", sv: "{short} av dem har för kort historik för en prognos." },
  filters: { fi: "Rajaa mittareita", en: "Filter the indicators", sv: "Avgränsa mätarna" },
  period: { fi: "Ajanjakso", en: "Period", sv: "Period" },
  days: { fi: "{n} päivää", en: "{n} days", sv: "{n} dagar" },
  clear: { fi: "Tyhjennä valinnat", en: "Clear filters", sv: "Töm valen" },
  odd: { fi: "Vain mittarit, joissa on poikkeamia", en: "Only indicators with points that look wrong", sv: "Bara mätare med punkter som ser fel ut" },
  details: { fi: "Kaikki tiedot", en: "All details", sv: "Alla uppgifter" },
  list: { fi: "Mittarit", en: "Indicators", sv: "Mätare" },
  up: { fi: "nousee", en: "rising", sv: "stiger" },
  down: { fi: "laskee", en: "falling", sv: "sjunker" },
  flat: { fi: "ennallaan", en: "flat", sv: "oförändrad" },
  perWeek: { fi: "{change} viikossa", en: "{change} a week", sv: "{change} i veckan" },
  now: { fi: "Nyt {value}", en: "Now {value}", sv: "Nu {value}" },
  noHistory: { fi: "Ei historiaa viimeisiltä {days} päivältä.", en: "No history in the last {days} days.", sv: "Ingen historik under de senaste {days} dagarna." },
  shortHistory: { fi: "{n} pistettä: liian vähän ennusteeseen.", en: "{n} points: too few to forecast.", sv: "{n} punkter: för få för en prognos." },
  oddCount: { fi: "{n} poikkeavaa", en: "{n} look wrong", sv: "{n} ser fel ut" },
  nothingOdd: { fi: "Yhdessäkään mittarissa ei ole poikkeavia pisteitä.", en: "No indicator has a point that looks wrong.", sv: "Ingen mätare har en punkt som ser fel ut." },
  chart: { fi: "{name}: historia ja ennuste", en: "{name}: history and forecast", sv: "{name}: historik och prognos" },
  chartEmpty: { fi: "Tällä mittarilla ei ole historiaa valitulta ajalta.", en: "This indicator has no history in the period chosen.", sv: "Den här mätaren har ingen historik under den valda perioden." },
  measured: { fi: "Mitattu", en: "Measured", sv: "Uppmätt" },
  model: { fi: "Malli", en: "Model", sv: "Modell" },
  forecast: { fi: "Ennuste", en: "Forecast", sv: "Prognos" },
  band: { fi: "95 %:n väli", en: "95 % interval", sv: "95 %-intervall" },
  anomaly: { fi: "Poikkeava", en: "Looks wrong", sv: "Ser fel ut" },
  formula: { fi: "Kaava", en: "Formula", sv: "Formel" },
  source: { fi: "Lähde", en: "Source", sv: "Källa" },
  forecastLine: { fi: "Ennuste {when}: {value} ({lo}–{hi}).", en: "Forecast for {when}: {value} ({lo} to {hi}).", sv: "Prognos för {when}: {value} ({lo}–{hi})." },
  modelLine: {
    fi: "Holt-Winters (additiivinen), {n} pistettä, askel {step}, kausi {season}.",
    en: "Holt-Winters (additive), {n} points, a step of {step}, season {season}.",
    sv: "Holt-Winters (additiv), {n} punkter, steg {step}, säsong {season}.",
  },
  noSeason: { fi: "ei kautta", en: "none", sv: "ingen" },
  seasonSteps: { fi: "{n} askelta", en: "{n} steps", sv: "{n} steg" },
  anomalies: { fi: "Poikkeavat pisteet", en: "Points that look wrong", sv: "Punkter som ser fel ut" },
  anomalyLine: { fi: "{when}: {value}, odotettu {expected}", en: "{when}: {value}, expected {expected}", sv: "{when}: {value}, väntat {expected}" },
  noAnomalies: { fi: "Ei poikkeavia pisteitä.", en: "No point looks wrong.", sv: "Ingen punkt ser fel ut." },
  minutes: { fi: "{n} min", en: "{n} min", sv: "{n} min" },
  hours: { fi: "{n} h", en: "{n} h", sv: "{n} h" },
  daysShort: { fi: "{n} vrk", en: "{n} d", sv: "{n} d" },
  roleLabel: { fi: "Roolisi", en: "Your role", sv: "Din roll" },
  roleViewer: { fi: "Katselija", en: "Viewer", sv: "Läsare" },
  roleSteward: { fi: "Ylläpitäjä", en: "Steward", sv: "Förvaltare" },
  signedOut: { fi: "Et ole kirjautunut", en: "Not signed in", sv: "Inte inloggad" },
  published: { fi: "Julkaistu ennuste", en: "Published forecast", sv: "Publicerad prognos" },
  publishedLoading: { fi: "Luetaan julkaistuja ennusteita…", en: "Reading the published forecasts…", sv: "Läser publicerade prognoser…" },
  publishedNone: { fi: "Tämän mittarin ennustetta ei ole julkaistu.", en: "No forecast of this indicator is published.", sv: "Ingen prognos för den här mätaren är publicerad." },
  publishedLine: { fi: "{by} julkaisi {when}: {days} päivän historiasta, {horizon} askelta eteenpäin.", en: "{by} published on {when}: from {days} days of history, {horizon} steps ahead.", sv: "{by} publicerade {when}: från {days} dagars historik, {horizon} steg framåt." },
  publishedEarlier: { fi: "Aiemmat julkaisut", en: "Earlier publications", sv: "Tidigare publiceringar" },
  publishLead: { fi: "Ylläpitäjä valitsee ajanjakson ja ennusteen pituuden; palvelin laskee ennusteen ja kaikki näkevät sen.", en: "A steward chooses the period and how far ahead; the server makes the forecast and every reader sees it.", sv: "En förvaltare väljer perioden och hur långt framåt; servern gör prognosen och alla läsare ser den." },
  horizon: { fi: "Askelia eteenpäin (1–48)", en: "Steps ahead (1–48)", sv: "Steg framåt (1–48)" },
  note: { fi: "Huomautus", en: "Note", sv: "Anteckning" },
  publish: { fi: "Julkaise ennuste", en: "Publish the forecast", sv: "Publicera prognosen" },
  publishOnly: { fi: "Vain ylläpitäjä julkaisee ennusteita.", en: "Only a steward publishes forecasts.", sv: "Bara en förvaltare publicerar prognoser." },
  publishDone: { fi: "Ennuste on julkaistu.", en: "The forecast is published.", sv: "Prognosen är publicerad." },
  withdraw: { fi: "Peru julkaisu", en: "Withdraw", sv: "Dra tillbaka" },
  confirmWithdraw: { fi: "Peru lopullisesti", en: "Withdraw for good", sv: "Dra tillbaka för gott" },
  cancel: { fi: "Peruuta", en: "Cancel", sv: "Avbryt" },
  withdrawn: { fi: "Julkaisu on peruttu.", en: "The forecast is withdrawn.", sv: "Prognosen är tillbakadragen." },
  gone: { fi: "Sitä ei enää ole, tai se ei ole sinun muutettavissasi.", en: "It is no longer there, or it is not yours to change.", sv: "Den finns inte längre, eller så är den inte din att ändra." },
  follow: { fi: "Seuraa", en: "Follow", sv: "Följ" },
  unfollow: { fi: "Lopeta seuraaminen", en: "Stop following", sv: "Sluta följa" },
  followSignIn: { fi: "Kirjaudu sisään seurataksesi mittareita.", en: "Sign in to follow indicators.", sv: "Logga in för att följa mätare." },
  followed: { fi: "Seuraat nyt mittaria {name}.", en: "You now follow {name}.", sv: "Du följer nu {name}." },
  unfollowed: { fi: "Et enää seuraa mittaria {name}.", en: "You no longer follow {name}.", sv: "Du följer inte längre {name}." },
  followsPage: { fi: "Seuratut", en: "Followed", sv: "Följda" },
  followsTitle: { fi: "Seuraamasi mittarit", en: "Indicators you follow", sv: "Mätare du följer" },
  followsEmpty: { fi: "Et seuraa vielä yhtään mittaria. Valitse mittari ja paina Seuraa.", en: "You follow no indicator yet. Choose one and press Follow.", sv: "Du följer ingen mätare ännu. Välj en och tryck på Följ." },
  followsLoading: { fi: "Luetaan seuraamiasi mittareita…", en: "Reading the indicators you follow…", sv: "Läser mätarna du följer…" },
  open: { fi: "Avaa", en: "Open", sv: "Öppna" },
  historyPage: { fi: "Historia", en: "History", sv: "Historik" },
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

/** One phrase in a language, its `{name}` slots filled. */
export function t(lang: Lang, word: Word, slots: Record<string, string | number> = {}): string {
  return WORDS[word][lang].replace(/\{(\w+)\}/g, (_, name: string) => String(slots[name] ?? `{${name}}`));
}

/** The language of the page: `?lang=` when it names one, else the browser's. */
export function langOf(search: string, browser: readonly string[]): Lang {
  const asked = new URLSearchParams(search).get("lang");
  if (asked === "fi" || asked === "en" || asked === "sv") return asked;
  const first = browser.map((tag) => /^(fi|sv|en)\b/i.exec(tag)?.[1].toLowerCase()).find(Boolean);
  return (first as Lang | undefined) ?? "en";
}

const locale = (lang: Lang) => ({ fi: "fi-FI", en: "en-GB", sv: "sv-FI" })[lang];

/** A whole number in the language's way: 1 234 in Finnish, 1,234 in English. */
export function number(lang: Lang, value: number): string {
  return new Intl.NumberFormat(locale(lang), { maximumFractionDigits: 0 }).format(value);
}

/** A measured value: whole above 100, two decimals above 1, three below. */
export function value(lang: Lang, v: number): string {
  const digits = Math.abs(v) >= 100 ? 0 : Math.abs(v) >= 1 ? 2 : 3;
  return new Intl.NumberFormat(locale(lang), { maximumFractionDigits: digits }).format(v);
}

/** A change as a signed share: +3,5 % in Finnish, +3.5% in English. */
export function percent(lang: Lang, share: number): string {
  return new Intl.NumberFormat(locale(lang), { style: "percent", maximumFractionDigits: 1, signDisplay: "exceptZero" }).format(share);
}

/** A day on Helsinki's calendar, short, for a time axis: 3.10. in Finnish, 3 Oct in English. */
export function dayShort(lang: Lang, ms: number): string {
  return new Intl.DateTimeFormat(locale(lang), { timeZone: ZONE, day: "numeric", month: lang === "fi" ? "numeric" : "short" }).format(ms);
}

/** A moment on Helsinki's clock: 8.10.2026 klo 10.00 in Finnish, 8 Oct 2026, 10:00 in English. */
export function moment(lang: Lang, ms: number): string {
  return new Intl.DateTimeFormat(locale(lang), {
    timeZone: ZONE,
    day: "numeric",
    month: lang === "fi" ? "numeric" : "short",
    year: "numeric",
    hour: "2-digit",
    minute: "2-digit",
    hourCycle: "h23",
  }).format(ms);
}

/** A step of the model's clock, in the largest whole unit that reads well. */
export function duration(lang: Lang, ms: number): string {
  const minutes = ms / 60_000;
  if (minutes < 120) return t(lang, "minutes", { n: number(lang, minutes) });
  if (minutes < 48 * 60) return t(lang, "hours", { n: number(lang, minutes / 60) });
  return t(lang, "daysShort", { n: number(lang, minutes / 1440) });
}
