/**
 * What the public endpoint of `bbsk-podujatia` answers, as the pipelines of T-4896 write the Event
 * model of T-4895: names and dates as Property, the address as a PostalAddress, the GPS point as a
 * GeoProperty. Fourteen towns, as the dashboard has to show on dev; a few rows carry what a source
 * slips in (no end, a date without a time, a `javascript:` link, no position, no start).
 */
const URN = (id: string) => `urn:ngsi-ld:Event:bbsk-podujatia:${id}`;
const value = (v: unknown) => ({ type: "Property", value: v });
const point = (lon: number, lat: number) => ({ type: "GeoProperty", value: { type: "Point", coordinates: [lon, lat] } });

interface Seed {
  id: string;
  name: string;
  start: string;
  end?: string;
  town: string;
  venue?: string;
  category?: string;
  url?: string;
  at?: [number, number];
}

const SEEDS: Seed[] = [
  { id: "a1", name: "Jesenný jarmok", start: "2026-10-24T09:00:00+02:00", end: "2026-10-24T18:00:00+02:00", town: "Banská Bystrica", venue: "Námestie SNP", category: "Jarmok", url: "https://www.banskabystrica.sk/podujatia/jesenny-jarmok/", at: [19.1462, 48.7363] },
  { id: "a2", name: "Koncert Štátnej opery", start: "2026-10-27T19:00:00+01:00", end: "2026-10-27T21:30:00+01:00", town: "Banská Bystrica", venue: "Štátna opera", category: "Hudba", url: "https://www.stateopera.sk/", at: [19.1503, 48.7372] },
  { id: "b1", name: "Zvolenské divadelné dni", start: "2026-10-29", end: "2026-10-31", town: "Zvolen", venue: "Divadlo J. G. Tajovského", category: "Divadlo", url: "https://www.zvolen.sk/podujatia/divadelne-dni", at: [19.1294, 48.5763] },
  { id: "b2", name: "Beseda o histórii zámku", start: "2026-11-05T17:00:00+01:00", town: "Zvolen", venue: "Zvolenský zámok", category: "Prednáška", url: "https://www.zvolen.sk/podujatia/beseda", at: [19.1263, 48.5767] },
  { id: "c1", name: "Horehronské dni kultúry", start: "2026-11-02T10:00:00+01:00", end: "2026-11-02T20:00:00+01:00", town: "Brezno", venue: "Námestie M. R. Štefánika", category: "Folklór", url: "https://www.brezno.sk/podujatia/horehronske-dni", at: [19.6447, 48.8045] },
  { id: "d1", name: "Novohradský maratón", start: "2026-11-08T09:30:00+01:00", town: "Lučenec", category: "Šport", url: "https://www.lucenec.sk/maraton", at: [19.6683, 48.3301] },
  { id: "e1", name: "Gemerský jarmok remesiel", start: "2026-11-14T09:00:00+01:00", end: "2026-11-15T17:00:00+01:00", town: "Rimavská Sobota", venue: "Hlavné námestie", category: "Jarmok", url: "https://www.rimavskasobota.sk/jarmok", at: [20.0221, 48.3818] },
  { id: "f1", name: "Salamandrové dni – nočná prehliadka", start: "2026-11-01T18:00:00+01:00", end: "2026-11-01T21:00:00+01:00", town: "Banská Štiavnica", venue: "Starý zámok", category: "Prehliadka", url: "https://www.banskastiavnica.travel/events/nocna-prehliadka/", at: [18.8946, 48.4588] },
  { id: "g1", name: "Výstava fotografií Pohronie", start: "2026-10-22T10:00:00+02:00", end: "2026-11-30T17:00:00+01:00", town: "Žiar nad Hronom", venue: "Mestské kultúrne centrum", category: "Výstava", url: "https://www.ziar.sk/vystava", at: [18.8541, 48.5899] },
  { id: "h1", name: "Podpolianske slávnosti jesene", start: "2026-10-25T14:00:00+01:00", town: "Detva", venue: "Amfiteáter", category: "Folklór", url: "https://www.detva.sk/podujatia", at: [19.4194, 48.5604] },
  { id: "i1", name: "Revúcke literárne popoludnie", start: "2026-11-12T16:00:00+01:00", town: "Revúca", venue: "Mestská knižnica", category: "Literatúra", url: "https://www.revuca.sk/kniznica", at: [20.1172, 48.6832] },
  { id: "j1", name: "Hontianska paráda – jesenná edícia", start: "2026-11-21T10:00:00+01:00", town: "Veľký Krtíš", category: "Folklór", url: "https://www.velkykrtis.sk/parada", at: [19.3487, 48.2103] },
  { id: "k1", name: "Krupinské hudobné večery", start: "2026-11-19T19:00:00+01:00", town: "Krupina", venue: "Kostol Panny Márie", category: "Hudba", url: "https://www.krupina.sk/vecery", at: [19.0674, 48.3554] },
  { id: "l1", name: "Poltárske sklárske dni", start: "2026-11-07T10:00:00+01:00", town: "Poltár", category: "Remeslá", url: "https://www.poltar.sk/sklarske-dni", at: [19.7929, 48.4306] },
  { id: "m1", name: "Žarnovický adventný trh", start: "2026-11-28T15:00:00+01:00", town: "Žarnovica", venue: "Námestie SNP", category: "Jarmok", url: "javascript:alert(1)", at: [18.7208, 48.4842] },
  { id: "n1", name: "Hriňovské husle", start: "2026-11-15T16:00:00+01:00", town: "Hriňová", category: "Hudba", at: [19.5273, 48.5786] },
  { id: "o1", name: "Tanečný večer v Kremnici", start: "2026-12-04T19:00:00+01:00", town: "Kremnica", venue: "Mestský dom kultúry", category: "Tanec", url: "https://www.kremnica.sk/tanec" },
];

function entityOf(seed: Seed) {
  return {
    id: URN(seed.id),
    type: "Event",
    name: value(seed.name),
    startDate: value(seed.start),
    ...(seed.end ? { endDate: value(seed.end) } : {}),
    ...(seed.venue ? { venue: value(seed.venue) } : {}),
    address: value({ addressLocality: seed.town, addressRegion: "Banskobystrický kraj", addressCountry: "SK" }),
    ...(seed.category ? { eventCategory: value(seed.category) } : {}),
    ...(seed.url ? { url: value(seed.url) } : {}),
    source: value(`https://example.test/feed/${seed.id}`),
    dataProvider: value(`Mesto ${seed.town}`),
    ...(seed.at ? { location: point(seed.at[0], seed.at[1]) } : {}),
  };
}

/** The rows the endpoint answers; the last one has no start and the dashboard leaves it out. */
export const EVENTS = [
  ...SEEDS.map(entityOf),
  { id: URN("bez-zaciatku"), type: "Event", name: value("Podujatie bez dátumu"), address: value({ addressLocality: "Detva" }) },
];

/** The fourteen towns with a placed event. */
export const TOWNS = [...new Set(SEEDS.filter((seed) => seed.at).map((seed) => seed.town))];

/** `GET …/entities?type=Event&offset=…&limit=…` as the endpoint answers it. */
export function answer(url: URL, rows: unknown[] = EVENTS): unknown[] {
  if (url.searchParams.get("type") !== "Event") return [];
  const offset = Number(url.searchParams.get("offset") ?? 0);
  const limit = Number(url.searchParams.get("limit") ?? 100);
  return rows.slice(offset, offset + limit);
}
