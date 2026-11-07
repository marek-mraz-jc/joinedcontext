/**
 * Every word on screen, Slovak first (T-2308).
 *
 * Slovak is the language of both publishers, so it is the default and English is what a reader
 * gets by asking; `config.language` decides, and no component holds a string. The two maps have
 * the same keys, which `locales.test.ts` asserts, because a missing Slovak key would silently
 * show English on a Slovak page.
 */
export interface Strings {
  /** The BCP 47 tag `Intl` formats numbers and dates with. */
  locale: string;
  title: string;
  subtitle: string;
  /** The heading over each body's cards, and the word that says whose number a card carries. */
  body: Record<"bbsk" | "banskabystrica", string>;
  bodyNote: Record<"bbsk" | "banskabystrica", string>;
  territory: Record<string, string>;
  indicator: Record<string, { title: string; unit: string }>;
  state: Record<"green" | "amber" | "red", string>;
  stateOf: string;
  notMeasured: string;
  notMeasuredWhy: string;
  window: string;
  computedAt: string;
  formula: string;
  loading: string;
  unavailable: string;
  unavailableWhy: string;
  empty: string;
  noEndpoint: string;
  rawUnit: string;
  /** The disclosure that holds a card's window, computation time and formula. */
  details: string;
  /** The caption over the bar chart of one indicator's districts. */
  districtsCompared: string;
  /** The heading of the district map, the picker of the indicator it colours, and its notes. */
  map: string;
  mapIndicator: string;
  mapHint: string;
  mapUnavailable: string;
  /** The years a card's trend line covers and its first and last value, for a screen reader. */
  trend: (from: string, to: string, first: string, last: string) => string;
  trendUnavailable: string;
}

const TERRITORY_SK: Record<string, string> = {
  kraj: "Banskobystrický kraj",
  mesto: "Mesto Banská Bystrica",
  "okres-banska-bystrica": "okres Banská Bystrica",
  "okres-banska-stiavnica": "okres Banská Štiavnica",
  "okres-brezno": "okres Brezno",
  "okres-detva": "okres Detva",
  "okres-krupina": "okres Krupina",
  "okres-lucenec": "okres Lučenec",
  "okres-poltar": "okres Poltár",
  "okres-revuca": "okres Revúca",
  "okres-rimavska-sobota": "okres Rimavská Sobota",
  "okres-velky-krtis": "okres Veľký Krtíš",
  "okres-zvolen": "okres Zvolen",
  "okres-zarnovica": "okres Žarnovica",
  "okres-ziar-nad-hronom": "okres Žiar nad Hronom",
};

const TERRITORY_EN: Record<string, string> = {
  kraj: "Banská Bystrica Region",
  mesto: "City of Banská Bystrica",
  "okres-banska-bystrica": "Banská Bystrica District",
  "okres-banska-stiavnica": "Banská Štiavnica District",
  "okres-brezno": "Brezno District",
  "okres-detva": "Detva District",
  "okres-krupina": "Krupina District",
  "okres-lucenec": "Lučenec District",
  "okres-poltar": "Poltár District",
  "okres-revuca": "Revúca District",
  "okres-rimavska-sobota": "Rimavská Sobota District",
  "okres-velky-krtis": "Veľký Krtíš District",
  "okres-zvolen": "Zvolen District",
  "okres-zarnovica": "Žarnovica District",
  "okres-ziar-nad-hronom": "Žiar nad Hronom District",
};

