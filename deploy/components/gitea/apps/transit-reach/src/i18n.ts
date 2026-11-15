/**
 * The App's words in Finnish, English and Swedish, and Helsinki's way of writing numbers and times.
 * The language comes from the address (`?lang=fi`), else the reader's browser, else English.
 */

export type Lang = "fi" | "en" | "sv";
export const LANGS: readonly Lang[] = ["fi", "en", "sv"];
export const ZONE = "Europe/Helsinki";

const WORDS = {
  title: { fi: "Kuinka pitkälle pääsen joukkoliikenteellä", en: "How far can I get by transit", sv: "Hur långt kommer jag med kollektivtrafik" },
  page: { fi: "Saavutettavuus", en: "Reach", sv: "Räckvidd" },
  loading: { fi: "Luetaan ajoneuvojen kulkua…", en: "Reading where the vehicles went…", sv: "Läser var fordonen körde…" },
  analysing: { fi: "Lasketaan…", en: "Working it out…", sv: "Räknar…" },
  historyUnreadable: {
    fi: "Ajoneuvojen historiaa ei voitu lukea (HTTP {status}): näkyvissä on vain kävelymatka.",
    en: "The vehicles' history could not be read (HTTP {status}): only walking is shown.",
    sv: "Fordonens historik kunde inte läsas (HTTP {status}): bara gångavståndet visas.",
  },
  analysisFailed: { fi: "Laskenta epäonnistui: {reason}", en: "The analysis failed: {reason}", sv: "Beräkningen misslyckades: {reason}" },
  retry: { fi: "Yritä uudelleen", en: "Retry", sv: "Försök igen" },
  kept: { fi: "Pysäkiltä palvelimella", en: "From a stop, on the server", sv: "Från en hållplats, på servern" },
  keep: { fi: "Lataa alueet lähtöpysäkiltä (GeoJSON)", en: "Download the areas from the start stop (GeoJSON)", sv: "Ladda ner områdena från starthållplatsen (GeoJSON)" },
  keepFrom: { fi: "Lähtöpysäkki: {stop}.", en: "Start stop: {stop}.", sv: "Starthållplats: {stop}." },
  keepHint: {
    fi: "Aloita pysäkiltä (luettelosta tai valikosta), niin sen alueet lasketaan palvelimella HSL:n verkosta ja säilytetään.",
    en: "Start from a stop (from the list or the menu) and its areas are computed on the server from HSL's network and kept.",
    sv: "Börja från en hållplats (i listan eller menyn) så räknas dess områden ut på servern ur HRT:s nät och sparas.",
  },
  keeping: { fi: "Lasketaan palvelimella…", en: "Working it out on the server…", sv: "Räknar på servern…" },
  keptDownloading: { fi: "GeoJSON-tiedosto ladataan.", en: "The GeoJSON file is downloading.", sv: "GeoJSON-filen laddas ner." },
  keptAreas: { fi: "Palvelimella säilytetyt alueet pysäkiltä {stop}: {areas}.", en: "Areas from {stop} kept on the server: {areas}.", sv: "Områden från {stop} sparade på servern: {areas}." },
  keptBand: { fi: "{minutes} min {km2} km²", en: "{minutes} min {km2} km²", sv: "{minutes} min {km2} km²" },
  keptFailed: { fi: "Alueita ei saatu: {why}", en: "No areas: {why}", sv: "Inga områden: {why}" },
  noSuchStop: { fi: "HSL:n verkossa ei ole tätä pysäkkiä sijainteineen.", en: "HSL's network holds no such stop with a place.", sv: "HRT:s nät har ingen sådan hållplats med en plats." },
  noNetwork: {
    fi: "Tilassa ei ole HSL:n pysäkkejä ja linjoja; sivu käyttää ajoneuvojen historiaa.",
    en: "The space holds no HSL stops and lines; the page uses the vehicles' history instead.",
    sv: "Utrymmet har inga hållplatser och linjer från HRT; sidan använder fordonens historik i stället.",
  },
  serverOffline: { fi: "Palvelimeen ei saatu yhteyttä. Tarkista yhteys ja yritä uudelleen.", en: "The server could not be reached. Check the connection and try again.", sv: "Servern kunde inte nås. Kontrollera anslutningen och försök igen." },
  serverDown: { fi: "Palvelin ei vastannut. Yritä hetken päästä uudelleen.", en: "The server did not answer. Try again in a moment.", sv: "Servern svarade inte. Försök igen om en stund." },
  signInAgain: { fi: "Kirjaudu uudelleen sisään ja yritä uudelleen.", en: "Sign in again, then try again.", sv: "Logga in igen och försök sedan igen." },
  noRight: { fi: "Tunnuksellasi ei ole oikeutta tähän.", en: "Your account may not do this.", sv: "Det här kontot får inte göra detta." },
  storageFull: { fi: "Sovelluksen tallennustila on täynnä. Yritä huomenna uudelleen.", en: "The app's storage is full. Try again tomorrow.", sv: "Appens lagring är full. Försök igen i morgon." },
  summary: {
    fi: "Saavutettava alue tästä pisteestä: {bands}. Pysäkkejä {limit} minuutissa: {reached}/{all}.",
    en: "Reachable from this point: {bands}. Stops within {limit} minutes: {reached} of {all}.",
    sv: "Nåbart från denna punkt: {bands}. Hållplatser inom {limit} minuter: {reached} av {all}.",
  },
  bandPhrase: { fi: "{minutes} min {area} km²", en: "{minutes} min {area} km²", sv: "{minutes} min {area} km²" },
  walkOnly: { fi: "Ajoneuvoista ei ole historiaa: alue on pelkkä kävelymatka.", en: "No vehicle history: the area is walking distance only.", sv: "Ingen fordonshistorik: området är bara gångavstånd." },
  derived: {
    fi: "Pysäkit on päätelty paikoista, joissa {vehicles} ajoneuvoa seisoi {from}–{to}. Rajapinnassa ei ole pysäkki- eikä aikataulutietoja, joten myös liikennevalot voivat näkyä pysäkkeinä. Odotus {wait} min jokaisella nousulla, kävely 4,5 km/h.",
    en: "Stops are derived from where {vehicles} vehicles stood still from {from} to {to}. The endpoint carries no stops or timetables, so a traffic light can show as a stop. A {wait}-minute wait at each boarding, walking at 4.5 km/h.",
    sv: "Hållplatserna är härledda från var {vehicles} fordon stod stilla från {from} till {to}. Gränssnittet har inga hållplatser eller tidtabeller, så ett trafikljus kan visas som en hållplats. {wait} minuters väntan vid varje påstigning, gång i 4,5 km/h.",
  },
  network: {
    fi: "Pysäkit ja linjat HSL:n rekistereistä ({stops} pysäkkiä, {lines} linjaversiota). Matka kestää linjan matkan kulkumuodon tavallisella nopeudella (metro 40 km/h, juna 50, bussi 20, ratikka 15, lautta 18), odotus {wait} min jokaisella nousulla, kävely 4,5 km/h. Aikatauluja ei ole mukana.",
    en: "Stops and lines from HSL's registers ({stops} stops, {lines} line variants). A ride takes the line's distance at the mode's usual speed (metro 40 km/h, train 50, bus 20, tram 15, ferry 18), a {wait}-minute wait at each boarding, walking at 4.5 km/h. No timetables are read.",
    sv: "Hållplatser och linjer ur HRT:s register ({stops} hållplatser, {lines} linjevarianter). En resa tar linjens sträcka i trafikslagets vanliga fart (metro 40 km/h, tåg 50, buss 20, spårvagn 15, färja 18), {wait} minuters väntan vid varje påstigning, gång i 4,5 km/h. Inga tidtabeller läses.",
  },
  networkUnreadable: {
    fi: "HSL:n pysäkkejä ja linjoja ei voitu lukea (HTTP {status}): pysäkit päätellään ajoneuvoista.",
    en: "HSL's stops and lines could not be read (HTTP {status}): the stops are derived from the vehicles.",
    sv: "HRT:s hållplatser och linjer kunde inte läsas (HTTP {status}): hållplatserna härleds ur fordonen.",
  },
  stopNamed: { fi: "{name}: linjat {routes}", en: "{name}: lines {routes}", sv: "{name}: linjer {routes}" },
  more: { fi: "ja {n} muuta kartalla", en: "and {n} more on the map", sv: "och {n} till på kartan" },
  controls: { fi: "Lähtöpiste ja historia", en: "Starting point and history", sv: "Startpunkt och historik" },
  hours: { fi: "Ajoneuvojen historia", en: "Vehicle history", sv: "Fordonshistorik" },
  hoursOption: { fi: "viimeiset {n} h", en: "last {n} h", sv: "senaste {n} h" },
  startStop: { fi: "Lähde pysäkiltä", en: "Start from a stop", sv: "Börja från en hållplats" },
  pickedPoint: { fi: "Kartalta valittu piste", en: "The point picked on the map", sv: "Punkten vald på kartan" },
  stopOption: { fi: "Pysäkki {n}: linjat {routes}", en: "Stop {n}: routes {routes}", sv: "Hållplats {n}: linjer {routes}" },
  noRoutes: { fi: "ei linjaa", en: "no route", sv: "ingen linje" },
  centre: { fi: "Takaisin Rautatientorille", en: "Back to Rautatientori", sv: "Tillbaka till Järnvägstorget" },
  map: { fi: "Kartta: saavutettava alue. Napsauta karttaa aloittaaksesi sieltä.", en: "Map: the area reached. Click the map to start from there.", sv: "Karta: det nåbara området. Klicka på kartan för att börja därifrån." },
  bandLegend: { fi: "enintään {minutes} min", en: "up to {minutes} min", sv: "högst {minutes} min" },
  legend: { fi: "Selite", en: "Legend", sv: "Teckenförklaring" },
  start: { fi: "Lähtöpiste", en: "Starting point", sv: "Startpunkt" },
  stopLegend: { fi: "Päätelty pysäkki", en: "Derived stop", sv: "Härledd hållplats" },
  stopLegendNetwork: { fi: "Pysäkki", en: "Stop", sv: "Hållplats" },
  cellLine: { fi: "{minutes} min lähtöpisteestä", en: "{minutes} min from the start", sv: "{minutes} min från starten" },
  stopLine: { fi: "Linjat {routes}", en: "Routes {routes}", sv: "Linjer {routes}" },
  stopReached: { fi: "{minutes} min lähtöpisteestä", en: "{minutes} min from the start", sv: "{minutes} min från starten" },
  stopFar: { fi: "Ei {limit} minuutissa", en: "Not within {limit} minutes", sv: "Inte inom {limit} minuter" },
  reached: { fi: "Pysäkit {limit} minuutissa", en: "Stops within {limit} minutes", sv: "Hållplatser inom {limit} minuter" },
  reachedLine: { fi: "{minutes} min: linjat {routes}", en: "{minutes} min: routes {routes}", sv: "{minutes} min: linjer {routes}" },
  startFrom: { fi: "{stop}: lähde tästä", en: "{stop}: start from here", sv: "{stop}: börja härifrån" },
  details: { fi: "Tiedot", en: "Details", sv: "Uppgifter" },
  detailsOf: { fi: "{stop}: tiedot", en: "{stop}: details", sv: "{stop}: uppgifter" },
  noReached: { fi: "Ei pysäkkejä {limit} minuutin sisällä.", en: "No stop within {limit} minutes.", sv: "Ingen hållplats inom {limit} minuter." },
  // Roles, origins and the history (T-3826).
  roleSteward: { fi: "Ylläpitäjä", en: "Steward", sv: "Förvaltare" },
  roleViewer: { fi: "Katselija", en: "Viewer", sv: "Läsare" },
  signedOut: { fi: "Et ole kirjautunut", en: "Not signed in", sv: "Inte inloggad" },
  roleLabel: { fi: "Roolisi", en: "Your role", sv: "Din roll" },
  gone: { fi: "Sitä ei enää ole, tai se ei ole sinun muutettavissasi.", en: "It is no longer there, or it is not yours to change.", sv: "Den finns inte längre, eller så är den inte din att ändra." },
  saveTitle: { fi: "Säilytä lähtöpiste", en: "Keep this origin", sv: "Spara startpunkten" },
  saveName: { fi: "Nimi", en: "Name", sv: "Namn" },
  saveHint: { fi: "Säilytä tämä piste alueineen ja vertaa sitä muihin Lähtöpisteet-sivulla.", en: "Keep this point with its areas and compare it with others on the Origins page.", sv: "Spara denna punkt med dess områden och jämför den med andra på sidan Startpunkter." },
  save: { fi: "Säilytä", en: "Keep", sv: "Spara" },
  saving: { fi: "Säilytetään…", en: "Keeping…", sv: "Sparar…" },
  saveSignIn: { fi: "Kirjaudu sisään säilyttääksesi lähtöpisteitä.", en: "Sign in to keep origins.", sv: "Logga in för att spara startpunkter." },
  saveWait: { fi: "Odota, kunnes alue on laskettu.", en: "Wait until the area is worked out.", sv: "Vänta tills området är uträknat." },
  saved: { fi: "Säilytetty: {name}. Vertaa sitä Lähtöpisteet-sivulla.", en: "Kept: {name}. Compare it on the Origins page.", sv: "Sparad: {name}. Jämför den på sidan Startpunkter." },
  saveFailed: { fi: "Ei säilytetty", en: "Not kept", sv: "Inte sparad" },
  pointName: { fi: "Piste {lat}, {lon}", en: "Point {lat}, {lon}", sv: "Punkt {lat}, {lon}" },
  originsPage: { fi: "Lähtöpisteet", en: "Origins", sv: "Startpunkter" },
  originsTitle: { fi: "Lähtöpisteet rinnakkain", en: "Origins side by side", sv: "Startpunkter sida vid sida" },
  originsLead: { fi: "Omat lähtöpisteesi ja julkaistut, ylläpitäjän valitsemat. Ala min -sarakkeet: saavutettava pinta-ala.", en: "Your own origins and the published ones a steward chose. The minute columns: the area reached.", sv: "Dina egna startpunkter och de publicerade som en förvaltare valt. Minutkolumnerna: den nåbara ytan." },
  originsLeadSteward: { fi: "Kaikkien lähtöpisteet; julkaistut näkyvät jokaiselle.", en: "Everyone's origins; the published ones every visitor sees.", sv: "Allas startpunkter; de publicerade ser varje besökare." },
  originsEmpty: { fi: "Lähtöpisteitä ei ole. Säilytä yksi Saavutettavuus-sivulla.", en: "No origin yet. Keep one on the Reach page.", sv: "Inga startpunkter ännu. Spara en på sidan Räckvidd." },
  originsLoading: { fi: "Luetaan lähtöpisteitä…", en: "Reading the origins…", sv: "Läser startpunkterna…" },
  originsFailed: { fi: "Lähtöpisteitä ei voitu lukea", en: "The origins could not be read", sv: "Startpunkterna kunde inte läsas" },
  colName: { fi: "Lähtöpiste", en: "Origin", sv: "Startpunkt" },
  colMinutes: { fi: "{minutes} min", en: "{minutes} min", sv: "{minutes} min" },
  colStops: { fi: "Pysäkit 30 min", en: "Stops in 30 min", sv: "Hållplatser 30 min" },
  colState: { fi: "Tila", en: "State", sv: "Status" },
  colActions: { fi: "Toiminnot", en: "Actions", sv: "Åtgärder" },
  area: { fi: "{km2} km²", en: "{km2} km²", sv: "{km2} km²" },
  stateMine: { fi: "oma", en: "yours", sv: "din" },
  stateOther: { fi: "toisen", en: "someone else's", sv: "någon annans" },
  statePublished: { fi: "julkaistu", en: "published", sv: "publicerad" },
  open: { fi: "Avaa", en: "Open", sv: "Öppna" },
  rename: { fi: "Nimeä uudelleen", en: "Rename", sv: "Byt namn" },
  renameSave: { fi: "Tallenna nimi", en: "Save the name", sv: "Spara namnet" },
  renamed: { fi: "Nimi tallennettu: {name}.", en: "The name is saved: {name}.", sv: "Namnet är sparat: {name}." },
  renameFailed: { fi: "Nimeä ei tallennettu", en: "The name was not saved", sv: "Namnet sparades inte" },
  remove: { fi: "Poista", en: "Delete", sv: "Ta bort" },
  confirmRemove: { fi: "Poista lopullisesti", en: "Delete for good", sv: "Ta bort för gott" },
  cancel: { fi: "Peruuta", en: "Cancel", sv: "Avbryt" },
  removed: { fi: "Poistettu: {name}.", en: "Deleted: {name}.", sv: "Borttagen: {name}." },
  removeFailed: { fi: "Ei poistettu", en: "Not deleted", sv: "Inte borttagen" },
  publish: { fi: "Julkaise", en: "Publish", sv: "Publicera" },
  withdraw: { fi: "Peru julkaisu", en: "Withdraw", sv: "Dra tillbaka" },
  publishOnly: { fi: "Vain ylläpitäjä julkaisee lähtöpisteitä kaikille.", en: "Only a steward publishes origins for everyone.", sv: "Bara en förvaltare publicerar startpunkter för alla." },
  published: { fi: "Julkaistu kaikille: {name}.", en: "Published for everyone: {name}.", sv: "Publicerad för alla: {name}." },
  withdrawn: { fi: "Julkaisu peruttu: {name}.", en: "Withdrawn: {name}.", sv: "Tillbakadragen: {name}." },
  publishFailed: { fi: "Julkaisua ei muutettu", en: "The publishing was not changed", sv: "Publiceringen ändrades inte" },
  notYours: { fi: "Vain sen säilyttäjä tai ylläpitäjä muuttaa tätä.", en: "Only whoever kept it, or a steward, changes this.", sv: "Bara den som sparade den, eller en förvaltare, ändrar detta." },
  historyPage: { fi: "Historia", en: "History", sv: "Historik" },
  historyTitle: { fi: "Muutokset", en: "Changes", sv: "Ändringar" },
  historyMine: { fi: "Omat muutoksesi; ylläpitäjä näkee kaikkien.", en: "Your own changes; a steward sees everyone's.", sv: "Dina egna ändringar; en förvaltare ser allas." },
  historyAll: { fi: "Kaikkien muutokset, uusin ensin.", en: "Everyone's changes, newest first.", sv: "Allas ändringar, nyaste först." },
  historyEmpty: { fi: "Muutoksia ei ole vielä.", en: "No change yet.", sv: "Inga ändringar ännu." },
  historyLoading: { fi: "Luetaan historiaa…", en: "Reading the history…", sv: "Läser historiken…" },
  historyFailed: { fi: "Historiaa ei voitu lukea", en: "The history could not be read", sv: "Historiken kunde inte läsas" },
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

/** The language of the page: `?lang=` when it names one, else the browser's first of ours, else English. */
export function langOf(search: string, browser: readonly string[]): Lang {
  const asked = new URLSearchParams(search).get("lang");
  if (LANGS.includes(asked as Lang)) return asked as Lang;
  for (const tag of browser) {
    const lang = tag.slice(0, 2).toLowerCase();
    if (LANGS.includes(lang as Lang)) return lang as Lang;
  }
  return "en";
}

/** Swedish as Finland writes it; English as Britain does. */
export const locale = (lang: Lang) => (lang === "en" ? "en-GB" : `${lang}-FI`);

/** A whole number in the language's way: 1 234 in Finnish and Swedish, 1,234 in English. */
export function number(lang: Lang, value: number): string {
  return new Intl.NumberFormat(locale(lang), { maximumFractionDigits: 0 }).format(value);
}

/** A number with one decimal: 2,4 in Finnish, 2.4 in English. */
export function decimal(lang: Lang, value: number): string {
  return new Intl.NumberFormat(locale(lang), { maximumFractionDigits: 1, minimumFractionDigits: 1 }).format(value);
}

/** A moment on Helsinki's clock: 8.10.2026 klo 10.00 in Finnish, 8 Oct 2026, 10:00 in English. */
export function moment(lang: Lang, ms: number): string {
  return new Intl.DateTimeFormat(locale(lang), {
    timeZone: ZONE,
    day: "numeric",
    month: lang === "en" ? "short" : "numeric",
    year: "numeric",
    hour: "2-digit",
    minute: "2-digit",
    hourCycle: "h23",
  }).format(ms);
}
