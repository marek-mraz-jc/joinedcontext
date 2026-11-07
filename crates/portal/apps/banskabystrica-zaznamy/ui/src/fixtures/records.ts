/**
 * What a raw space answers, as the gateway shapes it (T-2436, T-2437).
 *
 * One publisher's rows, of the shape `statistical-observation` 1.1.0 declares: the publisher's
 * own codes, the number with its unit on the Property, and the steward's note where somebody has
 * written one.
 */
export interface RecordFixture {
  localId: string;
  dataSet: string;
  indicator: string;
  refArea: string;
  refPeriod: string;
  value: number;
  unitCode?: string;
  unitText?: string;
  dateObserved: string;
  note?: string;
}

export function record(one: RecordFixture, space: string, domain: string): Record<string, unknown> {
  const entity: Record<string, unknown> = {
    id: `urn:ngsi-ld:KeyPerformanceIndicator:${domain}:${space}:${one.localId}`,
    type: "KeyPerformanceIndicator",
    dataSet: { type: "Property", value: one.dataSet },
    indicator: { type: "Property", value: one.indicator },
    refArea: { type: "Property", value: one.refArea },
    refPeriod: { type: "Property", value: one.refPeriod },
    kpiValue: { type: "Property", value: one.value, unitCode: one.unitCode ?? "MTQ" },
    source: { type: "Property", value: `https://data.statistics.sk/api/v2/dataset/${one.dataSet}` },
    dateObserved: { type: "Property", value: one.dateObserved },
  };
  if (one.unitText !== undefined) {
    entity.unitText = { type: "Property", value: one.unitText };
  }
  if (one.note !== undefined) {
    entity.stewardNote = { type: "Property", value: one.note };
  }
  return entity;
}

export const CITY: RecordFixture[] = [
  {
    localId: "vh5003rr-SK0321508438-2023-U03084",
    dataSet: "vh5003rr",
    indicator: "U03084",
    refArea: "SK0321508438",
    refPeriod: "2023",
    value: 4061.2,
    unitText: "v tis. m3 fakturovanej vody",
    dateObserved: "2026-08-14T00:00:00Z",
  },
  {
    localId: "vh5003rr-SK0321508438-2022-U03084",
    dataSet: "vh5003rr",
    indicator: "U03084",
    refArea: "SK0321508438",
    refPeriod: "2022",
    value: 3987.5,
    unitText: "v tis. m3 fakturovanej vody",
    dateObserved: "2026-08-14T00:00:00Z",
    note: "Porovnané s ročenkou mesta, sedí.",
  },
  {
    localId: "om7003rr-SK0321508438-2023-U10061",
    dataSet: "om7003rr",
    indicator: "U10061",
    refArea: "SK0321508438",
    refPeriod: "2023",
    value: 72123,
    unitCode: "C62",
    unitText: "osoby",
    dateObserved: "2026-08-02T00:00:00Z",
  },
];

export const REGION: RecordFixture[] = [
  {
    localId: "zp3803rs-SK032-2023-PROD_TONY-1",
    dataSet: "zp3803rs",
    indicator: "PROD_TONY",
    refArea: "SK032",
    refPeriod: "2023",
    value: 1893.4,
    unitCode: "TNE",
    unitText: "tony",
    dateObserved: "2026-08-11T00:00:00Z",
  },
  {
    localId: "om7003rr-SK032-2023-U10061",
    dataSet: "om7003rr",
    indicator: "U10061",
    refArea: "SK032",
    refPeriod: "2023",
    value: 607581,
    unitCode: "C62",
    unitText: "osoby",
    dateObserved: "2026-08-02T00:00:00Z",
  },
];

export function answer(
  rows: RecordFixture[],
  space = "banskabystrica-mesto",
  domain = "banskabystrica.sk",
): Record<string, unknown>[] {
  return rows.map((one) => record(one, space, domain));
}
