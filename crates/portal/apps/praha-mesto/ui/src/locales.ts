/** The screen's words in Czech and English; the Portal's language picks one, Czech otherwise. */
import type { Pollutant } from "./praha";

export interface Strings {
  locale: string;
  title: string;
  /** The one page's name in the shell. */
  page: string;
  subtitle: string;
  loading: string;
  empty: string;
  noEndpoint: string;
  unavailable: string;
  bikes: {
    title: string;
    note: string;
    stations: string;
    working: string;
    bikes: string;
    docks: string;
    map: string;
    legend: string;
    histogram: (counts: string) => string;
    histogramAxis: string;
  };
  noMap: string;
  search: string;
  shown: (shown: number, of: number) => string;
  station: string;
  outOfService: string;
  parking: { title: string; note: string; capacity: string; free: string; occupied: string; noLive: string; chart: (parks: number) => string };
  carPark: string;
  air: { title: string; note: string; station: string; unit: string; chart: (stations: number) => string };
  pollutant: Record<Pollutant, string>;
  reported: string;
  source: string;
}

export const LOCALES: Record<"cs" | "en", Strings> = {
  cs: {
    locale: "cs-CZ",
    title: "Praha teď",
    page: "Teď",
    subtitle: "Sdílená kola, parkoviště P+R a kvalita ovzduší z otevřených dat města, jak jsou právě teď.",
    loading: "Načítám…",
    empty: "Prostor zatím žádné záznamy nemá.",
    noEndpoint: "Aplikace nemá koncový bod pro prostor praha-mesto.",
    unavailable: "Data se nepodařilo načíst",
    bikes: {
      title: "Sdílená kola nextbike",
      note: "Stanice a jejich stav, jak je nextbike hlásí každých pět minut.",
      stations: "stanic",
      working: "v provozu",
      bikes: "kol k vypůjčení",
      docks: "volných stojanů",
      map: "Mapa stanic nextbike, barva podle počtu kol",
      legend: "Kol na stanici",
      histogram: (counts) => `Stanice v provozu podle počtu kol: ${counts}`,
      histogramAxis: "stanic",
    },
    noMap: "Mapu tento prohlížeč nevykreslí; stanice jsou v seznamu níže.",
    search: "Hledat stanici",
    shown: (shown, of) => `Zobrazeno ${shown} z ${of} stanic`,
    station: "Stanice",
    outOfService: "mimo provoz",
    parking: {
      title: "Parkoviště P+R",
      note: "Kapacita podle IPR Praha; volná a obsazená místa podle počítadel TSK, kde je parkoviště má.",
      capacity: "Kapacita",
      free: "Volno",
      occupied: "Obsazeno",
      noLive: "bez počítadla",
      chart: (parks) => `Obsazenost ${parks} parkovišť P+R s počítadlem, jinak jejich kapacita`,
    },
    carPark: "Parkoviště",
    air: {
      title: "Kvalita ovzduší",
      note: "Hodinové průměry stanic ČHMÚ v Praze.",
      station: "Stanice",
      unit: "µg/m³",
      chart: (stations) => `Hodinové koncentrace na ${stations} stanicích, µg/m³`,
    },
    pollutant: { pm10: "PM10", pm25: "PM2,5", no2: "NO₂", o3: "O₃" },
    reported: "Stav z",
    source: "Zdroje: nextbike (CC0), IPR Praha (datový podklad © IPR Praha, CC BY 4.0), TSK hl. m. Prahy přes Golemio, ČHMÚ.",
  },
  en: {
    locale: "en-GB",
    title: "Prague right now",
    page: "Now",
    subtitle: "Shared bikes, park-and-ride car parks and air quality from the city's open data, as they are right now.",
    loading: "Loading…",
    empty: "The space holds no records yet.",
    noEndpoint: "The app has no endpoint for the praha-mesto space.",
    unavailable: "The data could not be loaded",
    bikes: {
      title: "nextbike shared bikes",
      note: "Stations and their state, as nextbike reports them every five minutes.",
      stations: "stations",
      working: "in service",
      bikes: "bikes to rent",
      docks: "free docks",
      map: "Map of the nextbike stations, coloured by the bikes each holds",
      legend: "Bikes at the station",
      histogram: (counts) => `Stations in service by the bikes they hold: ${counts}`,
      histogramAxis: "stations",
    },
    noMap: "This browser cannot draw the map; the stations are in the list below.",
    search: "Find a station",
    shown: (shown, of) => `Showing ${shown} of ${of} stations`,
    station: "Station",
    outOfService: "out of service",
    parking: {
      title: "Park and ride",
      note: "Capacity from IPR Praha; free and occupied spaces from TSK's counters, where the car park has them.",
      capacity: "Capacity",
      free: "Free",
      occupied: "Occupied",
      noLive: "no counter",
      chart: (parks) => `Occupancy of ${parks} park-and-ride car parks with a counter, capacity otherwise`,
    },
    carPark: "Car park",
    air: {
      title: "Air quality",
      note: "Hourly averages of ČHMÚ's stations in Prague.",
      station: "Station",
      unit: "µg/m³",
      chart: (stations) => `Hourly concentrations at ${stations} stations, µg/m³`,
    },
    pollutant: { pm10: "PM10", pm25: "PM2.5", no2: "NO₂", o3: "O₃" },
    reported: "As of",
    source: "Sources: nextbike (CC0), IPR Praha (datový podklad © IPR Praha, CC BY 4.0), TSK Praha through Golemio, ČHMÚ.",
  },
};

export function stringsFor(language: string | undefined): Strings {
  return language?.toLowerCase().startsWith("en") ? LOCALES.en : LOCALES.cs;
}
