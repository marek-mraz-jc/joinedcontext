import type { Row } from "@joinedcontext/sdk";

const alert = (local: string, name: string, severity: string, status: string, district: string, day: number, category: string): Row => ({
  id: `urn:ngsi-ld:Alert:hel.fi:alerts:${local}`,
  type: "Alert",
  name,
  description: `${name}: reported by the ${category.toLowerCase()} feed.`,
  category,
  severity,
  status,
  district,
  dateIssued: `2026-09-${String(day).padStart(2, "0")}T07:30:00Z`,
});

/** A queue as the endpoint answers it: every severity, every status, three districts. */
export const ROWS: Row[] = [
  alert("a1", "Water main burst on Mannerheimintie", "critical", "open", "Kamppi", 24, "Water"),
  alert("a2", "Traffic lights out at Hakaniemi", "high", "open", "Kallio", 23, "Traffic"),
  alert("a3", "Tram line 4 diverted", "medium", "acknowledged", "Kamppi", 22, "Transport"),
  alert("a4", "Street lighting fault", "low", "open", "Vallila", 21, "Lighting"),
  alert("a5", "Flooded underpass", "high", "resolved", "Vallila", 20, "Water"),
  alert("a6", "Fallen tree on cycle path", "medium", "open", "Kallio", 24, "Parks"),
  alert("a7", "Power cut in three blocks", "critical", "acknowledged", "Vallila", 23, "Energy"),
  alert("a8", "Bin collection delayed", "low", "resolved", "Kamppi", 19, "Waste"),
];

/** Everyone reads; the editor role may change `status` and nothing else. */
export const EDITOR_ACCESS = {
  permissions: [
    { resource: { type: "Alert" }, actions: ["queryEntity", "retrieveEntity"], attributes: "*" as const },
    { resource: { type: "Alert" }, actions: ["updateAttrs"], attributes: ["status"] },
  ],
  prohibitions: [],
};

export const VIEWER_ACCESS = {
  permissions: [{ resource: { type: "Alert" }, actions: ["queryEntity", "retrieveEntity"], attributes: "*" as const }],
  prohibitions: [],
};
