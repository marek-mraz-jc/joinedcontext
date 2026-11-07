/**
 * Schools as the pipeline skoly writes them into `banskabystrica-verejne` (T-2781): counts as
 * Properties, the name as a LanguageProperty, a count the school map did not publish missing.
 */
const URN = (id: string) => `urn:ngsi-ld:Organization:banskabystrica.sk:banskabystrica-verejne:${id}`;
const v = (value: unknown) => ({ type: "Property", value });

function school(id: string, name: string, pupils?: number, teachers?: number, budget?: number) {
  const entity: Record<string, unknown> = {
    id: URN(id),
    type: "Organization",
    name: { type: "LanguageProperty", languageMap: { sk: name } },
    address: v(`${name.split(",").pop()?.trim() ?? name}, Banská Bystrica`),
  };
  if (pupils !== undefined) entity.pupilCount = v(pupils);
  if (teachers !== undefined) entity.teachingStaff = v(teachers);
  if (teachers !== undefined) entity.nonTeachingStaff = v(Math.round(teachers / 3));
  if (budget !== undefined) {
    entity.annualBudget = v(budget);
    entity.budgetYear = v(2024);
  }
  return entity;
}

export const SCHOOLS = [
  school("zs-moyzesova", "Základná škola, Moyzesova 18", 512, 40, 2_560_000),
  school("zs-ruzova", "Základná škola, Ružová 9", 420, 30, 2_100_000),
  school("zs-sitnianska", "Základná škola, Sitnianska 32", 380, 31, 1_900_000),
  school("zs-trieda-snp", "Základná škola, Trieda SNP 20", 610, 38, 2_900_000),
  school("zs-golianova", "Základná škola, Golianova 8", 300, 26, 1_650_000),
  school("zs-spojova", "Základná škola, Spojová 14", 455, 35, 2_200_000),
  school("zs-slnecna", "Základná škola, Slnečná 2", 250, 22, 1_400_000),
  school("zs-tatranska", "Základná škola, Tatranská 10", 540, 29, 1_600_000),
  school("zs-jana-cajaka", "Základná škola, Jána Čajaka 2", 330, 28, 1_800_000),
  school("gym-tajovskeho", "Gymnázium J. G. Tajovského, Tajovského 25", 690, 52, 3_400_000),
  school("sos-bez-udajov", "Stredná odborná škola, Tajovského 30"),
  school("zs-vzorec", "=HYPERLINK(\"http://example.org\"), Námestie 1", 100, 10, 500_000),
];
