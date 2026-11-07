/**
 * What the public endpoint of `bbsk-registre` answers, as the pipelines nemocnice,
 * socialne-sluzby and organizacie write it (T-2783): hospitals and social services as the Smart
 * Data Model PointOfInterest told apart by `category`, addresses as its PostalAddress (T-4329), names as LanguageProperty, positions as
 * GeoProperty where the register publishes one (social services have none), a missing value missing.
 */
const URN = (type: string, id: string) => `urn:ngsi-ld:${type}:bbsk.sk:bbsk-registre:${id}`;
const point = (lon: number, lat: number) => ({ type: "GeoProperty", value: { type: "Point", coordinates: [lon, lat] } });
const name = (sk: string) => ({ type: "LanguageProperty", languageMap: { sk } });
const value = (v: unknown) => ({ type: "Property", value: v });

export const HOSPITALS = [
  {
    id: URN("PointOfInterest", "fnspfdr"),
    type: "PointOfInterest",
    category: value(["hospital"]),
    name: name("Fakultná nemocnica s poliklinikou F. D. Roosevelta"),
    hospitalKind: value("general"),
    operatorName: value("Fakultná nemocnica s poliklinikou F. D. Roosevelta Banská Bystrica"),
    medicalSpecialties: value(["chirurgia", "interná medicína", "pediatria"]),
    address: value({ streetAddress: "Námestie L. Svobodu 1", addressLocality: "Banská Bystrica" }),
    location: point(19.1386, 48.7432),
  },
  {
    id: URN("PointOfInterest", "nemocnica-zvolen"),
    type: "PointOfInterest",
    category: value(["hospital"]),
    name: name("Nemocnica Zvolen"),
    hospitalKind: value("general"),
    address: value({ streetAddress: "Kuzmányho nábrežie 28", addressLocality: "Zvolen" }),
    location: point(19.13, 48.575),
  },
];

export const SOCIAL = [
  {
    id: URN("PointOfInterest", "dss-tisovec"),
    type: "PointOfInterest",
    category: value(["socialService"]),
    name: name("Domov sociálnych služieb Tisovec"),
    serviceKind: value("domov sociálnych služieb"),
    serviceForm: value("residentialYearRound"),
    targetGroup: value("dospelí s ťažkým zdravotným postihnutím"),
    capacity: value(48),
    providerKind: value("regionFounded"),
    address: value({ streetAddress: "Jesenského 869", addressLocality: "Tisovec" }),
    districtName: value("Rimavská Sobota"),
    url: value("https://www.dsstisovec.sk/"),
  },
  {
    id: URN("PointOfInterest", "terenna-sluzba"),
    type: "PointOfInterest",
    category: value(["socialService"]),
    name: name("Terénna opatrovateľská služba"),
    serviceForm: value("field"),
    districtName: value("Lučenec"),
    url: value("javascript:alert(1)"),
  },
];

export const ORGANIZATIONS = [
  {
    id: URN("PublicOrganization", "stredoslovenske-muzeum"),
    type: "PublicOrganization",
    name: name("Stredoslovenské múzeum"),
    organizationCategory: value("culture"),
    address: value({ streetAddress: "Námestie SNP 4", addressLocality: "Banská Bystrica" }),
    location: point(19.1455, 48.7362),
  },
  {
    id: URN("PublicOrganization", "spojena-skola-detva"),
    type: "PublicOrganization",
    name: name("Spojená škola Detva"),
    organizationCategory: value("school"),
    address: value({ streetAddress: "Štúrova 848", addressLocality: "Detva" }),
  },
];

/** The fixture key of one read: a PointOfInterest read is narrowed by its `category` query (T-4329). */
export function keyOf(url: URL): string | null {
  const type = url.searchParams.get("type");
  if (type !== "PointOfInterest") return type;
  const q = url.searchParams.get("q") ?? "";
  if (q.includes('category=="hospital"')) return "hospital";
  if (q.includes('category=="socialService"')) return "socialService";
  return type;
}

/** The rows of one read, keyed by `keyOf`, as `GET …/entities?type=…&q=…` answers them. */
export function answer(key: string | null): unknown[] {
  if (key === "hospital") return HOSPITALS;
  if (key === "socialService") return SOCIAL;
  if (key === "PointOfInterest") return [...HOSPITALS, ...SOCIAL];
  if (key === "PublicOrganization") return ORGANIZATIONS;
  return [];
}
