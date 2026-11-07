/**
 * Bridges as the pipeline mosty writes them into `bbsk-registre` (T-2783): the register's codes and
 * counts as Properties, the name as a LanguageProperty, a value the register did not publish missing.
 */
const URN = (id: string) => `urn:ngsi-ld:Bridge:bbsk.sk:bbsk-registre:${id}`;
const v = (value: unknown) => ({ type: "Property", value });

function bridge(id: string, name: string, extra: Record<string, unknown>) {
  const entity: Record<string, unknown> = { id: URN(id), type: "Bridge", name: { type: "LanguageProperty", languageMap: { sk: name } } };
  for (const [attr, value] of Object.entries(extra)) entity[attr] = v(value);
  return entity;
}

export const BRIDGES = [
  bridge("m1", "Most cez Hron v Banskej Bystrici", { bridgeCode: "66-001", roadClass: "firstClass", roadNumber: "I/66", yearBuilt: 1931, spanCount: 5, bridgedLength: 210, structureMaterial: "betón", heritageStatus: "culturalAndTechnical", managerName: "SSC", districtName: "Banská Bystrica" }),
  bridge("m2", "Most cez Slatinu", { bridgeCode: "66-014", roadClass: "firstClass", roadNumber: "I/66", yearBuilt: 1975, spanCount: 3, bridgedLength: 96, structureMaterial: "predpätý betón", heritageStatus: "notListed", districtName: "Zvolen" }),
  bridge("m3", "Most v Detve", { bridgeCode: "526-002", roadClass: "secondClass", roadNumber: "II/526", yearBuilt: 1962, spanCount: 2, bridgedLength: 34, heritageStatus: "notListed", districtName: "Detva" }),
  bridge("m4", "Most cez Ipeľ", { bridgeCode: "75-003", roadClass: "firstClass", roadNumber: "I/75", yearBuilt: 1988, spanCount: 4, bridgedLength: 140, heritageStatus: "notListed", districtName: "Lučenec" }),
  bridge("m5", "Most pri Brezne", { bridgeCode: "529-010", roadClass: "secondClass", roadNumber: "II/529", yearBuilt: 1958, spanCount: 1, bridgedLength: 18, heritageStatus: "notListed", districtName: "Brezno" }),
  bridge("m6", "Most v Revúcej", { bridgeCode: "532-004", roadClass: "secondClass", roadNumber: "II/532", yearBuilt: 2005, spanCount: 2, bridgedLength: 41, heritageStatus: "notListed", districtName: "Revúca" }),
  bridge("m7", "Most cez Rimavu", { bridgeCode: "571-001", roadClass: "thirdClass", roadNumber: "III/2752", yearBuilt: 1969, spanCount: 3, bridgedLength: 52, heritageStatus: "notListed", districtName: "Rimavská Sobota" }),
  bridge("m8", "Kamenný most v Štiavnici", { bridgeCode: "51-020", roadClass: "local", yearBuilt: 1850, spanCount: 3, bridgedLength: 30, structureMaterial: "kameň", heritageStatus: "cultural", districtName: "Banská Štiavnica" }),
  bridge("m9", "Most v Krupine", { bridgeCode: "526-030", roadClass: "secondClass", roadNumber: "II/526", yearBuilt: 1979, spanCount: 2, bridgedLength: 27, heritageStatus: "notListed", districtName: "Krupina" }),
  bridge("m10", "Most v Žiari", { bridgeCode: "65-040", roadClass: "firstClass", roadNumber: "I/65", yearBuilt: 1992, spanCount: 6, bridgedLength: 260, heritageStatus: "notListed", districtName: "Žiar nad Hronom" }),
  bridge("m11", "Most bez údajov", { bridgeCode: "x-1", districtName: "Poltár" }),
  bridge("m12", "=CMD()", { bridgeCode: "x-2", roadClass: "service", yearBuilt: 2999, spanCount: 1, bridgedLength: 9, districtName: "Poltár" }),
];
