/**
 * Localization and UI strings in Finnish, English and Swedish for bike-weather-demand.
 */

export type Lang = "fi" | "en" | "sv";
export const ZONE = "Europe/Helsinki";

const WORDS = {
  title: { fi: "Pyörät ja sää", en: "Bikes and the weather", sv: "Cyklar och vädret" },
  page: { fi: "Pyörät ja sää", en: "Bikes and the weather", sv: "Cyklar och vädret" },
  models: { fi: "Aseman mallin historia: palvelimen säilyttämät", en: "The station's model over time: kept by the server", sv: "Stationens modell över tid: sparad av servern" },
  modelsNote: {
    fi: "Sovelluksen palvelin opettaa mallin kerran päivässä aseman edellisiltä päiviltä ja säilyttää sen, joten mallin muutokset näkyvät.",
    en: "The App's server trains the model once a day on the station's days before and keeps it, so you see how the model changes.",
    sv: "Appens server tränar modellen en gång om dagen på stationens senaste dagar och sparar den, så att du ser hur modellen ändras.",
  },
  modelsReading: { fi: "Luetaan säilytettyjä malleja…", en: "Reading the kept models…", sv: "Läser de sparade modellerna…" },
  modelsEmpty: { fi: "Palvelin ei ole vielä säilyttänyt tämän aseman mallia.", en: "The server has not kept a model of this station yet.", sv: "Servern har ännu inte sparat någon modell för den här stationen." },
  modelsFailed: { fi: "Malleja ei voitu lukea: {reason}", en: "The models could not be read: {reason}", sv: "Modellerna kunde inte läsas: {reason}" },
  modelsStale: { fi: "Dataa ei voitu lukea juuri nyt: mallit ovat viimeksi säilytetyt.", en: "The data could not be read just now: these are the models as kept.", sv: "Data kunde inte läsas just nu: det här är modellerna som de sparades." },
  trainedOn: { fi: "Opetettu", en: "Trained on", sv: "Tränad" },
  trainedHours: { fi: "Tunteja", en: "Hours", sv: "Timmar" },
  perDegree: { fi: "Pyöriä / °C", en: "Bikes per °C", sv: "Cyklar per °C" },
  rainEffect: { fi: "Sade", en: "Rain", sv: "Regn" },
  spread: { fi: "Hajonta (σ)", en: "Spread (σ)", sv: "Spridning (σ)" },
  trainingData: { fi: "Opetusdata", en: "Training data", sv: "Träningsdata" },
  trainingDataOf: { fi: "Lataa {day} opetusdata", en: "Download the training data of {day}", sv: "Ladda ner träningsdata för {day}" },
  trainingFailed: { fi: "Opetusdataa ei voitu ladata: {reason}", en: "The training data could not be downloaded: {reason}", sv: "Träningsdata kunde inte laddas ner: {reason}" },
  station: { fi: "Asema", en: "Station", sv: "Station" },
  searchStation: { fi: "Etsi asemaa…", en: "Search station…", sv: "Sök station…" },
  bikesNow: { fi: "Pyöriä nyt", en: "Bikes now", sv: "Cyklar nu" },
  freeSlots: { fi: "Vapaita telineitä", en: "Free slots", sv: "Lediga platser" },
  totalSlots: { fi: "Telineitä yhteensä", en: "Total slots", sv: "Platser totalt" },
  mapTitle: { fi: "Asemat kartalla", en: "Stations on map", sv: "Stationer på kartan" },
  mapNotice: {
    fi: "Ympyrän väri kertoo pyörien saatavuudesta asemalla. Valitse asema napsauttamalla.",
    en: "Circle colour reflects bike availability at each station. Click to select a station.",
    sv: "Cirkelns färg visar hur många cyklar stationen har. Klicka för att välja en station.",
  },
  chartTitle: { fi: "Saatavuushistoria ja 6 tunnin ennuste", en: "Availability history and 6-hour estimate", sv: "Tillgänglighet och 6 timmars prognos" },
  chartEmpty: { fi: "Ei saatavuushistoriaa.", en: "No availability history.", sv: "Ingen tillgänglighetshistorik." },
  profileTitle: { fi: "Saatavuus viikon tunneittain", en: "Availability by hour of the week", sv: "Tillgänglighet per veckotimme" },
  profileTable: { fi: "Saatavuus viikon tunneittain", en: "Availability by hour of the week", sv: "Tillgänglighet per veckotimme" },
  hour: { fi: "Tunti", en: "Hour", sv: "Timme" },
  day: { fi: "Viikonpäivä", en: "Day of week", sv: "Veckodag" },
  mon: { fi: "Ma", en: "Mon", sv: "Mån" },
  tue: { fi: "Ti", en: "Tue", sv: "Tis" },
  wed: { fi: "Ke", en: "Wed", sv: "Ons" },
  thu: { fi: "To", en: "Thu", sv: "Tors" },
  fri: { fi: "Pe", en: "Fri", sv: "Fre" },
  sat: { fi: "La", en: "Sat", sv: "Lör" },
  sun: { fi: "Su", en: "Sun", sv: "Sön" },
  weatherEffect: {
    fi: "1 °C lämpimämpi: {perDegree} pyörää; sade: {rain} pyörää; {hours} tunnista sään kanssa",
    en: "1 °C warmer: {perDegree} bikes; rain: {rain} bikes; from {hours} hours with weather",
    sv: "1 °C varmare: {perDegree} cyklar; regn: {rain} cyklar; från {hours} timmar med väder",
  },
  weatherAssumption: {
    fi: "Sää pysyy samana kuin klo {time}: {temp} °C, {rainState}",
    en: "The weather stays as it was at {time}: {temp} °C, {rainState}",
    sv: "Vädret förblir som klockan {time}: {temp} °C, {rainState}",
  },
  noRain: { fi: "ei sadetta", en: "no rain", sv: "inget regn" },
  rain: { fi: "sadetta", en: "rain", sv: "regn" },
  tooLittleHistory: {
    fi: "Liian vähän historiaa (alle 24 tuntia): näytetään viikkoprofiili ilman sääennustetta.",
    en: "Too little history (fewer than 24 hourly points): showing profile only, no regression.",
    sv: "För lite historik (färre än 24 timmar): bara veckoprofilen visas, ingen väderprognos.",
  },
  noWeatherTerms: {
    fi: "Ei sääasemaa {km} km säteellä: säävaikutuksia ei arvioida.",
    en: "No weather station within {km} km: weather effects not evaluated.",
    sv: "Ingen väderstation inom {km} km: vädrets inverkan bedöms inte.",
  },
  weatherUnreadable: {
    fi: "Säätietoja ei voitu lukea.",
    en: "Weather data could not be read.",
    sv: "Väderdata kunde inte läsas.",
  },
  loading: { fi: "Luetaan tietoja…", en: "Reading data…", sv: "Läser data…" },
  analysing: { fi: "Lasketaan ennustetta…", en: "Calculating estimate…", sv: "Beräknar prognosen…" },
  emptyStations: { fi: "Pyöräasemia ei löytynyt.", en: "No bike stations readable.", sv: "Inga cykelstationer kunde läsas." },
  noHistory: { fi: "Asemalle ei ole historiatietoja.", en: "No history available for this station.", sv: "Ingen historik för den här stationen." },
  details: { fi: "Tiedot: {name}", en: "Details: {name}", sv: "Detaljer: {name}" },
  estimationFailed: { fi: "Ennusteen laskenta epäonnistui: {reason}", en: "Estimation failed: {reason}", sv: "Prognosen misslyckades: {reason}" },
  bikesHistorySeries: { fi: "Havaittu saatavuus", en: "Observed bikes", sv: "Observerade cyklar" },
  estimateSeries: { fi: "Ennuste", en: "Estimate", sv: "Prognos" },
  estimateBand: { fi: "Vaihteluväli (80 %)", en: "Uncertainty band (80%)", sv: "Osäkerhetsband (80 %)" },
  nearestWeather: { fi: "Lähin sääasema: {name} ({dist} km)", en: "Nearest weather station: {name} ({dist} km)", sv: "Närmaste väderstation: {name} ({dist} km)" },
  chosenWeather: { fi: "Valittu sääasema: {name} ({dist} km)", en: "Chosen weather station: {name} ({dist} km)", sv: "Vald väderstation: {name} ({dist} km)" },
  trainedDays: { fi: "Päiviä", en: "Days", sv: "Dagar" },
  historyPage: { fi: "Muutokset", en: "History", sv: "Historik" },
  historyTitle: { fi: "Muutokset", en: "Changes", sv: "Ändringar" },
  historySignIn: { fi: "Kirjaudu sisään nähdäksesi muutokset.", en: "Sign in to see the changes.", sv: "Logga in för att se ändringarna." },
  historyAll: { fi: "Kaikkien muutokset, uusin ensin.", en: "Everyone's changes, newest first.", sv: "Allas ändringar, nyaste först." },
  historyMine: { fi: "Omat muutoksesi, uusin ensin.", en: "Your own changes, newest first.", sv: "Dina egna ändringar, nyaste först." },
  historyLoading: { fi: "Luetaan muutoksia…", en: "Reading the changes…", sv: "Läser ändringarna…" },
  historyEmpty: { fi: "Ei muutoksia vielä.", en: "No changes yet.", sv: "Inga ändringar ännu." },
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
  inputsTitle: { fi: "Mallin syötteet", en: "Model inputs", sv: "Modellens indata" },
  inputsReading: { fi: "Luetaan mallin syötteitä…", en: "Reading the model inputs…", sv: "Läser modellens indata…" },
  inputsFailed: { fi: "Mallin syötteitä ei voitu lukea: {reason}", en: "The model inputs could not be read: {reason}", sv: "Modellens indata kunde inte läsas: {reason}" },
  inputsNow: { fi: "Malli lukee säätä asemalta {weather} ja oppii {days} päivästä.", en: "The model reads the weather of {weather} and trains on {days} days.", sv: "Modellen läser vädret från {weather} och tränar på {days} dagar." },
  inputsNearest: { fi: "lähin {km} km:n säteellä", en: "the nearest within {km} km", sv: "den närmaste inom {km} km" },
  inputsSetBy: { fi: "Asettanut {who}, {at}.", en: "Set by {who}, {at}.", sv: "Satt av {who}, {at}." },
  inputsDefaults: { fi: "Oletukset.", en: "The defaults.", sv: "Standardvärdena." },
  inputsLead: { fi: "Muutos opettaa tämän päivän mallin uudelleen.", en: "A change trains today's model again.", sv: "En ändring tränar dagens modell på nytt." },
  inputsStewardOnly: { fi: "Vain ylläpitäjä muuttaa mallin syötteitä.", en: "Only a steward changes the model inputs.", sv: "Bara en förvaltare ändrar modellens indata." },
  inputsSignIn: { fi: "Kirjaudu sisään ylläpitäjänä muuttaaksesi mallin syötteitä.", en: "Sign in as a steward to change the model inputs.", sv: "Logga in som förvaltare för att ändra modellens indata." },
  inputsWeather: { fi: "Sääasema", en: "Weather station", sv: "Väderstation" },
  inputsNearestOption: { fi: "Lähin säteellä", en: "The nearest within the radius", sv: "Den närmaste inom radien" },
  inputsRadius: { fi: "Säde (km, 1–25)", en: "Radius (km, 1–25)", sv: "Radie (km, 1–25)" },
  inputsDays: { fi: "Opetuspäiviä (1–7)", en: "Days to train on (1–7)", sv: "Dagar att träna på (1–7)" },
  inputsNote: { fi: "Huomautus", en: "Note", sv: "Anteckning" },
  inputsSave: { fi: "Tallenna syötteet", en: "Save inputs", sv: "Spara indata" },
  inputsResetButton: { fi: "Palauta oletukset", en: "Back to the defaults", sv: "Tillbaka till standardvärdena" },
  inputsAlreadyDefaults: { fi: "Asemalla on jo oletukset.", en: "The station has the defaults already.", sv: "Stationen har redan standardvärdena." },
  inputsSaved: { fi: "Tallennettu: tämän päivän malli opetetaan uudelleen.", en: "Saved: today's model is trained again.", sv: "Sparat: dagens modell tränas på nytt." },
  inputsReset: { fi: "Oletukset palautettu: tämän päivän malli opetetaan uudelleen.", en: "Back to the defaults: today's model is trained again.", sv: "Tillbaka till standardvärdena: dagens modell tränas på nytt." },
  inputsRefused: { fi: "Ei tallennettu: {reason}", en: "Not saved: {reason}", sv: "Inte sparat: {reason}" },
  radiusRange: { fi: "Säde on 1–25 km.", en: "The radius is 1 to 25 km.", sv: "Radien är 1 till 25 km." },
  daysRange: { fi: "Malli oppii 1–7 päivästä.", en: "A model trains on 1 to 7 days.", sv: "En modell tränar på 1 till 7 dagar." },
  noteLong: { fi: "Huomautus on enintään 200 merkkiä.", en: "The note is at most 200 characters.", sv: "Anteckningen är högst 200 tecken." },
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

/** Formats an ISO timestamp or Date to HH:mm in Europe/Helsinki time. */
export function formatHelsinkiTime(iso: string | Date, lang: Lang): string {
  const date = typeof iso === "string" ? new Date(iso) : iso;
  if (Number.isNaN(date.getTime())) return "";
  return new Intl.DateTimeFormat(locale(lang), {
    hour: "2-digit",
    minute: "2-digit",
    hour12: false,
    timeZone: ZONE,
  }).format(date);
}

export const DAYS: readonly Word[] = ["mon", "tue", "wed", "thu", "fri", "sat", "sun"] as const;

/** A moment on Helsinki's clock, date and time, in the language's way. */
export function moment(lang: Lang, ms: number): string {
  return new Intl.DateTimeFormat(locale(lang), { timeZone: ZONE, dateStyle: "medium", timeStyle: "short" }).format(ms);
}