const SK: Strings = {
  locale: "sk-SK",
  title: "Ukazovatele kraja a mesta",
  subtitle:
    "Banskobystrický samosprávny kraj a mesto Banská Bystrica, každý so svojimi vlastnými číslami.",
  body: {
    bbsk: "Banskobystrický samosprávny kraj",
    banskabystrica: "Mesto Banská Bystrica",
  },
  bodyNote: {
    bbsk: "Vydavateľ: Banskobystrický samosprávny kraj. Čísla sa týkajú celého kraja alebo jedného okresu, nie mesta.",
    banskabystrica:
      "Vydavateľ: Mesto Banská Bystrica. Čísla sa týkajú mesta, nie kraja. Kraj má približne osemkrát viac obyvateľov.",
  },
  territory: TERRITORY_SK,
  indicator: {
    "pm10-24h": { title: "Priemerná 24-hodinová koncentrácia PM10", unit: "µg/m³" },
    "pm25-rok": { title: "Priemerná ročná koncentrácia PM2,5", unit: "µg/m³" },
    "spotreba-vody-obyvatel": { title: "Spotreba vody na obyvateľa", unit: "l/os./deň" },
    "emisie-tuhe-km2": { title: "Emisie tuhých znečisťujúcich látok na km²", unit: "t/km²" },
    "obyvatelstvo-stav": { title: "Stav obyvateľstva", unit: "osôb" },
  },
  state: { green: "v norme", amber: "zvýšené", red: "nad limitom" },
  stateOf: "Stav podľa smernice 2008/50/ES",
  notMeasured: "nemerané",
  notMeasuredWhy: "Za toto obdobie neprišlo žiadne meranie. Nula by znamenala nameranú nulu.",
  window: "Obdobie",
  computedAt: "Vypočítané",
  formula: "Výpočet",
  loading: "Načítavajú sa ukazovatele…",
  unavailable: "Ukazovatele tohto vydavateľa sa nepodarilo načítať.",
  unavailableWhy: "Dôvod",
  empty: "Tento vydavateľ zatiaľ nezverejnil žiadny ukazovateľ.",
  noEndpoint: "Táto aplikácia nemá prístup k údajom tohto vydavateľa.",
  rawUnit: "Jednotka zo záznamu",
  details: "Obdobie a výpočet",
  districtsCompared: "Okresy v porovnaní",
  map: "Mapa okresov",
  mapIndicator: "Ukazovateľ na mape",
  mapHint: "Okres vyberiete kliknutím na mape alebo na jeho názov v grafe.",
  mapUnavailable: "Hranice okresov sa nepodarilo načítať",
  trend: (from, to, first, last) => `Vývoj ${from} – ${to}: od ${first} po ${last}`,
  trendUnavailable: "Ročné rady údajov kraja sa nepodarilo načítať, karty ukazujú posledné obdobie",
};

const EN: Strings = {
  locale: "en-GB",
  title: "Indicators of the region and the city",
  subtitle:
    "The Banská Bystrica self-governing region and the city of Banská Bystrica, each with its own numbers.",
  body: {
    bbsk: "Banská Bystrica self-governing region",
    banskabystrica: "City of Banská Bystrica",
  },
  bodyNote: {
    bbsk: "Published by the Banská Bystrica self-governing region. These numbers cover the whole region or one district, not the city.",
    banskabystrica:
      "Published by the city of Banská Bystrica. These numbers cover the city, not the region. The region has roughly eight times as many inhabitants.",
  },
  territory: TERRITORY_EN,
  indicator: {
    "pm10-24h": { title: "Mean 24-hour PM10 concentration", unit: "µg/m³" },
    "pm25-rok": { title: "Mean calendar-year PM2.5 concentration", unit: "µg/m³" },
    "spotreba-vody-obyvatel": { title: "Water use per inhabitant", unit: "l/person/day" },
    "emisie-tuhe-km2": { title: "Solid pollutant emissions per km²", unit: "t/km²" },
    "obyvatelstvo-stav": { title: "Population", unit: "persons" },
  },
  state: { green: "within the limit", amber: "elevated", red: "above the limit" },
  stateOf: "State against Directive 2008/50/EC",
  notMeasured: "not measured",
  notMeasuredWhy: "No reading arrived for this window. A zero would mean a measured zero.",
  window: "Window",
  computedAt: "Computed",
  formula: "Calculation",
  loading: "Loading the indicators…",
  unavailable: "This publisher's indicators could not be loaded.",
  unavailableWhy: "Reason",
  empty: "This publisher has not published an indicator yet.",
  noEndpoint: "This application has no access to this publisher's data.",
  rawUnit: "Unit as recorded",
  details: "Window and computation",
  districtsCompared: "Districts compared",
  map: "District map",
  mapIndicator: "Indicator on the map",
  mapHint: "Pick a district by clicking it on the map or its name in the chart.",
  mapUnavailable: "The district outlines could not be read",
  trend: (from, to, first, last) => `Trend ${from} – ${to}: from ${first} to ${last}`,
  trendUnavailable: "The region's yearly rows could not be read; the cards show the latest period",
};

export const LOCALES: Readonly<Record<string, Strings>> = { sk: SK, en: EN };

/** Slovak unless the served configuration asks for a language this application has. */
export function stringsFor(language?: string): Strings {
  const tag = (language ?? "sk").toLowerCase().split("-")[0];
  return LOCALES[tag] ?? SK;
}
