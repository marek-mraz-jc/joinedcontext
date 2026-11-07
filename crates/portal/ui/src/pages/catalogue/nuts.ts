/**
 * NUTS regions by their official names (T-4264, T-4160 #13): the catalogue's area facet and a
 * dataset's area say "Banskobystrický kraj", not `SK032` or its EU vocabulary URI. NUTS names are
 * the national ones in every language, as Eurostat publishes them, so the table has one name per
 * code. It holds the countries the platform's organizations sit in; any other code shows as the
 * code. Upgrade: read Eurostat's NUTS table when a tenant outside these countries publishes.
 */
const NUTS: Record<string, string> = {
  SK0: "Slovensko",
  SK01: "Bratislavský kraj",
  SK010: "Bratislavský kraj",
  SK02: "Západné Slovensko",
  SK021: "Trnavský kraj",
  SK022: "Trenčiansky kraj",
  SK023: "Nitriansky kraj",
  SK03: "Stredné Slovensko",
  SK031: "Žilinský kraj",
  SK032: "Banskobystrický kraj",
  SK04: "Východné Slovensko",
  SK041: "Prešovský kraj",
  SK042: "Košický kraj",
  CZ0: "Česko",
  CZ01: "Praha",
  CZ010: "Hlavní město Praha",
  CZ02: "Střední Čechy",
  CZ020: "Středočeský kraj",
  CZ03: "Jihozápad",
  CZ031: "Jihočeský kraj",
  CZ032: "Plzeňský kraj",
  CZ04: "Severozápad",
  CZ041: "Karlovarský kraj",
  CZ042: "Ústecký kraj",
  CZ05: "Severovýchod",
  CZ051: "Liberecký kraj",
  CZ052: "Královéhradecký kraj",
  CZ053: "Pardubický kraj",
  CZ06: "Jihovýchod",
  CZ063: "Kraj Vysočina",
  CZ064: "Jihomoravský kraj",
  CZ07: "Střední Morava",
  CZ071: "Olomoucký kraj",
  CZ072: "Zlínský kraj",
  CZ08: "Moravskoslezsko",
  CZ080: "Moravskoslezský kraj",
  FI1: "Manner-Suomi",
  FI1B: "Helsinki-Uusimaa",
  FI1B1: "Helsinki-Uusimaa",
};

/** The NUTS code of a code or of its URI (`http://data.europa.eu/nuts/code/SK032`), upper case. */
export function nutsCode(value: string): string {
  return (value.split("/").pop() ?? value).trim().toUpperCase();
}

/** The region's name, or the value as it came when it names no region this table holds. */
export function areaLabel(value: string): string {
  return NUTS[nutsCode(value)] ?? value;
}

/** Every region the table names, for the area picker: codes in the table's order. */
export const NUTS_CODES: readonly string[] = Object.keys(NUTS);
