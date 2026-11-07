/**
 * Sensor containers and their isles as the pipelines waste-fill and waste-stations write them into
 * `praha-mesto` (T-2785): fill 0 to 1, the reading's time as `dateModified`, the isle a Relationship.
 */
const URN = (type: string, id: string) => `urn:ngsi-ld:${type}:praha.eu:praha-mesto:${id}`;
const v = (value: unknown) => ({ type: "Property", value });
export const NOW = new Date("2026-10-06T12:00:00Z");
const hoursAgo = (h: number) => new Date(NOW.getTime() - h * 3_600_000).toISOString();

function container(id: string, code: string, kind: string, fill: number | null, ageHours: number | null, isle: string | null) {
  const entity: Record<string, unknown> = { id: URN("WasteContainer", id), type: "WasteContainer", containerCode: v(code), wasteKind: v(kind) };
  if (fill !== null) entity.fillingLevel = v(fill);
  if (ageHours !== null) entity.dateModified = v(hoursAgo(ageHours));
  if (isle) entity.refWasteContainerIsle = { type: "Relationship", object: URN("WasteContainerIsle", isle) };
  return entity;
}

export const CONTAINERS = [
  container("c1", "0001-PAP", "paper", 0.95, 2, "i1"),
  container("c2", "0001-PLA", "plastic", 0.4, 2, "i1"),
  container("c3", "0002-PAP", "paper", 0.7, 5, "i2"),
  container("c4", "0002-GLS", "colouredGlass", 0.2, 30, "i2"),
  container("c5", "0003-PLA", "plastic", 0.55, 1, "i3"),
  container("c6", "0003-MET", "metal", 0.1, 8, "i3"),
  container("c7", "0004-PAP", "paper", 0.88, 3, "i4"),
  container("c8", "0004-PLA", "plastic", 0.3, 4, "i4"),
  container("c9", "0005-GLS", "clearGlass", 0.45, 200, "i5"),
  container("c10", "0005-CRT", "beverageCartons", 0.6, 6, "i5"),
  container("c11", "0006-PAP", "paper", null, null, null),
  container("c12", "=0006-X", "plastic", 0.5, 7, "neznamy"),
];

export const ISLES = [
  { id: URN("WasteContainerIsle", "i1"), type: "WasteContainerIsle", name: { type: "LanguageProperty", languageMap: { cs: "Vinohradská 12" } } },
  { id: URN("WasteContainerIsle", "i2"), type: "WasteContainerIsle", name: { type: "LanguageProperty", languageMap: { cs: "Korunní 40" } } },
  { id: URN("WasteContainerIsle", "i3"), type: "WasteContainerIsle", name: { type: "LanguageProperty", languageMap: { cs: "Slezská 3" } } },
  { id: URN("WasteContainerIsle", "i4"), type: "WasteContainerIsle", name: { type: "LanguageProperty", languageMap: { cs: "Mánesova 70" } } },
  { id: URN("WasteContainerIsle", "i5"), type: "WasteContainerIsle", name: { type: "LanguageProperty", languageMap: { cs: "Italská 5" } } },
];

/** What `GET …/entities` answers: containers by type, isles by the ids asked for. */
export function answer(type: string | null, ids: string[]): unknown[] {
  if (type === "WasteContainer") return CONTAINERS;
  if (type === "WasteContainerIsle") return ISLES.filter((isle) => ids.length === 0 || ids.includes(isle.id));
  return [];
}
